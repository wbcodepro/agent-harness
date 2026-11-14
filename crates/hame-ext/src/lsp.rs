use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hame_kernel::BoxFuture;
use hame_sandbox::{Mode, SandboxPolicy, wrap};
use hame_session::ToolSpec;
use hame_tools::{Effect, GroupGuard, Tool, ToolContext, ToolOutput, write_atomic};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Notify, OnceCell, mpsc, oneshot};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
/// Most lines `references` returns.
const MAX_REFERENCES: usize = 100;
const MAX_LINE_CHARS: usize = 200;
/// Ends every refusal of a symbol request, so the model knows what to do
/// instead.
const FALLBACK: &str = "Fall back to grep to find the uses and edit to change them.";

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
    encode_uri(&path.to_string_lossy(), cfg!(windows))
}

fn encode_uri(path: &str, windows: bool) -> String {
    // `C:\a\b` is written `/C:/a/b` in a URI.
    let path = if windows {
        format!("/{}", path.replace('\\', "/"))
    } else {
        path.to_string()
    };
    let mut uri = String::from("file://");
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                uri.push(byte as char);
            }
            other => uri.push_str(&format!("%{other:02X}")),
        }
    }
    uri
}

/// The file a URI from the server names. `None` when it is not a local file.
fn uri_path(uri: &str) -> Option<PathBuf> {
    decode_uri(uri, cfg!(windows)).map(PathBuf::from)
}

fn decode_uri(uri: &str, windows: bool) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let mut bytes = Vec::with_capacity(rest.len());
    let mut raw = rest.bytes();
    while let Some(byte) = raw.next() {
        if byte == b'%' {
            let hex = [raw.next()?, raw.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    if !path.starts_with('/') {
        return None;
    }
    if windows {
        let path = match path.as_bytes() {
            [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => &path[1..],
            _ => &path,
        };
        return Some(path.replace('/', "\\"));
    }
    Some(path)
}

/// Where each line of `text` starts and ends, without its line ending.
/// Lines end the way the protocol says: `\n`, `\r\n` or `\r`.
fn line_spans(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let (mut start, mut at) = (0, 0);
    while at < bytes.len() {
        let ending = match bytes[at] {
            b'\r' if bytes.get(at + 1) == Some(&b'\n') => 2,
            b'\r' | b'\n' => 1,
            _ => 0,
        };
        if ending > 0 {
            spans.push((start, at));
            start = at + ending;
        }
        at += ending.max(1);
    }
    spans.push((start, bytes.len()));
    spans
}

/// The column of `symbol` on a 1-based line, in the UTF-16 code units the
/// protocol counts in. The first occurrence that stands alone as a word
/// wins over one inside a longer name.
fn column(text: &str, line: u64, symbol: &str) -> Result<u64, String> {
    let spans = line_spans(text);
    let span = usize::try_from(line)
        .ok()
        .and_then(|line| line.checked_sub(1))
        .and_then(|line| spans.get(line));
    let Some(&(start, end)) = span else {
        return Err(format!(
            "there is no line {line}; the file has {} lines.",
            spans.len()
        ));
    };
    let content = &text[start..end];
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let alone = |at: usize| {
        !content[..at].chars().next_back().is_some_and(word)
            && !content[at + symbol.len()..]
                .chars()
                .next()
                .is_some_and(word)
    };
    let found = || content.match_indices(symbol).map(|(at, _)| at);
    let at = found().find(|&at| alone(at)).or_else(|| found().next());
    let Some(at) = at.filter(|_| !symbol.is_empty()) else {
        return Err(format!(
            "line {line} does not contain `{symbol}`. It reads: {}",
            content.trim()
        ));
    };
    Ok(content[..at].encode_utf16().count() as u64)
}

/// The byte offset of a protocol position, which counts UTF-16 code units
/// within a line. A column past the end of its line means the line's end.
fn offset(text: &str, spans: &[(usize, usize)], position: &Value) -> Result<usize, String> {
    let (Some(line), Some(character)) = (position["line"].as_u64(), position["character"].as_u64())
    else {
        return Err("a position is malformed".into());
    };
    let span = usize::try_from(line).ok().and_then(|line| spans.get(line));
    let Some(&(start, end)) = span else {
        return Err(format!(
            "line {} is past the end of the file",
            line.saturating_add(1)
        ));
    };
    let mut units = 0;
    for (at, c) in text[start..end].char_indices() {
        if units >= character {
            return Ok(start + at);
        }
        units += c.len_utf16() as u64;
    }
    Ok(end)
}

/// `text` with the server's edits made, and how many there were. Every
/// range refers to the text as it was, so the edits go in from the end
/// backwards: none moves the place another one points at.
#[cfg(test)]
fn apply_edits(text: &str, edits: &[Value]) -> Result<(String, usize), String> {
    apply_edits_replacing(text, edits, None)
}

/// As [`apply_edits`], and when `replaced` is given, every edit that
/// replaces text must replace exactly that text. A rename replaces the old
/// name wherever it stands; a range holding anything else was worked out
/// against an older copy of the file, and applying it would overwrite
/// whatever is there now.
fn apply_edits_replacing(
    text: &str,
    edits: &[Value],
    replaced: Option<&str>,
) -> Result<(String, usize), String> {
    let spans = line_spans(text);
    let mut ranges = Vec::with_capacity(edits.len());
    for edit in edits {
        let new_text = edit["newText"].as_str().ok_or("an edit has no text")?;
        let start = offset(text, &spans, &edit["range"]["start"])?;
        let end = offset(text, &spans, &edit["range"]["end"])?;
        if end < start {
            return Err("an edit ends before it starts".into());
        }
        if let Some(expected) = replaced
            && end > start
            && &text[start..end] != expected
        {
            return Err(format!(
                "an edit would replace {:?} where {expected:?} was expected",
                text[start..end].chars().take(40).collect::<String>()
            ));
        }
        ranges.push((start, end, new_text));
    }
    // Stable, so insertions at one place keep the order they came in.
    ranges.sort_by_key(|&(start, end, _)| (start, end));
    // Some servers report the same edit twice.
    ranges.dedup();
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err("two edits overlap".into());
    }
    let mut changed = text.to_string();
    for &(start, end, new_text) in ranges.iter().rev() {
        changed.replace_range(start..end, new_text);
    }
    Ok((changed, ranges.len()))
}

/// The text edits of a `WorkspaceEdit`, per document, in either of the two
/// shapes servers send.
fn document_edits(edit: &Value) -> Result<Vec<(String, Vec<Value>)>, String> {
    let malformed = || "The language server's answer is malformed.".to_string();
    if let Some(steps) = edit["documentChanges"].as_array() {
        return steps
            .iter()
            .map(|step| {
                if let Some(kind) = step["kind"].as_str() {
                    return Err(format!(
                        "The rename includes a file operation ({kind}), which this tool does \
                         not do."
                    ));
                }
                match (
                    step["textDocument"]["uri"].as_str(),
                    step["edits"].as_array(),
                ) {
                    (Some(uri), Some(edits)) => Ok((uri.to_string(), edits.clone())),
                    _ => Err(malformed()),
                }
            })
            .collect();
    }
    let Some(changes) = edit["changes"].as_object() else {
        return Ok(Vec::new());
    };
    changes
        .iter()
        .map(|(uri, edits)| {
            let edits = edits.as_array().ok_or_else(malformed)?;
            Ok((uri.clone(), edits.clone()))
        })
        .collect()
}

/// An open document as the server knows it.
struct Document {
    version: i64,
    path: PathBuf,
    text: String,
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
    /// What the server said at startup that it can do.
    capabilities: Value,
    /// What was last sent for each open document.
    open: Mutex<HashMap<String, Document>>,
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
            .envs(hame_sandbox::inherited_environment(&[
                "CARGO_HOME",
                "RUSTUP_HOME",
            ]))
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

        let mut client = Self {
            outbox,
            waiting,
            next_id: AtomicU64::new(1),
            reports,
            reported,
            capabilities: Value::Null,
            open: Mutex::new(HashMap::new()),
            _group: group,
        };
        let root_uri = file_uri(root);
        let answer = client
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
                            "references": {},
                            "rename": {},
                        },
                        "workspace": {
                            "configuration": true,
                            "workspaceFolders": true,
                            "workspaceEdit": {"documentChanges": true},
                        },
                    },
                    "clientInfo": {"name": "hame", "version": env!("CARGO_PKG_VERSION")},
                }),
            )
            .await?;
        client.capabilities = answer["capabilities"].clone();
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

    /// Whether the server announced a capability such as `renameProvider`.
    fn supports(&self, capability: &str) -> bool {
        !matches!(
            self.capabilities[capability],
            Value::Null | Value::Bool(false)
        )
    }

    /// Makes the server's copy of a file match `text`. Unchanged text is
    /// sent again only with `always`, for a caller that wants a new report.
    fn sync(&self, path: &Path, text: &str, always: bool) {
        let uri = file_uri(path);
        let version = {
            let mut open = self.open.lock().expect("lsp lock");
            match open.get_mut(&uri) {
                Some(document) if !always && document.text == text => return,
                Some(document) => {
                    document.version += 1;
                    document.text = text.to_string();
                    document.version
                }
                None => {
                    let document = Document {
                        version: 1,
                        path: path.to_path_buf(),
                        text: text.to_string(),
                    };
                    open.insert(uri.clone(), document);
                    1
                }
            }
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
    }

    /// Sends every open document that has changed on disk. A server stops
    /// reading a file from disk once it is open, so without this it would
    /// answer about, and compute edits for, text that is no longer there.
    fn refresh(&self) {
        let stale: Vec<(PathBuf, String)> = self
            .open
            .lock()
            .expect("lsp lock")
            .values()
            .filter_map(|document| {
                let text = std::fs::read_to_string(&document.path).ok()?;
                (text != document.text).then(|| (document.path.clone(), text))
            })
            .collect();
        for (path, text) in stale {
            self.sync(&path, &text, false);
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
        self.sync(path, text, true);
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

/// The session's language servers, shared by the tools that talk to them
/// so each server runs once.
///
/// Servers start the first time a file of theirs is asked about and stay
/// up for the rest of the session, so later calls are quick.
struct Servers {
    servers: Vec<Server>,
    policy: Option<SandboxPolicy>,
    helper: PathBuf,
    root: PathBuf,
    /// Says no to files a rename may not change.
    guard: Option<PathGuard>,
}

/// Rules on a file a rename would change: given its absolute path and its
/// path within the working directory, the reason it may not be changed.
/// The approval rules only see the file a rename starts from; this is how
/// they reach the others.
pub type PathGuard = Arc<dyn Fn(&Path, &str) -> Result<(), String> + Send + Sync>;

/// One file's part of a rename, ready to be written.
struct Change {
    path: PathBuf,
    shown: String,
    text: String,
    edits: usize,
}

/// A symbol the model pointed at, in the server's terms.
struct Target<'a> {
    client: &'a LspClient,
    uri: String,
    position: Value,
}

impl Servers {
    fn new(
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
            guard: None,
        }
    }

    fn extensions(&self) -> String {
        let extensions: Vec<&str> = self
            .servers
            .iter()
            .flat_map(|server| server.config.extensions.iter().map(String::as_str))
            .collect();
        extensions.join(", ")
    }

    fn server_for(&self, path: &Path) -> Option<&Server> {
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        self.servers
            .iter()
            .find(|server| server.config.extensions.iter().any(|e| e == extension))
    }

    async fn client<'a>(&'a self, server: &'a Server) -> Result<&'a LspClient, String> {
        server
            .client
            .get_or_init(|| {
                LspClient::start(
                    &server.config,
                    self.policy.as_ref(),
                    &self.helper,
                    &self.root,
                )
            })
            .await
            .as_ref()
            .map_err(Clone::clone)
    }

    fn shown(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    /// Finds the symbol a call names and gets the server ready to be asked
    /// about it. `capability` is what the server must have announced.
    async fn locate<'a>(
        &'a self,
        input: &Value,
        ctx: &ToolContext,
        capability: &str,
    ) -> Result<Target<'a>, String> {
        // Weak models send numbers as strings.
        let line = input["line"]
            .as_u64()
            .or_else(|| input["line"].as_str()?.trim().parse().ok());
        let (Some(path), Some(line), Some(symbol)) =
            (input["path"].as_str(), line, input["symbol"].as_str())
        else {
            return Err("Invalid input: path, line and symbol are required.".into());
        };
        let path = ctx.resolve(path);
        let path = hame_kernel::real_path(&path).unwrap_or(path);
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let Some(server) = self.server_for(&path) else {
            return Err(format!(
                "No language server is configured for .{extension} files. {FALLBACK}"
            ));
        };
        // This runs outside the sandbox, so it keeps to what the sandbox
        // would let a tool read.
        self.readable(&path)?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        let character = column(&text, line, symbol)
            .map_err(|why| format!("In {}, {why}", self.shown(&path)))?;
        let client = self.client(server).await?;
        if !client.supports(capability) {
            return Err(format!(
                "The language server for .{extension} files does not support this. {FALLBACK}"
            ));
        }
        client.refresh();
        client.sync(&path, &text, false);
        Ok(Target {
            client,
            uri: file_uri(&path),
            position: json!({"line": line - 1, "character": character}),
        })
    }

    /// Works out every file a rename changes, without writing anything, so
    /// one edit that cannot be made stops the whole rename.
    fn plan(&self, edit: &Value, old_name: &str) -> Result<Vec<Change>, String> {
        let root = hame_kernel::real_path(&self.root).unwrap_or_else(|_| self.root.clone());
        let mut changes: BTreeMap<PathBuf, Change> = BTreeMap::new();
        for (uri, edits) in document_edits(edit)? {
            let path = uri_path(&uri).ok_or(format!("The rename would change {uri}."))?;
            let path = hame_kernel::real_path(&path)
                .map_err(|e| format!("Cannot read {}: {e}.", path.display()))?;
            // This tool runs outside the sandbox, so it keeps to the
            // working directory itself.
            let Ok(inside) = path.strip_prefix(&root) else {
                return Err(format!(
                    "The rename would change {}, which is outside the working directory.",
                    path.display()
                ));
            };
            let shown = inside.display().to_string();
            if inside.components().any(|part| part.as_os_str() == ".git") {
                return Err(format!("The rename would change {shown}."));
            }
            if let Some(guard) = &self.guard {
                guard(&path, &shown)?;
            }
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("Cannot read {shown}: {e}."))?;
            let (text, count) =
                apply_edits_replacing(&text, &edits, Some(old_name)).map_err(|why| {
                    format!(
                        "The language server's edits do not fit {shown} ({why}); it may have an \
                     old copy of the file."
                    )
                })?;
            let change = Change {
                path: path.clone(),
                shown,
                text,
                edits: count,
            };
            // A second set of edits would refer to the file as the first
            // left it; servers do not send that for a rename.
            if let Some(twice) = changes.insert(path, change) {
                return Err(format!(
                    "The language server sent edits to {} in more than one step.",
                    twice.shown
                ));
            }
        }
        Ok(changes
            .into_values()
            .filter(|change| change.edits > 0)
            .collect())
    }

    /// Locations as `path:line: text`, one line each, sorted so that a
    /// file's lines stay together.
    /// Whether a file is one these tools may read and show: inside the
    /// working directory, and not hidden by the sandbox policy. The tools
    /// run in the harness itself, where nothing else would stop them.
    fn readable(&self, path: &Path) -> Result<(), String> {
        let root = hame_kernel::real_path(&self.root).unwrap_or_else(|_| self.root.clone());
        let path = hame_kernel::real_path(path).unwrap_or_else(|_| path.to_path_buf());
        let refuse = || {
            Err(format!(
                "{} is not a file this tool may read.",
                path.display()
            ))
        };
        if !path.starts_with(&root) {
            return refuse();
        }
        if let Some(policy) = &self.policy {
            let hidden = policy.unreadable.iter().any(|hidden| {
                let hidden = hame_kernel::real_path(hidden).unwrap_or_else(|_| hidden.clone());
                path.starts_with(&hidden)
            });
            let env_file = policy.hide_env_files
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(".env"));
            if hidden || env_file {
                return refuse();
            }
        }
        Ok(())
    }

    fn render_references(&self, locations: &[Value]) -> String {
        let root = hame_kernel::real_path(&self.root).unwrap_or_else(|_| self.root.clone());
        let mut places: Vec<(PathBuf, u64)> = locations
            .iter()
            .filter_map(|location| {
                // A `LocationLink` names the same things differently.
                let uri = location["uri"]
                    .as_str()
                    .or(location["targetUri"].as_str())?;
                let range = location.get("range").unwrap_or(&location["targetRange"]);
                Some((uri_path(uri)?, range["start"]["line"].as_u64()?))
            })
            .collect();
        places.sort();
        places.dedup();
        let mut texts: HashMap<&Path, Option<String>> = HashMap::new();
        let mut lines: Vec<String> = places
            .iter()
            .take(MAX_REFERENCES)
            .map(|(path, line)| {
                let text = texts
                    .entry(path)
                    // A use in a library outside the project is named,
                    // not read.
                    .or_insert_with(|| {
                        self.readable(path)
                            .ok()
                            .and_then(|()| std::fs::read_to_string(path).ok())
                    });
                let content = text.as_deref().and_then(|text| {
                    let &(start, end) = line_spans(text).get(usize::try_from(*line).ok()?)?;
                    Some(text[start..end].trim())
                });
                let content: String = content.unwrap_or("").chars().take(MAX_LINE_CHARS).collect();
                let shown = path.strip_prefix(&root).unwrap_or(path).display();
                format!("{shown}:{}: {content}", line + 1)
                    .trim_end()
                    .to_string()
            })
            .collect();
        if places.len() > MAX_REFERENCES {
            lines.push(format!("... and {} more", places.len() - MAX_REFERENCES));
        }
        lines.join("\n")
    }
}

/// The tools language servers provide: `diagnostics`, `references` and
/// `rename_symbol`, sharing one set of running servers.
pub fn tools(
    servers: &BTreeMap<String, LspServerConfig>,
    policy: Option<SandboxPolicy>,
    helper: PathBuf,
    root: PathBuf,
    guard: Option<PathGuard>,
) -> Vec<Arc<dyn Tool>> {
    let mut lsp = Servers::new(servers, policy, helper, root);
    lsp.guard = guard;
    let lsp = Arc::new(lsp);
    vec![
        Arc::new(DiagnosticsTool {
            lsp: Arc::clone(&lsp),
        }),
        Arc::new(ReferencesTool {
            lsp: Arc::clone(&lsp),
        }),
        Arc::new(RenameSymbolTool { lsp }),
    ]
}

fn symbol_schema(extra: Value) -> Value {
    let mut properties = json!({
        "path": {"type": "string", "description": "A file the symbol appears in."},
        "line": {"type": "integer", "description": "A line of that file with the symbol on it, counted from 1."},
        "symbol": {"type": "string", "description": "The symbol's name exactly as written on that line."},
    });
    let mut required = vec!["path", "line", "symbol"];
    if let (Some(properties), Some(extra)) = (properties.as_object_mut(), extra.as_object()) {
        for (name, schema) in extra {
            properties.insert(name.clone(), schema.clone());
            required.push(name);
        }
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

/// Lists every place a symbol is used, as the language server sees it.
pub struct ReferencesTool {
    lsp: Arc<Servers>,
}

impl Tool for ReferencesTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "references".into(),
            description: format!(
                "List every place a function, type, variable or other symbol is defined or \
                 used, as path:line: text. Unlike grep it follows the language's rules: it \
                 skips other symbols with the same name and text in comments and strings. \
                 Point at the symbol with a file, a line and its name. Available for these \
                 file types: {}.",
                self.lsp.extensions()
            ),
            input_schema: symbol_schema(json!({})),
        }
    }

    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    fn host_side(&self) -> bool {
        true
    }

    /// One conversation per server: requests go one at a time.
    fn concurrent(&self) -> bool {
        false
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let target = match self.lsp.locate(input, ctx, "referencesProvider").await {
                Ok(target) => target,
                Err(why) => return ToolOutput::error(why),
            };
            let answer = target
                .client
                .request(
                    "textDocument/references",
                    json!({
                        "textDocument": {"uri": target.uri},
                        "position": target.position,
                        "context": {"includeDeclaration": true},
                    }),
                )
                .await;
            match answer {
                Err(why) => ToolOutput::error(format!("{why}. {FALLBACK}")),
                Ok(locations) => {
                    let found = self
                        .lsp
                        .render_references(locations.as_array().map_or(&[], Vec::as_slice));
                    if found.is_empty() {
                        ToolOutput::ok(
                            "No references found. If the language server has just started it \
                             may still be reading the project; try again or use grep.",
                        )
                    } else {
                        ToolOutput::ok(found)
                    }
                }
            }
        })
    }
}

/// Renames a symbol everywhere it is used, with the edits the language
/// server works out.
pub struct RenameSymbolTool {
    lsp: Arc<Servers>,
}

impl Tool for RenameSymbolTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "rename_symbol".into(),
            description: format!(
                "Rename a function, type, variable or other symbol in every file that uses \
                 it, in one call. Unlike a text replace it follows the language's rules and \
                 leaves other symbols with the same name alone. Point at the symbol with a \
                 file, a line and its current name. Returns the files changed. Available for \
                 these file types: {}.",
                self.lsp.extensions()
            ),
            input_schema: symbol_schema(json!({
                "new_name": {"type": "string", "description": "The symbol's new name."},
            })),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn host_side(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let Some(new_name) = input["new_name"].as_str().filter(|name| !name.is_empty()) else {
                return ToolOutput::error("Invalid input: new_name is required.");
            };
            // The sandbox does not see this tool's writes, so its rule is
            // kept here.
            if matches!(&self.lsp.policy, Some(policy) if policy.mode == Mode::ReadOnly) {
                return ToolOutput::error("Nothing can be written in this session.");
            }
            let target = match self.lsp.locate(input, ctx, "renameProvider").await {
                Ok(target) => target,
                Err(why) => return ToolOutput::error(why),
            };
            let answer = target
                .client
                .request(
                    "textDocument/rename",
                    json!({
                        "textDocument": {"uri": target.uri},
                        "position": target.position,
                        "newName": new_name,
                    }),
                )
                .await;
            let edit = match answer {
                Ok(edit) => edit,
                Err(why) => return ToolOutput::error(format!("{why}. {FALLBACK}")),
            };
            let old_name = input["symbol"].as_str().unwrap_or("");
            let changes = match self.lsp.plan(&edit, old_name) {
                Ok(changes) => changes,
                Err(why) => {
                    return ToolOutput::error(format!("{why} Nothing was changed. {FALLBACK}"));
                }
            };
            if changes.is_empty() {
                return ToolOutput::error(format!(
                    "The language server found nothing to rename there. {FALLBACK}"
                ));
            }
            for (done, change) in changes.iter().enumerate() {
                if let Err(e) = write_atomic(&change.path, change.text.as_bytes()) {
                    target.client.refresh();
                    let written: Vec<&str> = changes[..done]
                        .iter()
                        .map(|change| change.shown.as_str())
                        .collect();
                    return ToolOutput::error(format!(
                        "Cannot write {}: {e}. The rename is incomplete; already changed: {}.",
                        change.shown,
                        if written.is_empty() {
                            "nothing".to_string()
                        } else {
                            written.join(", ")
                        }
                    ));
                }
            }
            // The server learns of the new text in the files it has open.
            target.client.refresh();
            let plural = |count: usize, one: &str| {
                format!("{count} {one}{}", if count == 1 { "" } else { "s" })
            };
            let mut summary = format!(
                "Renamed {} to {new_name}: {} in {}.",
                input["symbol"].as_str().unwrap_or(""),
                plural(changes.iter().map(|change| change.edits).sum(), "edit"),
                plural(changes.len(), "file"),
            );
            for change in &changes {
                summary.push_str(&format!(
                    "\n{}: {}",
                    change.shown,
                    plural(change.edits, "edit")
                ));
            }
            ToolOutput::ok(summary)
        })
    }
}

/// Asks the project's language servers what is wrong with a file.
pub struct DiagnosticsTool {
    lsp: Arc<Servers>,
}

impl DiagnosticsTool {
    pub fn new(
        servers: &BTreeMap<String, LspServerConfig>,
        policy: Option<SandboxPolicy>,
        helper: PathBuf,
        root: PathBuf,
    ) -> Self {
        Self {
            lsp: Arc::new(Servers::new(servers, policy, helper, root)),
        }
    }
}

impl Tool for DiagnosticsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "diagnostics".into(),
            description: format!(
                "Get the compiler or language-server errors and warnings for a file, as \
                 path:line:column: severity: message. Use it after editing a file to check \
                 the change before moving on. Available for these file types: {}.",
                self.lsp.extensions()
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
            let path = hame_kernel::real_path(&path).unwrap_or(path);
            let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let Some(server) = self.lsp.server_for(&path) else {
                return ToolOutput::error(format!(
                    "No language server is configured for .{extension} files."
                ));
            };
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(e) => return ToolOutput::error(format!("Cannot read {}: {e}", path.display())),
            };
            let client = match self.lsp.client(server).await {
                Ok(client) => client,
                Err(why) => return ToolOutput::error(why),
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
                Some(diagnostics) => ToolOutput::ok(render(&path, &self.lsp.root, &diagnostics)),
            }
        })
    }
}

/// Tests of the text work, which need no server and run everywhere.
#[cfg(test)]
mod text_tests {
    use super::*;

    fn edit(line: u64, from: u64, to: u64, new_text: &str) -> Value {
        json!({"range": {"start": {"line": line, "character": from},
            "end": {"line": line, "character": to}}, "newText": new_text})
    }

    #[test]
    fn positions_count_utf16_units_within_a_line() {
        // é is one unit and two bytes, 😀 two units and four bytes.
        let text = "ab\r\né😀x\nlast";
        let spans = line_spans(text);
        assert_eq!(spans, [(0, 2), (4, 11), (12, 16)]);
        let at = |line: u64, character: u64| {
            offset(text, &spans, &json!({"line": line, "character": character}))
        };
        assert_eq!(at(1, 0), Ok(4));
        assert_eq!(at(1, 1), Ok(6));
        assert_eq!(at(1, 3), Ok(10));
        // Past the end of a line is its end, not the next line.
        assert_eq!(at(0, 9), Ok(2));
        assert_eq!(at(2, 4), Ok(16));
        assert!(at(3, 0).is_err());

        assert_eq!(column(text, 2, "x"), Ok(3));
        // A whole word is preferred to the same letters in a longer name.
        assert_eq!(column("fn foobar(foo: u32)", 1, "foo"), Ok(10));
        assert_eq!(column("fn foobar()", 1, "foo"), Ok(3));
        assert!(column("fn foobar()", 1, "").is_err());
        assert!(column("fn foobar()", 0, "foo").is_err());
    }

    #[test]
    fn edits_apply_whatever_order_they_come_in() {
        let text = "é😀 total + total\r\ntotal\r\n";
        let edits = [
            edit(0, 4, 9, "sum"),
            edit(0, 12, 17, "sum"),
            edit(1, 0, 5, "sum"),
            // Repeated by the server: counted and made once.
            edit(0, 4, 9, "sum"),
        ];
        assert_eq!(
            apply_edits(text, &edits),
            Ok(("é😀 sum + sum\r\nsum\r\n".to_string(), 3))
        );
        // Insertions at one place stay in the order given.
        let inserts = [edit(0, 0, 0, "a"), edit(0, 0, 0, "b"), edit(0, 0, 1, "c")];
        assert_eq!(apply_edits("x", &inserts), Ok(("abc".to_string(), 3)));

        assert!(apply_edits(text, &[edit(0, 4, 9, "a"), edit(0, 8, 12, "b")]).is_err());
        assert!(apply_edits(text, &[edit(7, 0, 1, "a")]).is_err());
        assert!(apply_edits(text, &[json!({"range": {}, "newText": "a"})]).is_err());
        // A rename only ever replaces the old name: a range that holds
        // something else was worked out against an older copy.
        let old = "let total = 1;\n";
        assert!(apply_edits_replacing(old, &[edit(0, 4, 9, "sum")], Some("total")).is_ok());
        let shifted = "// new\nlet total = 1;\n";
        let refused = apply_edits_replacing(shifted, &[edit(0, 4, 9, "sum")], Some("total"));
        assert!(refused.unwrap_err().contains("was expected"));
    }

    #[test]
    fn both_shapes_of_workspace_edit_are_read() {
        let edits = vec![edit(0, 0, 1, "n")];
        let want = Ok(vec![("file:///a.rs".to_string(), edits.clone())]);
        assert_eq!(
            document_edits(&json!({"changes": {"file:///a.rs": edits}})),
            want
        );
        let versioned = json!({"documentChanges": [
            {"textDocument": {"uri": "file:///a.rs", "version": 3}, "edits": edits}]});
        assert_eq!(document_edits(&versioned), want);
        assert_eq!(document_edits(&Value::Null), Ok(Vec::new()));
        let creates = json!({"documentChanges": [{"kind": "create", "uri": "file:///b.rs"}]});
        assert!(document_edits(&creates).unwrap_err().contains("(create)"));
    }

    #[test]
    fn uris_name_files_on_either_kind_of_system() {
        assert_eq!(
            decode_uri("file:///a%20b/%C3%A7.rs", false).as_deref(),
            Some("/a b/ç.rs")
        );
        let windows = encode_uri(r"C:\Users\me\a b.rs", true);
        assert_eq!(windows, "file:///C%3A/Users/me/a%20b.rs");
        assert_eq!(
            decode_uri(&windows, true).as_deref(),
            Some(r"C:\Users\me\a b.rs")
        );
        // As servers write it.
        assert_eq!(
            decode_uri("file:///c:/Users/me/a.rs", true).as_deref(),
            Some(r"c:\Users\me\a.rs")
        );
        assert_eq!(decode_uri("untitled:Untitled-1", false), None);
        assert_eq!(decode_uri("file:///a%2", false), None);
    }

    #[test]
    fn a_long_list_of_references_is_cut_with_a_count() {
        let lsp = Servers::new(
            &BTreeMap::new(),
            None,
            PathBuf::from("hame"),
            PathBuf::from("/nowhere"),
        );
        // Two on one line are one line of output.
        let locations: Vec<Value> = (0..131u64)
            .map(|line| line.min(129))
            .map(|line| json!({"uri": "file:///nowhere/x.rs", "range": {"start": {"line": line}}}))
            .collect();
        let shown = lsp.render_references(&locations);
        let lines: Vec<&str> = shown.lines().collect();
        assert_eq!(lines.len(), MAX_REFERENCES + 1);
        assert!(lines[0].ends_with("x.rs:1:"), "{}", lines[0]);
        assert!(lines[99].ends_with("x.rs:100:"));
        assert_eq!(lines[100], "... and 30 more");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A language server that reports every line containing "BUG", first
    /// with an empty report (as real servers do), then the real one. A
    /// symbol is a word, used wherever the workspace's .txt files have it;
    /// some new names make a rename answer in a particular way.
    const SERVER: &str = r#"
import json, os, re, sys
from urllib.parse import quote, unquote
docs = {}
root = None
def u16(s): return len(s.encode("utf-16-le")) // 2
def text_of(uri):
    if uri in docs: return docs[uri]
    with open(unquote(uri[7:]), encoding="utf-8", newline="") as f: return f.read()
def word_at(uri, pos):
    line = text_of(uri).split("\n")[pos["line"]]
    for m in re.finditer(r"\w+", line):
        if u16(line[:m.start()]) == pos["character"]: return m.group()
def uses(word):
    found = []
    for name in sorted(os.listdir(root)) if word else []:
        if not name.endswith(".txt"): continue
        uri = "file://" + quote(os.path.join(root, name))
        for i, line in enumerate(text_of(uri).split("\n")):
            for m in re.finditer(r"\b%s\b" % re.escape(word), line):
                found.append((uri, {"start": {"line": i, "character": u16(line[:m.start()])},
                                    "end": {"line": i, "character": u16(line[:m.end()])}}))
    return found
def rename(uri, pos, new):
    edits = {}
    for u, r in uses(word_at(uri, pos)): edits.setdefault(u, []).append({"range": r, "newText": new})
    if new == "outside":
        first = {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}
        edits["file://" + quote(os.path.join(os.path.dirname(root), "outside.txt"))] = [{"range": first, "newText": new}]
    if new == "moved":
        return {"documentChanges": [{"kind": "rename", "oldUri": uri, "newUri": uri + ".old"}]}
    if new.startswith("doc"):
        return {"documentChanges": [{"textDocument": {"uri": u, "version": None}, "edits": e} for u, e in edits.items()]}
    return {"changes": edits}
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
        root = unquote(m["params"]["rootUri"][7:])
        can = {} if "bare" in sys.argv else {"referencesProvider": True, "renameProvider": True}
        send({"jsonrpc": "2.0", "id": 99, "method": "workspace/configuration", "params": {"items": [{}, {}]}})
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"capabilities": can}})
    elif method == "textDocument/didOpen":
        docs[m["params"]["textDocument"]["uri"]] = m["params"]["textDocument"]["text"]
        publish(m["params"]["textDocument"]["uri"], m["params"]["textDocument"]["text"])
    elif method == "textDocument/didChange":
        docs[m["params"]["textDocument"]["uri"]] = m["params"]["contentChanges"][0]["text"]
        publish(m["params"]["textDocument"]["uri"], m["params"]["contentChanges"][0]["text"])
    elif method == "textDocument/references":
        p = m["params"]
        found = uses(word_at(p["textDocument"]["uri"], p["position"]))
        send({"jsonrpc": "2.0", "id": m["id"], "result": [{"uri": u, "range": r} for u, r in found]})
    elif method == "textDocument/rename":
        p = m["params"]
        send({"jsonrpc": "2.0", "id": m["id"], "result": rename(p["textDocument"]["uri"], p["position"], p["newName"])})
"#;

    /// The fake server set up for `.txt` files in `dir`.
    fn servers(dir: &Path, bare: bool) -> Arc<Servers> {
        let script = dir.join("server.py");
        std::fs::write(&script, SERVER).unwrap();
        let mut args = vec![script.display().to_string()];
        if bare {
            args.push("bare".into());
        }
        let mut servers = BTreeMap::new();
        servers.insert(
            "fake".to_string(),
            LspServerConfig {
                command: hame_kernel::python_program()
                    .expect("these tests need Python")
                    .into(),
                args,
                extensions: vec!["txt".into()],
                sandbox: false,
                timeout_secs: Some(10),
                settle_ms: Some(200),
            },
        );
        Arc::new(Servers::new(
            &servers,
            None,
            PathBuf::from("hame"),
            dir.to_path_buf(),
        ))
    }

    fn tool(dir: &Path) -> DiagnosticsTool {
        DiagnosticsTool {
            lsp: servers(dir, false),
        }
    }

    const A: &str = "let total = 1;\né😀 total + total\n";
    const B: &str = "  print(total)  \nsubtotal\n";

    #[tokio::test]
    async fn references_are_listed_by_file_and_line() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("a.txt"), A).unwrap();
        std::fs::write(root.join("b.txt"), B).unwrap();
        let tool = ReferencesTool {
            lsp: servers(&root, false),
        };
        let ctx = ToolContext::new(&root);

        // The server answers only for the exact UTF-16 column of the name.
        let found = tool
            .call(
                &json!({"path": "a.txt", "line": 2, "symbol": "total"}),
                &ctx,
            )
            .await;
        let want = "a.txt:1: let total = 1;\na.txt:2: é😀 total + total\nb.txt:1: print(total)";
        assert_eq!(found, ToolOutput::ok(want));
        let as_text = tool
            .call(
                &json!({"path": "b.txt", "line": "1", "symbol": "total"}),
                &ctx,
            )
            .await;
        assert_eq!(as_text, ToolOutput::ok(want));

        // A file changed since the server opened it is sent again first.
        std::fs::write(root.join("a.txt"), "é total\n").unwrap();
        let after = tool
            .call(
                &json!({"path": "b.txt", "line": 1, "symbol": "total"}),
                &ctx,
            )
            .await;
        assert_eq!(
            after,
            ToolOutput::ok("a.txt:1: é total\nb.txt:1: print(total)")
        );

        let absent = tool
            .call(&json!({"path": "b.txt", "line": 2, "symbol": "nope"}), &ctx)
            .await;
        assert!(absent.is_error && absent.text.contains("does not contain `nope`"));
        let past = tool
            .call(
                &json!({"path": "b.txt", "line": 9, "symbol": "total"}),
                &ctx,
            )
            .await;
        assert!(past.is_error && past.text.contains("there is no line 9"));
    }

    #[tokio::test]
    async fn a_rename_changes_every_file_or_none() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path().canonicalize().unwrap();
        let root = outer.join("work");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(outer.join("outside.txt"), "total\n").unwrap();
        std::fs::write(root.join("a.txt"), A).unwrap();
        std::fs::write(root.join("b.txt"), B).unwrap();
        let read = |name: &str| std::fs::read_to_string(root.join(name)).unwrap();
        let tool = RenameSymbolTool {
            lsp: servers(&root, false),
        };
        let ctx = ToolContext::new(&root);
        let rename = |path: &'static str, line: u64, symbol: &'static str, new: &'static str| {
            let input = json!({"path": path, "line": line, "symbol": symbol, "new_name": new});
            let (tool, ctx) = (&tool, &ctx);
            async move { tool.call(&input, ctx).await }
        };

        // An edit outside the working directory stops all of them.
        let refused = rename("a.txt", 1, "total", "outside").await;
        assert!(refused.is_error, "{}", refused.text);
        assert!(refused.text.contains("outside the working directory"));
        assert!(refused.text.contains("Nothing was changed"));
        assert_eq!((read("a.txt"), read("b.txt")), (A.into(), B.into()));
        assert_eq!(
            std::fs::read_to_string(outer.join("outside.txt")).unwrap(),
            "total\n"
        );
        let moved = rename("a.txt", 1, "total", "moved").await;
        assert!(moved.is_error && moved.text.contains("file operation (rename)"));
        assert_eq!(read("a.txt"), A);

        // Two edits on one line, after characters that take one and two
        // UTF-16 units but two and four bytes.
        let done = rename("a.txt", 2, "total", "sum").await;
        assert_eq!(
            done,
            ToolOutput::ok(
                "Renamed total to sum: 4 edits in 2 files.\na.txt: 3 edits\nb.txt: 1 edit"
            )
        );
        assert_eq!(read("a.txt"), "let sum = 1;\né😀 sum + sum\n");
        assert_eq!(read("b.txt"), "  print(sum)  \nsubtotal\n");

        // The server has a.txt open: this works only if it was told of the
        // first rename. The answer comes as `documentChanges` this time.
        let again = rename("b.txt", 1, "sum", "doc_sum").await;
        assert!(!again.is_error, "{}", again.text);
        assert_eq!(read("a.txt"), "let doc_sum = 1;\né😀 doc_sum + doc_sum\n");
        assert_eq!(read("b.txt"), "  print(doc_sum)  \nsubtotal\n");

        let nothing = rename("b.txt", 2, "subtotal", "").await;
        assert!(nothing.is_error && nothing.text.contains("new_name"));
    }

    #[tokio::test]
    async fn without_a_server_that_can_the_model_is_sent_back_to_grep() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("a.txt"), A).unwrap();
        let lsp = servers(&root, true);
        let references = ReferencesTool {
            lsp: Arc::clone(&lsp),
        };
        let rename = RenameSymbolTool { lsp };
        let ctx = ToolContext::new(&root);
        assert_eq!(references.effect(), Effect::ReadOnly);
        assert_eq!(rename.effect(), Effect::Write);

        for path in ["x.zig", "a.txt"] {
            let input = json!({"path": path, "line": 1, "symbol": "total", "new_name": "sum"});
            let why = if path == "x.zig" {
                "No language server is configured for .zig files."
            } else {
                "does not support this."
            };
            let found = references
                .call(&json!({"path": path, "line": 1, "symbol": "total"}), &ctx)
                .await;
            assert!(found.is_error && found.text.contains(why), "{}", found.text);
            assert!(found.text.ends_with(FALLBACK));
            let renamed = rename.call(&input, &ctx).await;
            assert!(renamed.is_error && renamed.text.contains(why));
            assert!(renamed.text.ends_with(FALLBACK));
        }
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), A);
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
