//! Reusable workflows: versioned, parameterized tool sequences with
//! preconditions and postconditions.
//!
//! A workflow replaces repeated planning for a task that has been done
//! before. It is only as trustworthy as its checks, so:
//!
//! - it runs only if the user trusted this exact file (its SHA-256 is in the
//!   trust list); a file the agent wrote or changed is refused until the user
//!   reviews it — one successful run never promotes itself;
//! - its preconditions are checked against live state before any step runs;
//!   a failure means the application drifted and the workflow needs repair,
//!   and nothing is done;
//! - every step goes through the tool pipeline, so authorization, approval
//!   and journalling apply exactly as to a call the model made;
//! - it stops at the first failed step and reports the completed prefix;
//! - its postconditions are checked afterwards; a run whose postconditions
//!   fail is reported as failed even if every step succeeded.
//!
//! Steps address elements semantically (role, label, selector), never by
//! recorded coordinates or observation refs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hame_agent::{ToolPipeline, ToolRequest};
use hame_kernel::BoxFuture;
use hame_session::ToolSpec;
use hame_session::contract::{Capability, Channel, Predicate};
use hame_tools::{Effect, Tool, ToolContext, ToolOutput, ToolSet};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::outcome::{StateProbe, describe, verify};

pub const WORKFLOW_TOOL: &str = "workflow";
pub const TRUST_FILE: &str = "trusted-workflows.json";
const MAX_STEPS: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamSpec {
    #[serde(default)]
    pub description: String,
    #[serde(default = "yes")]
    pub required: bool,
    /// A regular expression the whole value must match.
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    pub channel: Channel,
    pub capability: Capability,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub tool: String,
    pub input: Value,
    /// What the step is for, shown in reports.
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub name: String,
    pub version: u32,
    pub description: String,
    /// Who wrote it and from what.
    #[serde(default)]
    pub provenance: String,
    /// Application versions it was tested against.
    #[serde(default)]
    pub tested_with: Vec<String>,
    #[serde(default)]
    pub params: BTreeMap<String, ParamSpec>,
    /// Grants the workflow needs; checked before it runs.
    #[serde(default)]
    pub requires: Vec<Requirement>,
    #[serde(default, rename = "pre")]
    pub preconditions: Vec<Predicate>,
    pub steps: Vec<Step>,
    #[serde(default, rename = "post")]
    pub postconditions: Vec<Predicate>,
}

impl Workflow {
    pub fn parse(text: &str) -> Result<Self, String> {
        let workflow: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        if workflow.steps.is_empty() || workflow.steps.len() > MAX_STEPS {
            return Err(format!("a workflow has 1 to {MAX_STEPS} steps"));
        }
        if workflow.postconditions.is_empty() {
            return Err("a workflow needs at least one postcondition".into());
        }
        for predicate in workflow
            .preconditions
            .iter()
            .chain(&workflow.postconditions)
        {
            predicate.validate()?;
        }
        for (name, spec) in &workflow.params {
            if let Some(pattern) = &spec.pattern {
                regex::Regex::new(pattern).map_err(|e| format!("param {name}: {e}"))?;
            }
        }
        Ok(workflow)
    }

    /// Checks `given` against the parameter specs and fills defaults.
    pub fn bind(
        &self,
        given: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, String>, String> {
        for name in given.keys() {
            if !self.params.contains_key(name) {
                return Err(format!("unknown parameter {name:?}"));
            }
        }
        let mut bound = BTreeMap::new();
        for (name, spec) in &self.params {
            let value = match given.get(name).or(spec.default.as_ref()) {
                Some(value) => value.clone(),
                None if spec.required => return Err(format!("missing parameter {name:?}")),
                None => continue,
            };
            if let Some(pattern) = &spec.pattern {
                let re =
                    regex::Regex::new(&format!("^(?:{pattern})$")).map_err(|e| e.to_string())?;
                if !re.is_match(&value) {
                    return Err(format!("parameter {name:?} does not match {pattern}"));
                }
            }
            bound.insert(name.clone(), value);
        }
        Ok(bound)
    }
}

/// Replaces `{{name}}` in every string of `value`. A reference to an
/// unbound name is an error rather than an empty string.
pub fn substitute(value: &Value, params: &BTreeMap<String, String>) -> Result<Value, String> {
    Ok(match value {
        Value::String(text) => {
            let mut out = String::new();
            let mut rest = text.as_str();
            while let Some(start) = rest.find("{{") {
                out.push_str(&rest[..start]);
                let after = &rest[start + 2..];
                let end = after
                    .find("}}")
                    .ok_or_else(|| format!("unclosed {{{{ in {text:?}"))?;
                let name = after[..end].trim();
                out.push_str(
                    params
                        .get(name)
                        .ok_or_else(|| format!("unbound parameter {name:?}"))?,
                );
                rest = &after[end + 2..];
            }
            out.push_str(rest);
            Value::String(out)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| substitute(item, params))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(k, v)| Ok((k.clone(), substitute(v, params)?)))
                .collect::<Result<_, String>>()?,
        ),
        other => other.clone(),
    })
}

fn substitute_predicates(
    predicates: &[Predicate],
    params: &BTreeMap<String, String>,
) -> Result<Vec<Predicate>, String> {
    predicates
        .iter()
        .map(|p| {
            let value = serde_json::to_value(p).map_err(|e| e.to_string())?;
            // A parameter spliced into a shell command is one word, whatever
            // it contains: the model supplies parameters.
            let value = if matches!(p, Predicate::Command { .. }) {
                let quoted: BTreeMap<String, String> = params
                    .iter()
                    .map(|(k, v)| (k.clone(), shell_quote(v)))
                    .collect();
                substitute(&value, &quoted)?
            } else {
                substitute(&value, params)?
            };
            let predicate: Predicate = serde_json::from_value(value).map_err(|e| e.to_string())?;
            predicate.validate()?;
            Ok(predicate)
        })
        .collect()
}

/// `text` as a single POSIX shell word.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The set of workflow file hashes the user has reviewed and trusted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustList {
    /// name → sha256 of the trusted file.
    pub trusted: BTreeMap<String, String>,
}

impl TrustList {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        hame_tools::write_atomic(path, serde_json::to_string_pretty(self)?.as_bytes())
    }

    pub fn is_trusted(&self, name: &str, text: &str) -> bool {
        self.trusted
            .get(name)
            .is_some_and(|sha| *sha == hame_session::artifact::sha256_hex(text.as_bytes()))
    }

    pub fn trust(&mut self, name: &str, text: &str) {
        self.trusted.insert(
            name.to_string(),
            hame_session::artifact::sha256_hex(text.as_bytes()),
        );
    }
}

/// Finds `name.toml` in the workflow directories, first match wins.
pub fn find(dirs: &[PathBuf], name: &str) -> Option<(PathBuf, String)> {
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    dirs.iter().find_map(|dir| {
        let path = dir.join(format!("{name}.toml"));
        std::fs::read_to_string(&path).ok().map(|text| (path, text))
    })
}

/// Every workflow in the directories: (name, description, trusted).
pub fn list(dirs: &[PathBuf], trust: &TrustList) -> Vec<(String, String, bool)> {
    let mut seen = BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "toml") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if seen.contains_key(name) {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                let description = Workflow::parse(&text)
                    .map(|w| w.description)
                    .unwrap_or_else(|e| format!("(invalid: {e})"));
                seen.insert(
                    name.to_string(),
                    (description, trust.is_trusted(name, &text)),
                );
            }
        }
    }
    seen.into_iter().map(|(n, (d, t))| (n, d, t)).collect()
}

/// The outcome of one workflow run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunReport {
    pub name: String,
    pub passed: bool,
    /// `precondition`, `step`, `postcondition`, or empty when passed.
    pub stopped_at: String,
    pub completed_steps: usize,
    pub text: String,
}

/// Runs workflows as a tool.
pub struct WorkflowTool {
    pub dirs: Vec<PathBuf>,
    pub trust_file: PathBuf,
    /// The tools steps may call.
    pub tools: ToolSet,
    pub pipeline: Arc<ToolPipeline>,
    pub probe: Option<Arc<dyn StateProbe>>,
    /// Confines `command` pre- and postconditions.
    pub confine: Option<crate::outcome::Confinement>,
    pub authorization: hame_session::contract::Authorization,
    /// Runs steps whose tool is not host-side, so they are confined as the
    /// model's own calls are. `None` runs them in this process.
    pub runner: Option<Arc<dyn hame_tools::ToolRunner>>,
}

impl WorkflowTool {
    pub async fn run(
        &self,
        name: &str,
        params: &BTreeMap<String, String>,
        ctx: &ToolContext,
    ) -> Result<RunReport, String> {
        let (path, text) = find(&self.dirs, name)
            .ok_or_else(|| format!("There is no workflow named {name:?}."))?;
        let trust = TrustList::load(&self.trust_file);
        if !trust.is_trusted(name, &text) {
            return Err(format!(
                "Workflow {name:?} ({}) is not trusted in this exact version. A person must \
                 review it and run `hame workflow trust {name}`; it cannot be trusted from \
                 inside a session.",
                path.display()
            ));
        }
        let workflow = Workflow::parse(&text)?;
        let params = workflow.bind(params)?;
        for need in &workflow.requires {
            let granted =
                self.authorization.grants.iter().any(|g| {
                    g.channel == need.channel && g.capabilities.contains(&need.capability)
                });
            if !granted {
                return Err(format!(
                    "Workflow {name:?} needs {:?} on {:?}, which is not granted.",
                    need.capability, need.channel
                ));
            }
        }
        let contract = |id: &str, acceptance| hame_session::contract::TaskContract {
            version: hame_session::contract::CONTRACT_VERSION,
            id: format!("{name}:{id}"),
            outcome: workflow.description.clone(),
            constraints: vec![],
            acceptance,
            authorization: Default::default(),
            budget: Default::default(),
        };
        let mut report = RunReport {
            name: name.to_string(),
            passed: false,
            stopped_at: String::new(),
            completed_steps: 0,
            text: format!("Workflow {name} v{}\n", workflow.version),
        };
        let pre = substitute_predicates(&workflow.preconditions, &params)?;
        if !pre.is_empty() {
            let check = verify(
                &contract("pre", pre),
                &ctx.cwd,
                self.probe.as_deref(),
                self.confine.as_ref(),
            )
            .await;
            if !check.passed {
                report.stopped_at = "precondition".into();
                report.text.push_str(&format!(
                    "Preconditions failed, so nothing was done. The application state differs \
                     from what this workflow was built for (drift); it needs repair or the task \
                     needs another approach.\n{}",
                    describe(&check)
                ));
                return Ok(report);
            }
        }
        for (index, step) in workflow.steps.iter().enumerate() {
            let Some(tool) = self.tools.get(&step.tool) else {
                report.stopped_at = "step".into();
                report.text.push_str(&format!(
                    "Step {}: no tool named {:?}.\n",
                    index + 1,
                    step.tool
                ));
                return Ok(report);
            };
            let input = substitute(&step.input, &params)?;
            let request = ToolRequest {
                call_id: format!("workflow-{name}-{}", index + 1),
                name: step.tool.clone(),
                input,
                effect: tool.effect(),
            };
            let output = self
                .pipeline
                .run(request, |request: ToolRequest| {
                    let tool = Arc::clone(&tool);
                    async move {
                        match &self.runner {
                            Some(runner) if !tool.host_side() => {
                                runner.run(&tool, &request.input, ctx).await
                            }
                            _ => tool.call(&request.input, ctx).await,
                        }
                    }
                })
                .await;
            let first_line = output.text.lines().next().unwrap_or("").to_string();
            report.text.push_str(&format!(
                "Step {} {}{}: {}\n",
                index + 1,
                step.tool,
                if step.note.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", step.note)
                },
                if output.is_error { "FAILED" } else { "ok" },
            ));
            if output.is_error {
                report.stopped_at = "step".into();
                report.text.push_str(&format!(
                    "Stopped after {} completed steps: {first_line}\n{}",
                    report.completed_steps, output.text
                ));
                return Ok(report);
            }
            report.completed_steps += 1;
        }
        let post = substitute_predicates(&workflow.postconditions, &params)?;
        let check = verify(
            &contract("post", post),
            &ctx.cwd,
            self.probe.as_deref(),
            self.confine.as_ref(),
        )
        .await;
        report.passed = check.passed;
        if !check.passed {
            report.stopped_at = "postcondition".into();
        }
        report.text.push_str(&describe(&check));
        Ok(report)
    }
}

impl Tool for WorkflowTool {
    fn spec(&self) -> ToolSpec {
        let catalog = list(&self.dirs, &TrustList::load(&self.trust_file))
            .into_iter()
            .map(|(name, description, trusted)| {
                format!(
                    "{name}{}: {description}",
                    if trusted { "" } else { " (untrusted)" }
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        ToolSpec {
            name: WORKFLOW_TOOL.into(),
            description: format!(
                "Run a saved, user-trusted workflow: a tested sequence of tool calls with \
                 checks before and after. It refuses to run when the application has changed \
                 (preconditions fail) and reports failure when the result is not as expected. \
                 Prefer it over doing the same steps by hand. Available: {}",
                if catalog.is_empty() {
                    "none".into()
                } else {
                    catalog
                }
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "params": {"type": "object", "additionalProperties": {"type": "string"}}
                },
                "required": ["name"],
                "additionalProperties": false
            }),
        }
    }

    /// Each step is ruled on by itself.
    fn effect(&self) -> Effect {
        Effect::Control
    }

    fn host_side(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let Some(name) = input["name"].as_str() else {
                return ToolOutput::error("Invalid input: name is required.");
            };
            let params: BTreeMap<String, String> = match input.get("params") {
                None | Some(Value::Null) => BTreeMap::new(),
                Some(value) => match serde_json::from_value(value.clone()) {
                    Ok(params) => params,
                    Err(e) => return ToolOutput::error(format!("Invalid params: {e}")),
                },
            };
            match self.run(name, &params, ctx).await {
                Ok(report) if report.passed => ToolOutput::ok(report.text),
                Ok(report) => ToolOutput::error(report.text),
                Err(e) => ToolOutput::error(e),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hame_tools::{Read, Write};

    const FLOW: &str = r#"
name = "greet"
version = 1
description = "Write a greeting file"
provenance = "test"
tested_with = ["hame write tool"]
[params.who]
pattern = "[A-Za-zÀ-ỹ ]+"
[params.file]
default = "greeting.txt"
[[pre]]
kind = "file_exists"
path = "ready.flag"
[[steps]]
tool = "write"
input = { path = "{{file}}", content = "Xin chào {{who}}" }
note = "write the greeting"
[[post]]
kind = "file_contains"
path = "{{file}}"
text = "Xin chào {{who}}"
"#;

    fn tool(dir: &Path) -> WorkflowTool {
        let mut tools = ToolSet::new();
        tools.insert(Arc::new(Write));
        tools.insert(Arc::new(Read));
        WorkflowTool {
            dirs: vec![dir.join("flows")],
            trust_file: dir.join(TRUST_FILE),
            tools,
            pipeline: Arc::new(ToolPipeline::new()),
            probe: None,
            confine: None,
            authorization: Default::default(),
            runner: None,
        }
    }

    #[tokio::test]
    async fn parameters_in_command_checks_stay_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let post = vec![Predicate::Command {
            command: "test -s {{file}}".into(),
            exit: 0,
        }];
        let hostile = format!("x; touch '{}'", marker.display());
        let bound = substitute_predicates(&post, &params(&[("file", &hostile)])).unwrap();
        let check = verify(&contract_of(bound), dir.path(), None, None).await;
        assert!(!marker.exists(), "the parameter ran as a command");
        assert!(!check.passed, "test -s on a missing file passed");
        // Quotes inside the value do not end the word either.
        let bound = substitute_predicates(&post, &params(&[("file", "a'; touch b; '")])).unwrap();
        let _ = verify(&contract_of(bound), dir.path(), None, None).await;
        assert!(!dir.path().join("b").exists());
        // An empty condition in a workflow file is refused when it is read.
        let vacuous = FLOW.replace("text = \"Xin chào {{who}}\"", "text = \"\"");
        assert!(Workflow::parse(&vacuous).unwrap_err().contains("empty"));
    }

    fn contract_of(acceptance: Vec<Predicate>) -> hame_session::contract::TaskContract {
        hame_session::contract::TaskContract {
            version: 1,
            id: "t".into(),
            outcome: "t".into(),
            constraints: vec![],
            acceptance,
            authorization: Default::default(),
            budget: Default::default(),
        }
    }

    fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[tokio::test]
    async fn only_trusted_versions_run_and_drift_stops_before_any_step() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("flows")).unwrap();
        std::fs::write(dir.path().join("flows/greet.toml"), FLOW).unwrap();
        let tool = tool(dir.path());
        let ctx = ToolContext::new(dir.path());

        let refused = tool.run("greet", &params(&[("who", "An")]), &ctx).await;
        assert!(refused.unwrap_err().contains("not trusted"));

        let mut trust = TrustList::default();
        trust.trust("greet", FLOW);
        trust.save(&tool.trust_file).unwrap();

        // Precondition fails: drift, nothing written.
        let report = tool
            .run("greet", &params(&[("who", "An")]), &ctx)
            .await
            .unwrap();
        assert!(!report.passed);
        assert_eq!(report.stopped_at, "precondition");
        assert!(!dir.path().join("greeting.txt").exists());

        std::fs::write(dir.path().join("ready.flag"), "").unwrap();
        let report = tool
            .run("greet", &params(&[("who", "Đức Anh")]), &ctx)
            .await
            .unwrap();
        assert!(report.passed, "{}", report.text);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("greeting.txt")).unwrap(),
            "Xin chào Đức Anh"
        );

        // Bad parameters are refused before anything runs.
        assert!(
            tool.run("greet", &params(&[("who", "a;rm")]), &ctx)
                .await
                .is_err()
        );
        assert!(
            tool.run("greet", &params(&[("bogus", "x")]), &ctx)
                .await
                .is_err()
        );

        // Editing the file revokes trust.
        std::fs::write(
            dir.path().join("flows/greet.toml"),
            FLOW.replace("Xin chào", "Hello"),
        )
        .unwrap();
        assert!(
            tool.run("greet", &params(&[("who", "An")]), &ctx)
                .await
                .is_err()
        );
    }

    #[test]
    fn workflows_need_steps_and_postconditions_and_reject_bad_names() {
        assert!(Workflow::parse(&FLOW.replace("[[post]]", "[[unused]]")).is_err());
        assert!(find(&[PathBuf::from(".")], "../etc/passwd").is_none());
        let bound = Workflow::parse(FLOW)
            .unwrap()
            .bind(&params(&[("who", "An")]))
            .unwrap();
        assert_eq!(bound["file"], "greeting.txt");
        assert!(substitute(&json!("{{nope}}"), &bound).is_err());
    }
}
