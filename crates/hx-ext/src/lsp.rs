use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hx_kernel::BoxFuture;
use hx_sandbox::{SandboxPolicy, wrap};
use hx_session::ToolSpec;
use hx_tools::{Effect, GroupGuard, Tool, ToolContext, ToolOutput};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Notify, OnceCell, mpsc, oneshot};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
const INHERITED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "TERM",
    "TMPDIR",
    "CARGO_HOME",
    "RUSTUP_HOME",
];

/// How to start a language server and which files it handles.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LspServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// File extensions, without the dot, this server checks.
    pub extensions: Vec<String>,
    /// Run the server under the same confinement as the agent's tools.
    #[serde(default = "yes")]
    pub sandbox: bool,
    /// Longest time to wait for a file's diagnostics. Default 30.
    pub timeout_secs: Option<u64>,
    /// How long the server must stay quiet before its report counts as
    /// final, in milliseconds. Default 1500.
    pub settle_ms: Option<u64>,
}

fn yes() -> bool {
    true
}

fn language_id(extension: &str) -> &str {
    match extension {
        "rs" => "rust",
        "py" => "python",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "go" => "go",
        "rb" => "ruby",
        other => other,
    }
}

fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                uri.push(byte as char);
            }
            other => uri.push_str(&format!("%{other:02X}")),
        }
    }
    uri
}

/// What the server last said about one file, and how many times it has
/// reported on it.
#[derive(Default, Clone)]
struct Report {
    count: u64,
    diagnostics: Vec<Value>,
}

type Waiting = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

/// A running language server.
struct LspClient {
    outbox: mpsc::UnboundedSender<Value>,
    waiting: Waiting,
    next_id: AtomicU64,
    reports: Arc<Mutex<HashMap<String, Report>>>,
    reported: Arc<Notify>,
    /// Version last sent for each open document.
    open: Mutex<HashMap<String, i64>>,
    _group: GroupGuard,
}

impl LspClient {
    async fn start(
        config: &LspServerConfig,
        policy: Option<&SandboxPolicy>,
        helper: &Path,
        root: &Path,
    ) -> Result<Self, String> {
        let args: Vec<OsString> = config.args.iter().map(OsString::from).collect();
        let (program, args) = match policy.filter(|_| config.sandbox) {
            Some(policy) => {
                let wrapped = wrap(policy, helper, Path::new(&config.command), &args)
                    .map_err(|e| e.to_string())?;
                (wrapped.program, wrapped.args)
            }
            None => (PathBuf::from(&config.command), args),
        };
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .current_dir(root)
            .env_clear()
            .envs(
                std::env::vars_os().filter(|(key, _)| {
                    key.to_str().is_some_and(|key| INHERITED_ENV.contains(&key))
                }),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot start language server {}: {e}", config.command))?;
        let group = GroupGuard::new(child.id());
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");

        let (outbox, mut outgoing) = mpsc::unbounded_channel::<Value>();
        tokio::spawn(async move {
            while let Some(message) = outgoing.recv().await {
                let body = message.to_string();
                let frame = format!("Content-Length: {}\r\n\r\n{body}", body.len());
                if stdin.write_all(frame.as_bytes()).await.is_err() || stdin.flush().await.is_err()
                {
                    break;
                }
            }
        });

        let waiting: Waiting = Arc::default();
        let reports: Arc<Mutex<HashMap<String, Report>>> = Arc::default();
        let reported = Arc::new(Notify::new());
        let (r_waiting, r_reports, r_reported, replies) = (
            Arc::clone(&waiting),
            Arc::clone(&reports),
            Arc::clone(&reported),
            outbox.clone(),
        );
        tokio::spawn(async move {
            let _child = child;
            let mut reader = BufReader::new(stdout);
            loop {
                // Headers, then a body of the announced length.
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line).await {
                        Ok(0) | Err(_) => {
                            r_waiting.lock().expect("lsp lock").clear();
                            return;
                        }
                        Ok(_) => {}
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                if reader.read_exact(&mut body).await.is_err() {
                    r_waiting.lock().expect("lsp lock").clear();
                    return;
                }
                let Ok(message) = serde_json::from_slice::<Value>(&body) else {
                    continue;
                };
                match (message["method"].as_str(), message.get("id")) {
                    (Some("textDocument/publishDiagnostics"), _) => {
                        if let Some(uri) = message["params"]["uri"].as_str() {
                            let mut reports = r_reports.lock().expect("lsp lock");
                            let report = reports.entry(uri.to_string()).or_default();
                            report.count += 1;
                            report.diagnostics = message["params"]["diagnostics"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default();
                        }
                        r_reported.notify_waiters();
                    }
                    // Requests from the server: answer so it does not stall.
                    (Some(method), Some(id)) => {
                        let result = match method {
                            // One (empty) settings object per item asked for.
                            "workspace/configuration" => Value::Array(vec![
                                Value::Null;
                                message["params"]["items"]
                                    .as_array()
                                    .map_or(0, Vec::len)
                                    .min(64)
                            ]),
                            _ => Value::Null,
                        };
                        let _ = replies.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
                    }
                    (None, Some(id)) => {
                        let waiter = id
                            .as_u64()
                            .and_then(|id| r_waiting.lock().expect("lsp lock").remove(&id));
                        if let Some(waiter) = waiter {
                            let _ = waiter.send(message);
                        }
                    }
                    _ => {}
                }
            }
        });

        let client = Self {
            outbox,
            waiting,
            next_id: AtomicU64::new(1),
            reports,
            reported,
            open: Mutex::new(HashMap::new()),
            _group: group,
        };
        let root_uri = file_uri(root);
        client
            .request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "rootUri": root_uri,
                    "workspaceFolders": [{"uri": root_uri, "name": "workspace"}],
                    "capabilities": {
                        "textDocument": {
                            "publishDiagnostics": {"relatedInformation": false},
                            "synchronization": {"didSave": true},
                        },
                        "workspace": {"configuration": true, "workspaceFolders": true},
                    },
                    "clientInfo": {"name": "hx", "version": env!("CARGO_PKG_VERSION")},
                }),
            )
            .await?;
        client.notify("initialized", json!({}));
        Ok(client)
    }

    fn notify(&self, method: &str, params: Value) {
        let _ = self
            .outbox
            .send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = oneshot::channel();
        self.waiting.lock().expect("lsp lock").insert(id, answer);
        self.outbox
            .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .map_err(|_| "the language server stopped".to_string())?;
        match tokio::time::timeout(STARTUP_TIMEOUT, answered).await {
            Ok(Ok(response)) => match response.get("error") {
                Some(error) => Err(format!("the language server refused {method}: {error}")),
                None => Ok(response["result"].clone()),
            },
            Ok(Err(_)) => Err("the language server stopped".into()),
            Err(_) => Err(format!("the language server did not answer {method}")),
        }
    }

    /// Sends the file's current content and waits for the server's verdict
    /// on it. `None` means it reported nothing in time.
    async fn diagnose(
        &self,
        path: &Path,
        text: &str,
        limit: Duration,
        settle: Duration,
    ) -> Option<Vec<Value>> {
        let uri = file_uri(path);
        let before = self
            .reports
            .lock()
            .expect("lsp lock")
            .get(&uri)
            .map_or(0, |report| report.count);
        let version = {
            let mut open = self.open.lock().expect("lsp lock");
            let version = open.entry(uri.clone()).or_insert(0);
            *version += 1;
            *version
        };
        if version == 1 {
            let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            self.notify(
                "textDocument/didOpen",
                json!({"textDocument": {"uri": uri, "languageId": language_id(extension),
                    "version": version, "text": text}}),
            );
        } else {
            self.notify(
                "textDocument/didChange",
                json!({"textDocument": {"uri": uri, "version": version},
                    "contentChanges": [{"text": text}]}),
            );
        }
        self.notify(
            "textDocument/didSave",
            json!({"textDocument": {"uri": uri}, "text": text}),
        );

        let latest = || {
            self.reports
                .lock()
                .expect("lsp lock")
                .get(&uri)
                .filter(|report| report.count > before)
                .cloned()
        };
        let deadline = tokio::time::Instant::now() + limit;
        let mut seen: Option<Report> = None;
        loop {
            // Register for the next report before looking, so one that
            // lands in between is not missed.
            let next = self.reported.notified();
            let current = latest();
            let fresh = match (&current, &seen) {
                (Some(current), Some(seen)) => current.count > seen.count,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if fresh {
                seen = current;
            }
            // After a first report, a quiet period means the server is
            // done; servers often send an early report and then a fuller one.
            let wait = match &seen {
                Some(_) if !fresh => return seen.map(|report| report.diagnostics),
                Some(_) => settle,
                None => deadline.saturating_duration_since(tokio::time::Instant::now()),
            };
            if wait.is_zero() || tokio::time::Instant::now() >= deadline {
                return seen.map(|report| report.diagnostics);
            }
            let _ = tokio::time::timeout(wait, next).await;
        }
    }
}

fn render(path: &Path, root: &Path, diagnostics: &[Value]) -> String {
    let shown = path.strip_prefix(root).unwrap_or(path).display();
    let mut lines: Vec<(u64, String)> = diagnostics
        .iter()
        .map(|diagnostic| {
            let start = &diagnostic["range"]["start"];
            let line = start["line"].as_u64().unwrap_or(0) + 1;
            let column = start["character"].as_u64().unwrap_or(0) + 1;
            let severity = match diagnostic["severity"].as_u64() {
                Some(1) | None => "error",
                Some(2) => "warning",
                Some(3) => "info",
                _ => "hint",
            };
            let message = diagnostic["message"]
                .as_str()
                .unwrap_or("")
                .replace('\n', " ");
            (
                line,
                format!("{shown}:{line}:{column}: {severity}: {message}"),
            )
        })
        .collect();
    lines.sort();
    lines
        .into_iter()
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

struct Server {
    config: LspServerConfig,
    client: OnceCell<Result<LspClient, String>>,
}

/// Asks the project's language servers what is wrong with a file.
///
/// Servers start the first time a file of theirs is checked and stay up
/// for the rest of the session, so later checks are quick.
pub struct DiagnosticsTool {
    servers: Vec<Server>,
    policy: Option<SandboxPolicy>,
    helper: PathBuf,
    root: PathBuf,
}

impl DiagnosticsTool {
    pub fn new(
        servers: &BTreeMap<String, LspServerConfig>,
        policy: Option<SandboxPolicy>,
        helper: PathBuf,
        root: PathBuf,
    ) -> Self {
        Self {
            servers: servers
                .values()
                .map(|config| Server {
                    config: config.clone(),
                    client: OnceCell::new(),
                })
                .collect(),
            policy,
            helper,
            root,
        }
    }
}

impl Tool for DiagnosticsTool {
    fn spec(&self) -> ToolSpec {
        let extensions: Vec<&str> = self
            .servers
            .iter()
            .flat_map(|server| server.config.extensions.iter().map(String::as_str))
            .collect();
        ToolSpec {
            name: "diagnostics".into(),
            description: format!(
                "Get the compiler or language-server errors and warnings for a file, as \
                 path:line:column: severity: message. Use it after editing a file to check \
                 the change before moving on. Available for these file types: {}.",
                extensions.join(", ")
            ),
            input_schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string", "description": "The file to check."}},
                "required": ["path"],
                "additionalProperties": false
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    fn host_side(&self) -> bool {
        true
    }

    /// One conversation per server: checks go one at a time.
    fn concurrent(&self) -> bool {
        false
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let Some(path) = input["path"].as_str() else {
                return ToolOutput::error("Invalid input: path is required.");
            };
            let path = ctx.resolve(path);
            let path = path.canonicalize().unwrap_or(path);
            let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let Some(server) = self
                .servers
                .iter()
                .find(|server| server.config.extensions.iter().any(|e| e == extension))
            else {
                return ToolOutput::error(format!(
                    "No language server is configured for .{extension} files."
                ));
            };
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(e) => return ToolOutput::error(format!("Cannot read {}: {e}", path.display())),
            };
            let client = server
                .client
                .get_or_init(|| {
                    LspClient::start(
                        &server.config,
                        self.policy.as_ref(),
                        &self.helper,
                        &self.root,
                    )
                })
                .await;
            let client = match client {
                Ok(client) => client,
                Err(why) => return ToolOutput::error(why.clone()),
            };
            let limit = Duration::from_secs(server.config.timeout_secs.unwrap_or(30));
            let settle = Duration::from_millis(server.config.settle_ms.unwrap_or(1500));
            match client.diagnose(&path, &text, limit, settle).await {
                None => ToolOutput::error(format!(
                    "The language server reported nothing about this file within {} s. It may \
                     still be starting; try again, or check with the project's build command.",
                    limit.as_secs()
                )),
                Some(diagnostics) if diagnostics.is_empty() => ToolOutput::ok("No problems found."),
                Some(diagnostics) => ToolOutput::ok(render(&path, &self.root, &diagnostics)),
            }
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A language server that reports every line containing "BUG", first
    /// with an empty report (as real servers do), then the real one.
    const SERVER: &str = r#"
import json, sys
def send(m):
    b = json.dumps(m).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(b) + b); sys.stdout.buffer.flush()
def read():
    n = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        line = line.strip()
        if not line: break
        if line.lower().startswith(b"content-length:"): n = int(line.split(b":")[1])
    return json.loads(sys.stdin.buffer.read(n))
def publish(uri, text):
    send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": []}})
    found = [{"range": {"start": {"line": i, "character": l.index("BUG")}, "end": {"line": i, "character": 3}},
              "severity": 1 if "fatal" in l else 2, "message": "found a bug\nhere"}
             for i, l in enumerate(text.split("\n")) if "BUG" in l]
    send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": found}})
while True:
    m = read()
    if m is None: break
    method = m.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": 99, "method": "workspace/configuration", "params": {"items": [{}, {}]}})
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"capabilities": {}}})
    elif method == "textDocument/didOpen":
        publish(m["params"]["textDocument"]["uri"], m["params"]["textDocument"]["text"])
    elif method == "textDocument/didChange":
        publish(m["params"]["textDocument"]["uri"], m["params"]["contentChanges"][0]["text"])
"#;

    fn tool(dir: &Path) -> DiagnosticsTool {
        let script = dir.join("server.py");
        std::fs::write(&script, SERVER).unwrap();
        let mut servers = BTreeMap::new();
        servers.insert(
            "fake".to_string(),
            LspServerConfig {
                command: "python3".into(),
                args: vec![script.display().to_string()],
                extensions: vec!["txt".into()],
                sandbox: false,
                timeout_secs: Some(10),
                settle_ms: Some(200),
            },
        );
        DiagnosticsTool::new(&servers, None, PathBuf::from("hx"), dir.to_path_buf())
    }

    #[tokio::test]
    async fn diagnostics_follow_the_file_as_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let tool = tool(&root);
        let ctx = ToolContext::new(&root);
        assert!(tool.spec().description.ends_with("file types: txt."));

        std::fs::write(root.join("a b.txt"), "fine\n  fatal BUG\nok\nminor BUG\n").unwrap();
        let first = tool.call(&json!({"path": "a b.txt"}), &ctx).await;
        assert_eq!(
            first,
            ToolOutput::ok(
                "a b.txt:2:9: error: found a bug here\na b.txt:4:7: warning: found a bug here"
            )
        );

        // After a fix, the same running server reports the new state.
        std::fs::write(root.join("a b.txt"), "fine\nfixed\n").unwrap();
        let fixed = tool.call(&json!({"path": "a b.txt"}), &ctx).await;
        assert_eq!(fixed, ToolOutput::ok("No problems found."));

        let unknown = tool.call(&json!({"path": "x.zig"}), &ctx).await;
        assert!(unknown.is_error && unknown.text.contains("No language server"));
        let missing = tool.call(&json!({"path": "none.txt"}), &ctx).await;
        assert!(missing.is_error && missing.text.contains("Cannot read"));
    }

    #[test]
    fn uris_escape_what_paths_may_contain() {
        assert_eq!(file_uri(Path::new("/a b/ç.rs")), "file:///a%20b/%C3%A7.rs");
    }
}
