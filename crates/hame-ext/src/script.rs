//! The `script` tool: several tool calls chained in a short program.
//!
//! Tools reached through `tool_call` cost a model request each, and every
//! intermediate result goes through the model's context on its way to the
//! next call. A script calls them as functions instead: the results stay
//! in the script, and only what it prints comes back.
//!
//! The program is Python, run in the same confinement as the shell tool.
//! It cannot reach the tools itself: each `call` is a line sent to the
//! harness, which runs the tool through the same permission layers and
//! hooks as a call the model made directly.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use hame_agent::{ToolPipeline, ToolRequest};
use hame_kernel::BoxFuture;
use hame_sandbox::{SandboxPolicy, wrap};
use hame_session::ToolSpec;
use hame_tools::{Effect, Tool, ToolContext, ToolOutput, ToolSet};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub const SCRIPT_TOOL: &str = "script";

/// A script may run this long, calls included.
const TIME_LIMIT: Duration = Duration::from_secs(300);
/// And make this many calls.
const MAX_CALLS: usize = 100;

/// Runs inside the confined process. It reads the program from its first
/// line of input, keeps the real standard output for talking to the
/// harness, and collects what the program prints.
const RUNNER: &str = r#"
import sys, json, io, traceback
_in, _out = sys.stdin, sys.stdout
_code = json.loads(_in.readline())
_printed = io.StringIO()
sys.stdout = _printed
def call(name, **arguments):
    _out.write(json.dumps({"call": name, "arguments": arguments}) + "\n")
    _out.flush()
    reply = json.loads(_in.readline())
    if reply["error"]:
        raise RuntimeError(reply["text"])
    try:
        return json.loads(reply["text"])
    except ValueError:
        return reply["text"]
_error = None
try:
    exec(compile(_code, "script", "exec"), {"call": call, "json": json, "__name__": "__main__"})
except BaseException:
    _error = traceback.format_exc()
_out.write(json.dumps({"done": _printed.getvalue(), "error": _error}) + "\n")
_out.flush()
"#;

pub struct ScriptTool {
    /// The tools a script may call: the ones not declared to the model.
    pub tools: ToolSet,
    /// Every call a script makes goes through this, like any other call.
    pub pipeline: Arc<ToolPipeline>,
    /// How the program is confined; `None` runs it unconfined.
    pub policy: Option<SandboxPolicy>,
    /// The `hame` executable, which the Linux sandbox re-executes.
    pub helper: PathBuf,
}

impl ScriptTool {
    async fn run(&self, code: &str, ctx: &ToolContext) -> Result<String, String> {
        let args = vec![
            OsString::from("-I"),
            OsString::from("-c"),
            OsString::from(RUNNER),
        ];
        // Asked rather than assumed: Windows has a `python3` that starts and
        // runs nothing, and under the sandbox wrapper every name "starts".
        let python = hame_kernel::python_program()
            .ok_or("Scripts need Python, and neither python3 nor python runs here.")?;
        let (program, args) = match &self.policy {
            Some(policy) => {
                let wrapped = wrap(policy, &self.helper, std::path::Path::new(python), &args)
                    .map_err(|e| e.to_string())?;
                (wrapped.program, wrapped.args)
            }
            None => (python.into(), args),
        };
        let mut child = tokio::process::Command::new(program)
            .args(args)
            .current_dir(&ctx.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("Cannot start Python for the script: {e}"))?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let mut lines = BufReader::new(child.stdout.take().expect("stdout is piped")).lines();
        let send = |value: Value| format!("{value}\n");
        stdin
            .write_all(send(json!(code)).as_bytes())
            .await
            .map_err(|e| format!("Cannot start the script: {e}"))?;

        let mut calls = 0;
        loop {
            let line =
                lines.next_line().await.ok().flatten().ok_or(
                    "The script ended without finishing; Python may have failed to start it.",
                )?;
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(printed) = message["done"].as_str() {
                return match message["error"].as_str() {
                    None => Ok(printed.to_string()),
                    Some(error) => Err(format!("{printed}\n{error}").trim().to_string()),
                };
            }
            let name = message["call"].as_str().unwrap_or("");
            calls += 1;
            let output = if calls > MAX_CALLS {
                ToolOutput::error(format!("A script may make at most {MAX_CALLS} calls."))
            } else {
                match self.tools.get(name) {
                    Some(tool) => {
                        let request = ToolRequest {
                            call_id: format!("script-{calls}"),
                            name: name.to_string(),
                            input: message["arguments"].clone(),
                            effect: tool.effect(),
                        };
                        self.pipeline
                            .run(request, |request: ToolRequest| {
                                let tool = Arc::clone(&tool);
                                async move { tool.call(&request.input, ctx).await }
                            })
                            .await
                    }
                    None => ToolOutput::error(format!(
                        "There is no tool named {name:?} for scripts. Find the exact name \
                         with tool_search."
                    )),
                }
            };
            let reply = send(json!({"text": output.text, "error": output.is_error}));
            if stdin.write_all(reply.as_bytes()).await.is_err() {
                return Err("The script stopped while a call was being answered.".into());
            }
        }
    }
}

impl Tool for ScriptTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: SCRIPT_TOOL.into(),
            description: "Run a short Python program that calls the tools found with \
                          tool_search, so that several calls and the work between them take \
                          one step. In the program, call(name, **arguments) runs a tool and \
                          returns its result, parsed when it is JSON; an error from the tool \
                          raises RuntimeError. Only what the program prints comes back to you, \
                          so print the answer, not the intermediate data. Use it when you \
                          need many calls, or need to filter or combine large results; for \
                          one call, use tool_call."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "code": {"type": "string", "description": "The Python program."}
                },
                "required": ["code"],
                "additionalProperties": false
            }),
        }
    }

    // What a script does depends on the tools it calls, each of which is
    // ruled on by itself.
    fn effect(&self) -> Effect {
        Effect::Execute
    }

    fn host_side(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let Some(code) = input["code"]
                .as_str()
                .filter(|code| !code.trim().is_empty())
            else {
                return ToolOutput::error("Invalid input: code is required.");
            };
            match tokio::time::timeout(TIME_LIMIT, self.run(code, ctx)).await {
                Ok(Ok(printed)) if printed.trim().is_empty() => {
                    ToolOutput::ok("(the script printed nothing; print what you want to see)")
                }
                Ok(Ok(printed)) => ToolOutput::ok(printed),
                Ok(Err(problem)) => ToolOutput::error(problem),
                Err(_) => ToolOutput::error(format!(
                    "The script was stopped after {} seconds.",
                    TIME_LIMIT.as_secs()
                )),
            }
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use hame_kernel::{Layer, Next};

    /// A tool that looks a price up, standing in for an MCP tool.
    struct Price;

    impl Tool for Price {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "mcp__shop__price".into(),
                description: "Price of an item.".into(),
                input_schema: json!({"type": "object"}),
            }
        }
        fn effect(&self) -> Effect {
            Effect::ReadOnly
        }
        fn host_side(&self) -> bool {
            true
        }
        fn call<'a>(
            &'a self,
            input: &'a Value,
            _ctx: &'a ToolContext,
        ) -> BoxFuture<'a, ToolOutput> {
            Box::pin(async move {
                match input["item"].as_str() {
                    Some("pen") => ToolOutput::ok(r#"{"item": "pen", "price": 3}"#),
                    Some("ink") => ToolOutput::ok(r#"{"item": "ink", "price": 39}"#),
                    _ => ToolOutput::error("no such item"),
                }
            })
        }
    }

    /// Refuses one item, the way a permission rule would.
    struct NoInk;

    impl Layer<ToolRequest, ToolOutput> for NoInk {
        fn call<'a>(
            &'a self,
            request: ToolRequest,
            next: Next<'a, ToolRequest, ToolOutput>,
        ) -> BoxFuture<'a, ToolOutput> {
            Box::pin(async move {
                if request.name == "mcp__shop__price" && request.input["item"] == "ink" {
                    ToolOutput::error("Denied by the user's permission rules.")
                } else {
                    next.run(request).await
                }
            })
        }
    }

    fn tool(pipeline: Arc<ToolPipeline>) -> ScriptTool {
        let mut tools = ToolSet::new();
        tools.insert(Arc::new(Price));
        ScriptTool {
            tools,
            pipeline,
            policy: None,
            helper: PathBuf::from("hame"),
        }
    }

    async fn run(script: &ScriptTool, code: &str) -> ToolOutput {
        let dir = tempfile::tempdir().unwrap();
        script
            .call(&json!({ "code": code }), &ToolContext::new(dir.path()))
            .await
    }

    #[tokio::test]
    async fn a_script_chains_calls_and_returns_only_what_it_prints() {
        let script = tool(Arc::new(ToolPipeline::new()));
        let out = run(
            &script,
            "total = sum(call('mcp__shop__price', item=i)['price'] for i in ['pen', 'ink'])\nprint('total', total)",
        )
        .await;
        assert_eq!(out, ToolOutput::ok("total 42\n"));

        // A tool's error is an exception the program can handle or not.
        let handled = run(
            &script,
            "try:\n    call('mcp__shop__price', item='gold')\nexcept RuntimeError as e:\n    print('failed:', e)",
        )
        .await;
        assert_eq!(handled, ToolOutput::ok("failed: no such item\n"));
        let unhandled = run(
            &script,
            "print('before')\ncall('mcp__shop__price', item='gold')",
        )
        .await;
        assert!(unhandled.is_error);
        assert!(
            unhandled.text.starts_with("before\n")
                && unhandled.text.contains("RuntimeError: no such item"),
            "{}",
            unhandled.text
        );

        // Only the tools it was given, and nothing printed is said so.
        let unknown = run(&script, "call('bash', command='id')").await;
        assert!(
            unknown.is_error && unknown.text.contains("no tool named"),
            "{}",
            unknown.text
        );
        assert!(run(&script, "x = 1").await.text.contains("printed nothing"));
        assert!(
            run(&script, "def broken(:")
                .await
                .text
                .contains("SyntaxError")
        );
    }

    #[tokio::test]
    async fn calls_made_by_a_script_are_ruled_on_like_any_other() {
        let pipeline = Arc::new(ToolPipeline::new());
        pipeline.push(NoInk);
        let script = tool(pipeline);
        let out = run(
            &script,
            "print(call('mcp__shop__price', item='pen')['price'])\nprint(call('mcp__shop__price', item='ink'))",
        )
        .await;
        assert!(out.is_error);
        assert!(out.text.starts_with("3\n"), "{}", out.text);
        assert!(
            out.text.contains("Denied by the user's permission rules."),
            "{}",
            out.text
        );
    }
}
