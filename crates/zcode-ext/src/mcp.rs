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

/// How to reach one Model Context Protocol server: a `command` to start
/// and talk to over stdio, or the `url` of one served over HTTP.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    #[serde(default)]
    pub command: String,
    /// Endpoint of a server that speaks the streamable HTTP transport.
    #[serde(default)]
    pub url: Option<String>,
    /// Headers sent with every HTTP request, for example an
    /// `Authorization` header. `$NAME` or `${NAME}` in a value stands for
    /// an environment variable or secret of that name.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
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

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            url: None,
            headers: BTreeMap::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            sandbox: true,
        }
    }
}

impl McpServerConfig {
    /// Replaces `$NAME` and `${NAME}` in header values with what `lookup`
    /// gives for the name. A name it does not know is an error: sending the
    /// literal text as a credential would only fail later and less clearly.
    pub fn expand_headers(
        &mut self,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<(), String> {
        for (header, value) in &mut self.headers {
            let mut expanded = String::new();
            let mut rest = value.as_str();
            while let Some(at) = rest.find('$') {
                expanded.push_str(&rest[..at]);
                let after = &rest[at + 1..];
                let (name, remainder) = match after.strip_prefix('{') {
                    Some(braced) => match braced.split_once('}') {
                        Some((name, remainder)) => (name, remainder),
                        None => ("", after),
                    },
                    None => {
                        let end = after
                            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                            .unwrap_or(after.len());
                        after.split_at(end)
                    }
                };
                if name.is_empty() {
                    expanded.push('$');
                    rest = after;
                    continue;
                }
                expanded.push_str(
                    &lookup(name).ok_or_else(|| {
                        format!("header {header} needs ${name}, which is not set")
                    })?,
                );
                rest = remainder;
            }
            expanded.push_str(rest);
            *value = expanded;
        }
        Ok(())
    }
}

/// A server reached over HTTP: every message is a POST to one endpoint,
/// answered with JSON or with a short event stream that carries the answer.
struct HttpLink {
    http: reqwest::Client,
    url: String,
    headers: BTreeMap<String, String>,
    /// Given by the server at initialization, to be sent back from then on.
    session: Mutex<Option<String>>,
    /// The protocol version agreed at initialization.
    version: Mutex<Option<String>>,
}

/// Pulls complete events off the front of an event-stream buffer and
/// returns the JSON-RPC message answering request `id`, if it has arrived.
fn response_in_stream(buffer: &mut String, id: u64) -> Option<Value> {
    while let Some(end) = buffer.find("\n\n") {
        let event: String = buffer.drain(..end + 2).collect();
        let data: Vec<&str> = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(|data| data.strip_prefix(' ').unwrap_or(data))
            .collect();
        if let Ok(message) = serde_json::from_str::<Value>(&data.join("\n"))
            && message.get("method").is_none()
            && message["id"].as_u64() == Some(id)
        {
            return Some(message);
        }
    }
    None
}

impl HttpLink {
    /// Sends one message. Returns the response to it when `id` is given,
    /// and nothing for a notification.
    async fn exchange(
        &self,
        name: &str,
        message: &Value,
        id: Option<u64>,
    ) -> Result<Option<Value>, String> {
        let mut request = self
            .http
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        for (header, value) in &self.headers {
            request = request.header(header, value);
        }
        let session = self.session.lock().expect("mcp lock").clone();
        if let Some(session) = session {
            request = request.header("mcp-session-id", session);
        }
        let version = self.version.lock().expect("mcp lock").clone();
        if let Some(version) = version {
            request = request.header("mcp-protocol-version", version);
        }
        let mut response = request
            .body(message.to_string())
            .send()
            .await
            .map_err(|e| format!("cannot reach MCP server {name}: {e}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(match status.as_u16() {
                401 | 403 => format!(
                    "MCP server {name} refused the credentials (HTTP {status}); check its \
                     `headers` in the configuration"
                ),
                _ => format!("MCP server {name} answered HTTP {status}"),
            });
        }
        if let Some(session) = response.headers().get("mcp-session-id")
            && let Ok(session) = session.to_str()
        {
            *self.session.lock().expect("mcp lock") = Some(session.to_string());
        }
        let Some(id) = id else {
            return Ok(None);
        };
        let streamed = response
            .headers()
            .get("content-type")
            .and_then(|kind| kind.to_str().ok())
            .is_some_and(|kind| kind.starts_with("text/event-stream"));
        let cut_off = || format!("MCP server {name} closed the connection before answering");
        if !streamed {
            let body = response.bytes().await.map_err(|_| cut_off())?;
            return serde_json::from_slice(&body)
                .map(Some)
                .map_err(|e| format!("MCP server {name} sent an unreadable answer: {e}"));
        }
        // The answer is one of the events; the server may send others
        // first, and may or may not close the stream afterwards.
        let mut buffer = String::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| cut_off())? {
            buffer.push_str(&String::from_utf8_lossy(&chunk).replace("\r\n", "\n"));
            if let Some(message) = response_in_stream(&mut buffer, id) {
                return Ok(Some(message));
            }
        }
        buffer.push_str("\n\n");
        response_in_stream(&mut buffer, id)
            .map(Some)
            .ok_or_else(cut_off)
    }
}

/// How messages reach the server.
enum Link {
    Stdio {
        outbox: mpsc::UnboundedSender<String>,
        waiting: Waiting,
        /// Kills the server and anything it started when the client is
        /// dropped.
        _group: GroupGuard,
    },
    Http(HttpLink),
}

type Waiting = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

/// A running server and the JSON-RPC conversation with it.
pub struct McpClient {
    name: String,
    link: Link,
    next_id: AtomicU64,
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
        match (&config.url, config.command.is_empty()) {
            (Some(url), true) => return Self::connect(name, url, &config.headers).await,
            (Some(_), false) => {
                return Err(format!(
                    "MCP server {name}: give either `command` or `url`, not both"
                ));
            }
            (None, true) => {
                return Err(format!("MCP server {name}: `command` or `url` is required"));
            }
            (None, false) => {}
        }
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
            link: Link::Stdio {
                outbox,
                waiting,
                _group: group,
            },
            next_id: AtomicU64::new(1),
        });
        client.handshake().await?;
        Ok(client)
    }

    /// Connects to a server over HTTP and completes the handshake.
    async fn connect(
        name: &str,
        url: &str,
        headers: &BTreeMap<String, String>,
    ) -> Result<Arc<Self>, String> {
        let http = reqwest::Client::builder()
            .connect_timeout(STARTUP_TIMEOUT)
            .build()
            .map_err(|e| format!("MCP server {name}: {e}"))?;
        let client = Arc::new(Self {
            name: name.to_string(),
            link: Link::Http(HttpLink {
                http,
                url: url.to_string(),
                headers: headers.clone(),
                session: Mutex::new(None),
                version: Mutex::new(None),
            }),
            next_id: AtomicU64::new(1),
        });
        client.handshake().await?;
        Ok(client)
    }

    async fn handshake(&self) -> Result<(), String> {
        let agreed = self
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
        let ready = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        match &self.link {
            Link::Stdio { outbox, .. } => {
                let _ = outbox.send(ready.to_string());
            }
            Link::Http(link) => {
                *link.version.lock().expect("mcp lock") = Some(
                    agreed["protocolVersion"]
                        .as_str()
                        .unwrap_or(PROTOCOL_VERSION)
                        .to_string(),
                );
                link.exchange(&self.name, &ready, None).await?;
            }
        }
        Ok(())
    }

    async fn request(&self, method: &str, params: Value, limit: Duration) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let gone = || format!("MCP server {} stopped responding", self.name);
        let late = || {
            format!(
                "MCP server {} did not answer {method} within {} s",
                self.name,
                limit.as_secs()
            )
        };
        let response = match &self.link {
            Link::Stdio {
                outbox, waiting, ..
            } => {
                let (answer, answered) = oneshot::channel();
                waiting.lock().expect("mcp lock").insert(id, answer);
                outbox.send(message.to_string()).map_err(|_| gone())?;
                match tokio::time::timeout(limit, answered).await {
                    Ok(Ok(response)) => response,
                    Ok(Err(_)) => return Err(gone()),
                    Err(_) => {
                        waiting.lock().expect("mcp lock").remove(&id);
                        return Err(late());
                    }
                }
            }
            Link::Http(link) => {
                match tokio::time::timeout(limit, link.exchange(&self.name, &message, Some(id)))
                    .await
                {
                    Ok(exchanged) => exchanged?.ok_or_else(gone)?,
                    Err(_) => return Err(late()),
                }
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
            ..McpServerConfig::default()
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

    /// Serves the MCP HTTP transport on a local port: wants a bearer token,
    /// hands out a session at initialization and insists on it afterwards,
    /// and answers `tools/list` as an event stream with a notification in
    /// front of the answer.
    async fn http_server() -> String {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let (head, body) = loop {
                        let Ok(n) = socket.read(&mut chunk).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&raw).to_string();
                        if let Some((head, body)) = text.split_once("\r\n\r\n") {
                            let length: usize = head
                                .lines()
                                .find_map(|line| {
                                    line.to_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse().unwrap())
                                })
                                .unwrap_or(0);
                            if body.len() >= length {
                                break (head.to_lowercase(), body.to_string());
                            }
                        }
                    };
                    let message: Value = serde_json::from_str(&body).unwrap();
                    let (id, method) = (&message["id"], message["method"].as_str().unwrap_or(""));
                    let answer =
                        |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result});
                    let (status, kind, extra, payload) = if !head
                        .contains("authorization: bearer t0ken")
                    {
                        ("401 Unauthorized", "text/plain", "", String::new())
                    } else if method == "initialize" {
                        let result = json!({"protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": {"name": "h", "version": "0"}});
                        (
                            "200 OK",
                            "application/json",
                            "Mcp-Session-Id: s-77\r\n",
                            answer(result).to_string(),
                        )
                    } else if !head.contains("mcp-session-id: s-77")
                        || !head.contains("mcp-protocol-version: 2025-06-18")
                    {
                        ("400 Bad Request", "text/plain", "", String::new())
                    } else if method == "notifications/initialized" {
                        ("202 Accepted", "text/plain", "", String::new())
                    } else if method == "tools/list" {
                        let tools = json!({"tools": [{"name": "greet", "description": "Greet someone.", "inputSchema": {"type": "object"}}]});
                        let stream = format!(
                            "event: message\r\ndata: {}\r\n\r\nevent: message\r\ndata: {}\r\n\r\n",
                            json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {}}),
                            answer(tools)
                        );
                        ("200 OK", "text/event-stream", "", stream)
                    } else {
                        let name = message["params"]["arguments"]["name"]
                            .as_str()
                            .unwrap_or("?");
                        let result =
                            json!({"content": [{"type": "text", "text": format!("hello {name}")}]});
                        ("200 OK", "application/json", "", answer(result).to_string())
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                        payload.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        url
    }

    #[tokio::test]
    async fn a_server_over_http_is_used_like_a_local_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = McpServerConfig {
            url: Some(http_server().await),
            headers: BTreeMap::from([(
                "Authorization".to_string(),
                "Bearer ${API_TOKEN}".to_string(),
            )]),
            ..McpServerConfig::default()
        };

        // A credential that is not available is said so before any request.
        let error = config.clone().expand_headers(|_| None).unwrap_err();
        assert_eq!(
            error,
            "header Authorization needs $API_TOKEN, which is not set"
        );
        config
            .expand_headers(|name| (name == "API_TOKEN").then(|| "t0ken".to_string()))
            .unwrap();
        assert_eq!(config.headers["Authorization"], "Bearer t0ken");

        let client = McpClient::start("web", &config, None, Path::new("zcode"), dir.path())
            .await
            .unwrap();
        let tools = client.tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].spec().name, "mcp__web__greet");
        let greeted = tools[0]
            .call(&json!({"name": "Lan"}), &ToolContext::new(dir.path()))
            .await;
        assert_eq!(greeted, ToolOutput::ok("hello Lan"));

        let wrong = McpServerConfig {
            headers: BTreeMap::from([("Authorization".to_string(), "Bearer nope".to_string())]),
            ..config.clone()
        };
        let error = McpClient::start("web", &wrong, None, Path::new("zcode"), dir.path())
            .await
            .err()
            .unwrap();
        assert!(error.contains("refused the credentials"), "{error}");

        let both = McpServerConfig {
            command: "true".into(),
            ..config
        };
        let error = McpClient::start("web", &both, None, Path::new("zcode"), dir.path())
            .await
            .err()
            .unwrap();
        assert!(error.contains("either `command` or `url`"), "{error}");
    }

    #[test]
    fn header_values_expand_names_and_leave_other_dollars_alone() {
        let known = |name: &str| match name {
            "ONE" => Some("1".to_string()),
            "TWO" => Some("2".to_string()),
            _ => None,
        };
        let expand = |value: &str| {
            let mut config = McpServerConfig {
                headers: BTreeMap::from([("A".to_string(), value.to_string())]),
                ..McpServerConfig::default()
            };
            config
                .expand_headers(known)
                .map(|()| config.headers["A"].clone())
        };
        assert_eq!(expand("x-$ONE-${TWO}y").unwrap(), "x-1-2y");
        // A dollar that is not followed by a name is just a dollar.
        assert_eq!(expand("100$ $ ${").unwrap(), "100$ $ ${");
        assert_eq!(
            expand("Bearer $MISSING").unwrap_err(),
            "header A needs $MISSING, which is not set"
        );
    }

    #[tokio::test]
    async fn a_server_that_cannot_start_or_dies_is_an_error_not_a_hang() {
        let dir = tempfile::tempdir().unwrap();
        let missing = McpServerConfig {
            command: "/nonexistent/mcp-server".into(),
            sandbox: false,
            ..McpServerConfig::default()
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
