//! `hame workflow`: list saved workflows, show one, and trust or untrust
//! an exact version of it.
//!
//! Trust is the user's decision and is made here, outside any session: the
//! agent's tools cannot write hame's data directory, and the `workflow`
//! tool refuses a file whose SHA-256 is not in the trust list. Editing a
//! trusted file revokes its trust.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;
use hame_verify::task_file::describe_predicate;
use hame_verify::workflow::{TRUST_FILE, TrustList, Workflow, find, list};

#[derive(Subcommand)]
pub enum WorkflowCommand {
    /// List the workflows this directory sees, and whether each is trusted.
    List,
    /// Print a workflow: what it needs, checks and does, then its file.
    Show { name: String },
    /// Trust this exact version of a workflow after reviewing it. The agent
    /// can then run it; editing the file revokes the trust.
    Trust {
        name: String,
        /// Do not ask for confirmation (for scripts; you have reviewed the
        /// file). Without it, trusting needs a terminal.
        #[arg(long)]
        yes: bool,
    },
    /// Remove a workflow from the trust list.
    Untrust { name: String },
}

/// Where workflows are looked for, first match wins. Must match the
/// directories `hame_app::build` gives the `workflow` tool.
pub fn workflow_dirs(home: &Path, cwd: &Path) -> Vec<PathBuf> {
    vec![cwd.join(".hame").join("workflows"), home.join("workflows")]
}

pub fn run(home: &Path, command: WorkflowCommand) -> Result<ExitCode, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let dirs = workflow_dirs(home, &cwd);
    let trust_file = home.join(TRUST_FILE);
    let mut out = std::io::stdout().lock();
    match command {
        WorkflowCommand::List => list_workflows(&dirs, &trust_file, &mut out)?,
        WorkflowCommand::Show { name } => show(&dirs, &trust_file, &name, &mut out)?,
        WorkflowCommand::Trust { name, yes } => {
            let interactive = std::io::stdin().is_terminal();
            if !yes && !interactive {
                return Err(
                    "trusting a workflow needs confirmation at a terminal; review it with \
                     `hame workflow show` and pass --yes to trust it from a script"
                        .into(),
                );
            }
            let confirm =
                |question: &str| -> Result<bool, String> {
                    if yes {
                        return Ok(true);
                    }
                    print!("{question} [y/N]: ");
                    std::io::stdout().flush().map_err(|e| e.to_string())?;
                    let mut line = String::new();
                    std::io::stdin()
                        .lock()
                        .read_line(&mut line)
                        .map_err(|e| e.to_string())?;
                    Ok(line.trim().eq_ignore_ascii_case("y")
                        || line.trim().eq_ignore_ascii_case("yes"))
                };
            if !trust(&dirs, &trust_file, &name, &mut out, confirm)? {
                writeln!(out, "Not trusted; nothing changed.").map_err(|e| e.to_string())?;
                return Ok(ExitCode::FAILURE);
            }
        }
        WorkflowCommand::Untrust { name } => untrust(&trust_file, &name, &mut out)?,
    }
    Ok(ExitCode::SUCCESS)
}

fn io(error: std::io::Error) -> String {
    error.to_string()
}

fn list_workflows(dirs: &[PathBuf], trust_file: &Path, out: &mut impl Write) -> Result<(), String> {
    let trust = TrustList::load(trust_file);
    let found = list(dirs, &trust);
    if found.is_empty() {
        writeln!(
            out,
            "No workflows. Put NAME.toml files in {} or {}.",
            dirs[0].display(),
            dirs[1].display()
        )
        .map_err(io)?;
        return Ok(());
    }
    for (name, description, trusted) in found {
        let path = find(dirs, &name)
            .map(|(path, _)| path.display().to_string())
            .unwrap_or_default();
        writeln!(
            out,
            "{name}\t{}\t{description}\t{path}",
            if trusted { "trusted" } else { "untrusted" }
        )
        .map_err(io)?;
    }
    Ok(())
}

/// Loads a workflow by name: its path, text and parsed form.
fn load(dirs: &[PathBuf], name: &str) -> Result<(PathBuf, String, Workflow), String> {
    let (path, text) = find(dirs, name).ok_or_else(|| {
        format!(
            "there is no workflow named {name:?} in {} or {}",
            dirs[0].display(),
            dirs[1].display()
        )
    })?;
    let workflow = Workflow::parse(&text)
        .map_err(|e| format!("{} is not a valid workflow: {e}", path.display()))?;
    Ok((path, text, workflow))
}

/// What a person needs to judge a workflow before trusting it.
fn summary(workflow: &Workflow) -> String {
    let mut text = format!(
        "{} v{}: {}\n",
        workflow.name, workflow.version, workflow.description
    );
    if !workflow.provenance.is_empty() {
        text.push_str(&format!("provenance: {}\n", workflow.provenance));
    }
    if !workflow.tested_with.is_empty() {
        text.push_str(&format!(
            "tested with: {}\n",
            workflow.tested_with.join(", ")
        ));
    }
    for (name, spec) in &workflow.params {
        text.push_str(&format!(
            "param {name}{}{}{}{}\n",
            if spec.required && spec.default.is_none() {
                " (required)"
            } else {
                ""
            },
            spec.pattern
                .as_ref()
                .map(|p| format!(" matching {p}"))
                .unwrap_or_default(),
            spec.default
                .as_ref()
                .map(|d| format!(" default {d:?}"))
                .unwrap_or_default(),
            if spec.description.is_empty() {
                String::new()
            } else {
                format!(": {}", spec.description)
            }
        ));
    }
    for need in &workflow.requires {
        text.push_str(&format!(
            "needs grant: {:?} on {:?}\n",
            need.capability, need.channel
        ));
    }
    for predicate in &workflow.preconditions {
        text.push_str(&format!("before: {}\n", describe_predicate(predicate)));
    }
    for (index, step) in workflow.steps.iter().enumerate() {
        text.push_str(&format!(
            "step {}: {} {}{}\n",
            index + 1,
            step.tool,
            step.input,
            if step.note.is_empty() {
                String::new()
            } else {
                format!(" ({})", step.note)
            }
        ));
    }
    for predicate in &workflow.postconditions {
        text.push_str(&format!("after: {}\n", describe_predicate(predicate)));
    }
    text
}

fn show(
    dirs: &[PathBuf],
    trust_file: &Path,
    name: &str,
    out: &mut impl Write,
) -> Result<(), String> {
    let (path, text, workflow) = load(dirs, name)?;
    let trusted = TrustList::load(trust_file).is_trusted(name, &text);
    writeln!(out, "{}", path.display()).map_err(io)?;
    writeln!(
        out,
        "sha256 {} ({})",
        hame_session::artifact::sha256_hex(text.as_bytes()),
        if trusted {
            "trusted"
        } else {
            "not trusted in this version"
        }
    )
    .map_err(io)?;
    write!(out, "{}", summary(&workflow)).map_err(io)?;
    writeln!(out, "\n--- {} ---\n{text}", path.display()).map_err(io)?;
    Ok(())
}

/// Shows the workflow and records its hash once `confirm` agrees. Returns
/// whether it is trusted afterwards.
fn trust(
    dirs: &[PathBuf],
    trust_file: &Path,
    name: &str,
    out: &mut impl Write,
    confirm: impl FnOnce(&str) -> Result<bool, String>,
) -> Result<bool, String> {
    let (path, text, workflow) = load(dirs, name)?;
    let mut list = TrustList::load(trust_file);
    if list.is_trusted(name, &text) {
        writeln!(
            out,
            "{name} ({}) is already trusted in this version.",
            path.display()
        )
        .map_err(io)?;
        return Ok(true);
    }
    writeln!(out, "{}\n{}", path.display(), summary(&workflow)).map_err(io)?;
    writeln!(
        out,
        "Every step still goes through permission rules and grants; trusting lets the agent \
         run these steps without planning them again."
    )
    .map_err(io)?;
    if !confirm("Trust this exact version?")? {
        return Ok(false);
    }
    list.trust(name, &text);
    list.save(trust_file)
        .map_err(|e| format!("cannot write {}: {e}", trust_file.display()))?;
    writeln!(
        out,
        "Trusted {name} (sha256 {}). Editing the file revokes this.",
        hame_session::artifact::sha256_hex(text.as_bytes())
    )
    .map_err(io)?;
    Ok(true)
}

fn untrust(trust_file: &Path, name: &str, out: &mut impl Write) -> Result<(), String> {
    let mut list = TrustList::load(trust_file);
    if list.trusted.remove(name).is_none() {
        writeln!(out, "{name} was not trusted.").map_err(io)?;
        return Ok(());
    }
    list.save(trust_file)
        .map_err(|e| format!("cannot write {}: {e}", trust_file.display()))?;
    writeln!(out, "{name} is no longer trusted.").map_err(io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOW: &str = r#"
name = "greet"
version = 2
description = "Write a greeting file"
provenance = "written by hand"
tested_with = ["hame write tool"]
[params.who]
pattern = "[A-Za-z ]+"
[[steps]]
tool = "write"
input = { path = "greeting.txt", content = "Hello {{who}}" }
note = "write the greeting"
[[post]]
kind = "file_contains"
path = "greeting.txt"
text = "Hello {{who}}"
"#;

    fn setup() -> (tempfile::TempDir, Vec<PathBuf>, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("project");
        let dirs = workflow_dirs(&home, &cwd);
        std::fs::create_dir_all(&dirs[0]).unwrap();
        std::fs::write(dirs[0].join("greet.toml"), FLOW).unwrap();
        std::fs::create_dir_all(&dirs[1]).unwrap();
        std::fs::write(dirs[1].join("broken.toml"), "name = 1").unwrap();
        let trust_file = home.join(TRUST_FILE);
        (dir, dirs, trust_file)
    }

    fn text(out: Vec<u8>) -> String {
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn trust_records_the_exact_version_and_an_edit_revokes_it() {
        let (_dir, dirs, trust_file) = setup();

        let mut out = Vec::new();
        list_workflows(&dirs, &trust_file, &mut out).unwrap();
        let listed = text(out);
        assert!(
            listed.contains("greet\tuntrusted\tWrite a greeting file"),
            "{listed}"
        );
        assert!(listed.contains("broken\tuntrusted\t(invalid:"), "{listed}");

        // Declining changes nothing.
        let mut out = Vec::new();
        assert!(!trust(&dirs, &trust_file, "greet", &mut out, |_| Ok(false)).unwrap());
        assert!(!trust_file.exists());
        let shown = text(out);
        assert!(shown.contains("step 1: write"), "{shown}");
        assert!(
            shown.contains("after: file greeting.txt contains"),
            "{shown}"
        );
        assert!(
            shown.contains("param who (required) matching [A-Za-z ]+"),
            "{shown}"
        );

        let mut out = Vec::new();
        assert!(trust(&dirs, &trust_file, "greet", &mut out, |_| Ok(true)).unwrap());
        assert!(TrustList::load(&trust_file).is_trusted("greet", FLOW));
        let mut out = Vec::new();
        list_workflows(&dirs, &trust_file, &mut out).unwrap();
        assert!(text(out).contains("greet\ttrusted\t"));

        // The tool sees the same: edited means untrusted again.
        std::fs::write(dirs[0].join("greet.toml"), FLOW.replace("Hello", "Hi")).unwrap();
        let mut out = Vec::new();
        show(&dirs, &trust_file, "greet", &mut out).unwrap();
        assert!(text(out).contains("not trusted in this version"));

        let mut out = Vec::new();
        untrust(&trust_file, "greet", &mut out).unwrap();
        assert!(TrustList::load(&trust_file).trusted.is_empty());
    }

    #[test]
    fn invalid_missing_and_unsafe_names_are_refused() {
        let (_dir, dirs, trust_file) = setup();
        let mut out = Vec::new();
        let invalid = trust(&dirs, &trust_file, "broken", &mut out, |_| Ok(true));
        assert!(invalid.unwrap_err().contains("not a valid workflow"));
        assert!(trust(&dirs, &trust_file, "nope", &mut out, |_| Ok(true)).is_err());
        assert!(trust(&dirs, &trust_file, "../greet", &mut out, |_| Ok(true)).is_err());
        assert!(!trust_file.exists());
    }
}
