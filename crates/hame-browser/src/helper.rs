//! The helper process: a Python Playwright program speaking line-delimited
//! JSON over stdio, started once and kept for the tool's lifetime.

use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hame_session::contract::{AdapterManifest, CONTRACT_VERSION, Channel, NestedUsage};
use hame_tools::GroupGuard;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// The helper protocol this crate speaks. The helper reports its own in
/// the handshake and a mismatch is refused.
pub const HELPER_PROTOCOL: u32 = 1;

/// How long the handshake may take: importing Playwright and starting its
/// driver. Browsers start lazily, per profile, on first use.
const HELLO_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub(crate) enum HelperError {
    /// The helper could not be started or refused the handshake.
    Start(String),
    /// The helper exited or its pipe broke.
    Crashed(String),
    /// No reply in time; the helper was killed.
    Timeout,
}

pub(crate) struct Helper {
    _child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    pub pid: u32,
    pub manifest: AdapterManifest,
    pub playwright: String,
    stderr: Arc<Mutex<VecDeque<String>>>,
    // Dropped last: takes the helper's process group down with it.
    _group: GroupGuard,
}

impl Helper {
    pub async fn start(python: &Path, script: &Path, hello: Value) -> Result<Self, HelperError> {
        let mut command = tokio::process::Command::new(python);
        command
            .arg("-I")
            .arg(script)
            .current_dir(script.parent().unwrap_or(Path::new(".")))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|e| HelperError::Start(format!("cannot start {}: {e}", python.display())))?;
        let pid = child.id().unwrap_or(0);
        let group = GroupGuard::new(cfg!(unix).then_some(pid));
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout is piped")).lines();
        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        let tail = Arc::clone(&stderr);
        let mut err_lines = BufReader::new(child.stderr.take().expect("stderr is piped")).lines();
        tokio::spawn(async move {
            while let Ok(Some(line)) = err_lines.next_line().await {
                let mut tail = tail.lock().unwrap_or_else(|e| e.into_inner());
                if tail.len() >= 20 {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        });
        let mut helper = Helper {
            _child: child,
            stdin,
            stdout,
            next_id: 0,
            pid,
            manifest: AdapterManifest {
                name: String::new(),
                version: String::new(),
                protocol: 0,
                channel: Channel::Browser,
                operations: Vec::new(),
                platforms: Vec::new(),
                cancellable: false,
                nested_usage: NestedUsage::Unknown,
            },
            playwright: String::new(),
            stderr,
            _group: group,
        };
        let mut hello = hello;
        hello["op"] = json!("hello");
        hello["protocol"] = json!(HELPER_PROTOCOL);
        hello["contract"] = json!(CONTRACT_VERSION);
        let reply = match helper.request(hello, HELLO_TIMEOUT).await {
            Ok(reply) => reply,
            Err(HelperError::Crashed(why)) => {
                return Err(HelperError::Start(format!(
                    "the browser helper exited during the handshake: {why}"
                )));
            }
            Err(HelperError::Timeout) => {
                return Err(HelperError::Start(
                    "the browser helper did not answer the handshake".into(),
                ));
            }
            Err(e) => return Err(e),
        };
        if reply["ok"] != json!(true) {
            return Err(HelperError::Start(format!(
                "the browser helper refused to start: {}",
                reply["error"]["message"]
                    .as_str()
                    .unwrap_or("unknown error")
            )));
        }
        let manifest: AdapterManifest = serde_json::from_value(reply["manifest"].clone())
            .map_err(|e| HelperError::Start(format!("unreadable helper manifest: {e}")))?;
        if manifest.protocol != HELPER_PROTOCOL
            || reply["contract"] != json!(CONTRACT_VERSION)
            || manifest.channel != Channel::Browser
        {
            return Err(HelperError::Start(format!(
                "the browser helper speaks protocol {} / contract {}, this build needs {} / {}",
                manifest.protocol, reply["contract"], HELPER_PROTOCOL, CONTRACT_VERSION
            )));
        }
        if manifest.nested_usage != NestedUsage::None {
            return Err(HelperError::Start(
                "the browser helper declares model calls of its own, which hame does not allow"
                    .into(),
            ));
        }
        helper.manifest = manifest;
        helper.playwright = reply["playwright"].as_str().unwrap_or("").to_string();
        Ok(helper)
    }

    /// Sends one request and returns its reply line, parsed. The reply may
    /// be `ok: false`; that is the caller's to interpret.
    pub async fn request(
        &mut self,
        mut req: Value,
        timeout: Duration,
    ) -> Result<Value, HelperError> {
        self.next_id += 1;
        let id = self.next_id;
        req["id"] = json!(id);
        let mut line = req.to_string();
        line.push('\n');
        if let Err(e) = self.stdin.write_all(line.as_bytes()).await {
            return Err(HelperError::Crashed(self.why(&e.to_string())));
        }
        if let Err(e) = self.stdin.flush().await {
            return Err(HelperError::Crashed(self.why(&e.to_string())));
        }
        let read = async {
            loop {
                match self.stdout.next_line().await {
                    Ok(Some(line)) => {
                        let Ok(reply) = serde_json::from_str::<Value>(&line) else {
                            continue;
                        };
                        if reply["id"] == json!(id) {
                            return Ok(reply);
                        }
                    }
                    Ok(None) => return Err("it exited".to_string()),
                    Err(e) => return Err(e.to_string()),
                }
            }
        };
        match tokio::time::timeout(timeout, read).await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(e)) => Err(HelperError::Crashed(self.why(&e))),
            Err(_) => Err(HelperError::Timeout),
        }
    }

    fn why(&self, base: &str) -> String {
        let tail = self.stderr_tail();
        if tail.is_empty() {
            base.to_string()
        } else {
            format!("{base}; its last output: {tail}")
        }
    }

    pub fn stderr_tail(&self) -> String {
        let tail = self.stderr.lock().unwrap_or_else(|e| e.into_inner());
        let text: Vec<&str> = tail.iter().rev().take(5).map(String::as_str).collect();
        let mut text: Vec<&str> = text.into_iter().rev().collect();
        text.retain(|l| !l.trim().is_empty());
        text.join(" | ").chars().take(600).collect()
    }
}
