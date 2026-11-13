use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use zcode_kernel::BoxFuture;
use zcode_sandbox::{SandboxPolicy, wrap};
use zcode_session::ToolSpec;
use zcode_tools::{Effect, GroupGuard, Tool, ToolContext, ToolOutput};

const PROTOCOL_VERSION: &str = "2025-06-18";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const CALL_TIMEOUT: Duration = Duration::from_secs(300);
/// Environment a server inherits; the rest of the harness's environment,
/// credentials included, is withheld unless the configuration passes it.
const INHERITED_ENV: &[&str] = &["PATH", "HOME", "USER", "LOGNAME", "LANG", "TERM", "TMPDIR"];

/// How to start one Model Context Protocol server (stdio transport).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Run the server under the same confinement as the agent's tools.
    /// Turn off for servers that need wider access than the workspace.
    #[serde(default = "yes")]
    pub sandbox: bool,
}

fn yes() -> bool {
    true
}

type Waiting = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

/// A running server and the JSON-RPC conversation with it.
pub struct McpClient {
    name: String,
    outbox: mpsc::UnboundedSender<String>,
    waiting: Waiting,
    next_id: AtomicU64,
    /// Kills the server and anything it started when the client is dropped.
    _group: GroupGuard,
}

impl McpClient {
    /// Starts the server and completes the protocol handshake.
    ///
    /// `policy`, when given and confined, wraps the server process;
    /// `helper` is the `zcode` executable the Linux sandbox re-executes.
    pub async fn start(
        name: &str,
        config: &McpServerConfig,
        policy: Option<&SandboxPolicy>,
        helper: &Path,
        cwd: &Path,
    ) -> Result<Arc<Self>, String> {
        let args: Vec<OsString> = config.args.iter().map(OsString::from).collect();
        let (program, args) = match policy.filter(|_| config.sandbox) {
            Some(policy) => {
                let wrapped = wrap(policy, helper, Path::new(&config.command), &args)
                    .map_err(|e| e.to_string())?;
                (wrapped.program, wrapped.args)
            }
            None => (config.command.clone().into(), args),
        };
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(
                std::env::vars_os().filter(|(key, _)| {
                    key.to_str().is_some_and(|key| INHERITED_ENV.contains(&key))
                }),
            )
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot start MCP server {name}: {e}"))?;
        let group = GroupGuard::new(child.id());
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");

        let (outbox, mut outgoing) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(line) = outgoing.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    break;
                }
            }
        });

        let waiting: Waiting = Arc::default();
        let (reader_waiting, replies) = (Arc::clone(&waiting), outbox.clone());
        tokio::spawn(async move {
            // Holding the child here keeps it alive as long as it talks.
            let _child = child;
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                match (message.get("method"), message.get("id")) {
                    // A response to one of our requests.
                    (None, Some(id)) => {
                        let waiter = id
                            .as_u64()
                            .and_then(|id| reader_waiting.lock().expect("mcp lock").remove(&id));
                        if let Some(waiter) = waiter {
                            let _ = waiter.send(message);
                        }
                    }
                    // The server asking us for something: nothing is offered.
                    (Some(_), Some(id)) => {
                        let _ = replies.send(
                            json!({"jsonrpc": "2.0", "id": id,
                                "error": {"code": -32601, "message": "not supported"}})
                            .to_string(),
                        );
                    }
                    _ => {}
                }
            }
            // The server is gone: fail everything still waiting.
            reader_waiting.lock().expect("mcp lock").clear();
        });

        let client = Arc::new(Self {
            name: name.to_string(),
            outbox,
            waiting,
            next_id: AtomicU64::new(1),
            _group: group,
        });
        client
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "zcode", "version": env!("CARGO_PKG_VERSION")},
                }),
                STARTUP_TIMEOUT,
            )
            .await?;
        client.notify("notifications/initialized");
        Ok(client)
    }

    fn notify(&self, method: &str) {
        let _ = self
            .outbox
            .send(json!({"jsonrpc": "2.0", "method": method}).to_string());
    }

    async fn request(&self, method: &str, params: Value, limit: Duration) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = oneshot::channel();
        self.waiting.lock().expect("mcp lock").insert(id, answer);
        let gone = || format!("MCP server {} stopped responding", self.name);
        self.outbox
            .send(
                json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string(),
            )
            .map_err(|_| gone())?;
        let response = match tokio::time::timeout(limit, answered).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => return Err(gone()),
            Err(_) => {
                self.waiting.lock().expect("mcp lock").remove(&id);
                return Err(format!(
                    "MCP server {} did not answer {method} within {} s",
                    self.name,
                    limit.as_secs()
                ));
            }
        };
        if let Some(error) = response.get("error") {
            return Err(format!(
                "MCP server {} refused {method}: {}",
                self.name,
                error["message"].as_str().unwrap_or("unknown error")
            ));
        }
        Ok(response["result"].clone())
    }

    /// Every tool the server offers, named `mcp__<server>__<tool>`.
    pub async fn tools(self: &Arc<Self>) -> Result<Vec<Arc<dyn Tool>>, String> {
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let page = self.request("tools/list", params, STARTUP_TIMEOUT).await?;
            for tool in page["tools"].as_array().into_iter().flatten() {
                let Some(remote) = tool["name"].as_str() else {
                    continue;
                };
                tools.push(Arc::new(McpTool {
                    client: Arc::clone(self),
                    remote: remote.to_string(),
                    spec: ToolSpec {
                        name: exposed_name(&self.name, remote),
                        description: tool["description"].as_str().unwrap_or("").to_string(),
                        input_schema: match &tool["inputSchema"] {
                            Value::Object(schema) => Value::Object(schema.clone()),
                            _ => json!({"type": "object"}),
                        },
                    },
                }));
            }
            cursor = page["nextCursor"].as_str().map(String::from);
            if cursor.is_none() {
                return Ok(tools);
            }
        }
    }
}

/// Providers accept only `[A-Za-z0-9_-]{1,64}` as a tool name.
fn exposed_name(server: &str, tool: &str) -> String {
    format!("mcp__{server}__{tool}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

struct McpTool {
    client: Arc<McpClient>,
    remote: String,
    spec: ToolSpec,
}

impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    /// A server can do anything its process can; treat every call like
    /// running a program.
    fn effect(&self) -> Effect {
        Effect::Execute
    }

    fn host_side(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, input: &'a Value, _ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let params = json!({"name": self.remote, "arguments": input});
            let result = match self
                .client
                .request("tools/call", params, CALL_TIMEOUT)
                .await
            {
                Ok(result) => result,
                Err(e) => return ToolOutput::error(e),
            };
            let mut text = String::new();
            for item in result["content"].as_array().into_iter().flatten() {
                match item["type"].as_str() {
                    Some("text") => text.push_str(item["text"].as_str().unwrap_or("")),
                    Some(other) => text.push_str(&format!("[{other} content omitted]")),
                    None => {}
                }
                text.push('\n');
            }
            let text = text.trim_end().to_string();
            ToolOutput {
                text: if text.is_empty() {
                    "(no output)".into()
                } else {
                    text
                },
                is_error: result["isError"].as_bool().unwrap_or(false),
            }
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A minimal MCP server: three tools, paginated listing.
    const SERVER: &str = r#"
import json, sys
def send(m): sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    m = json.loads(line)
    i, method, p = m.get("id"), m.get("method"), m.get("params", {})
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": i, "result": {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "t", "version": "0"}}})
    elif method == "tools/list":
        if "cursor" not in p:
            send({"jsonrpc": "2.0", "id": i, "result": {"tools": [{"name": "echo", "description": "Echo text.", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}], "nextCursor": "2"}})
        else:
            send({"jsonrpc": "2.0", "id": i, "result": {"tools": [{"name": "write.file", "description": "Write a file."}, {"name": "fail"}]}})
    elif method == "tools/call":
        a = p.get("arguments", {})
        if p["name"] == "echo":
            send({"jsonrpc": "2.0", "id": i, "result": {"content": [{"type": "text", "text": "echo: " + a.get("text", "")}, {"type": "image", "data": ""}]}})
        elif p["name"] == "write.file":
            try:
                open(a["path"], "w").write("x")
                send({"jsonrpc": "2.0", "id": i, "result": {"content": [{"type": "text", "text": "written"}]}})
            except Exception as e:
                send({"jsonrpc": "2.0", "id": i, "result": {"content": [{"type": "text", "text": str(e)}], "isError": True}})
        else:
            send({"jsonrpc": "2.0", "id": i, "error": {"code": -32000, "message": "boom"}})
"#;

    fn config(dir: &Path) -> McpServerConfig {
        let script = dir.join("server.py");
        std::fs::write(&script, SERVER).unwrap();
        McpServerConfig {
            command: "python3".into(),
            args: vec![script.display().to_string()],
            env: BTreeMap::new(),
            sandbox: true,
        }
    }

    #[tokio::test]
    async fn tools_are_listed_across_pages_and_called() {
        let dir = tempfile::tempdir().unwrap();
        let client = McpClient::start(
            "demo",
            &config(dir.path()),
            None,
            Path::new("zcode"),
            dir.path(),
        )
        .await
        .unwrap();
        let tools = client.tools().await.unwrap();
        let names: Vec<String> = tools.iter().map(|tool| tool.spec().name).collect();
        assert_eq!(
            names,
            [
                "mcp__demo__echo",
                "mcp__demo__write_file",
                "mcp__demo__fail"
            ]
        );
        assert!(tools.iter().all(|tool| tool.host_side()));
        assert_eq!(tools[2].spec().input_schema, json!({"type": "object"}));

        let ctx = ToolContext::new(dir.path());
        let echoed = tools[0].call(&json!({"text": "hi"}), &ctx).await;
        assert_eq!(echoed, ToolOutput::ok("echo: hi\n[image content omitted]"));
        let failed = tools[2].call(&json!({}), &ctx).await;
        assert!(
            failed.is_error && failed.text.contains("boom"),
            "{}",
            failed.text
        );
    }

    #[tokio::test]
    async fn a_server_that_cannot_start_or_dies_is_an_error_not_a_hang() {
        let dir = tempfile::tempdir().unwrap();
        let missing = McpServerConfig {
            command: "/nonexistent/mcp-server".into(),
            args: vec![],
            env: BTreeMap::new(),
            sandbox: false,
        };
        let error = McpClient::start("gone", &missing, None, Path::new("zcode"), dir.path())
            .await
            .err()
            .unwrap();
        assert!(error.contains("cannot start MCP server gone"), "{error}");

        // Starts, then exits without answering the handshake.
        let silent = McpServerConfig {
            command: "true".into(),
            ..missing
        };
        let error = McpClient::start("silent", &silent, None, Path::new("zcode"), dir.path())
            .await
            .err()
            .unwrap();
        assert!(error.contains("stopped responding"), "{error}");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_confined_server_cannot_write_outside_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let work = root.join("work");
        std::fs::create_dir(&work).unwrap();
        let mut policy = SandboxPolicy::workspace(&work);
        policy.writable.truncate(1);
        policy.unreadable.clear();

        let client = McpClient::start(
            "demo",
            &config(&root),
            Some(&policy),
            Path::new("zcode"),
            &work,
        )
        .await
        .unwrap();
        let tools = client.tools().await.unwrap();
        let ctx = ToolContext::new(&work);
        let inside = tools[1].call(&json!({"path": "in.txt"}), &ctx).await;
        assert_eq!(inside, ToolOutput::ok("written"));
        let outside = tools[1]
            .call(
                &json!({"path": root.join("out.txt").display().to_string()}),
                &ctx,
            )
            .await;
        assert!(outside.is_error, "{}", outside.text);
        assert!(!root.join("out.txt").exists());
    }
}
