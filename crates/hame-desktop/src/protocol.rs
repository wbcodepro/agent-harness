//! The helper process: building it, starting it, and talking to it.
//!
//! The helper is a small Swift program embedded in this crate. It is
//! compiled on first use into a cache directory keyed by the hash of its
//! source, so an upgraded hame never runs a stale helper, and it then runs
//! as one long-lived child process speaking line-delimited JSON:
//!
//! ```text
//! -> {"id": 1, "op": "hello", "protocol": 1, "first_observation": 1}
//! <- {"id": 1, "ok": true, "result": {"manifest": {...}, "helper_protocol": 1, ...}}
//! -> {"id": 2, "op": "observe", "app": "com.apple.TextEdit"}
//! <- {"id": 2, "ok": false, "error": {"code": "ambiguous", "message": "...", "details": {...}}}
//! -> {"id": 3, "op": "cancel", "target": 2}
//! ```
//!
//! Requests run one at a time. A `cancel` is read by the helper while the
//! request it names is still running, so it can stop a long `type` between
//! chunks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hame_session::contract::{AdapterManifest, CONTRACT_VERSION, Channel, NestedUsage};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// The helper's source, compiled on first use.
pub const HELPER_SOURCE: &str = include_str!("../helper/DesktopHelper.swift");

/// The request/response protocol between this crate and the helper.
/// Bumped whenever a request or result changes shape.
pub const HELPER_PROTOCOL: u32 = 1;

/// Flags the helper is compiled with; part of the cache key.
const SWIFTC_FLAGS: &[&str] = &["-O", "-swift-version", "5"];

/// A failure reported by the helper or by talking to it. `code` is one of
/// a small stable set (`stale_ref`, `stale_window`, `ambiguous`,
/// `not_found`, `unsupported`, `interference`, `unexpected_dialog`,
/// `cancelled`, `permission_denied`, `timeout`, `disabled`, `invalid`,
/// `not_frontmost`, `stale_capture`, `failed`, `helper_exited`, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct HelperError {
    pub code: String,
    pub message: String,
    pub details: Value,
}

impl HelperError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: Value::Null,
        }
    }

    /// Whether the helper may already have changed something when it
    /// failed. Preflight failures never have.
    pub fn dispatched(&self) -> bool {
        self.details["dispatched"]
            .as_bool()
            .unwrap_or(matches!(self.code.as_str(), "helper_exited" | "timeout"))
    }

    /// Text typed before the failure, for `type`.
    pub fn emitted(&self) -> Option<&str> {
        self.details["emitted"].as_str()
    }
}

impl std::fmt::Display for HelperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// Parses one response line into its request id and outcome.
pub fn parse_response(line: &str) -> Option<(u64, Result<Value, HelperError>)> {
    let value: Value = serde_json::from_str(line).ok()?;
    let id = value["id"].as_u64()?;
    if value["ok"].as_bool() == Some(true) {
        return Some((id, Ok(value["result"].clone())));
    }
    let error = &value["error"];
    Some((
        id,
        Err(HelperError {
            code: error["code"].as_str().unwrap_or("failed").to_string(),
            message: error["message"]
                .as_str()
                .unwrap_or("the helper failed without saying why")
                .to_string(),
            details: error["details"].clone(),
        }),
    ))
}

/// Serializes one request line: the operation and its parameters, with
/// the id added.
pub fn request_line(id: u64, op: &str, params: &Value) -> String {
    let mut object = match params {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    object.insert("id".into(), json!(id));
    object.insert("op".into(), json!(op));
    let mut line = Value::Object(object).to_string();
    line.push('\n');
    line
}

/// What the helper says about itself in the handshake.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Hello {
    pub manifest: AdapterManifest,
    pub helper_protocol: u32,
    pub ax_trusted: bool,
    pub screen_access: bool,
    pub pid: u32,
    #[serde(default)]
    pub os: String,
}

/// Checks the handshake: the same contract and helper protocol, the
/// desktop channel, and no model calls of its own.
pub fn validate_hello(result: &Value) -> Result<Hello, HelperError> {
    let hello: Hello = serde_json::from_value(result.clone()).map_err(|e| {
        HelperError::new(
            "protocol",
            format!("The desktop helper's handshake is malformed: {e}"),
        )
    })?;
    if hello.manifest.protocol != CONTRACT_VERSION {
        return Err(HelperError::new(
            "protocol",
            format!(
                "The desktop helper speaks contract version {}, this hame speaks {}.",
                hello.manifest.protocol, CONTRACT_VERSION
            ),
        ));
    }
    if hello.helper_protocol != HELPER_PROTOCOL {
        return Err(HelperError::new(
            "protocol",
            format!(
                "The desktop helper speaks helper protocol {}, this hame speaks {}. \
                 Delete the cached helper so it is rebuilt.",
                hello.helper_protocol, HELPER_PROTOCOL
            ),
        ));
    }
    if hello.manifest.channel != Channel::Desktop {
        return Err(HelperError::new(
            "protocol",
            "The desktop helper declared a channel other than desktop.",
        ));
    }
    if hello.manifest.nested_usage != NestedUsage::None {
        return Err(HelperError::new(
            "protocol",
            "The desktop helper declares model calls of its own; adapters must not run nested agents.",
        ));
    }
    Ok(hello)
}

/// The cache key for the helper binary: source, compiler flags and
/// architecture.
pub fn source_hash() -> String {
    let mut hasher = Sha256::new();
    hasher.update(HELPER_SOURCE.as_bytes());
    for flag in SWIFTC_FLAGS {
        hasher.update(flag.as_bytes());
    }
    hasher.update(std::env::consts::ARCH.as_bytes());
    let digest = hasher.finalize();
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Where the compiled helper lives in `cache_dir`.
pub fn helper_binary(cache_dir: &Path) -> PathBuf {
    cache_dir.join(format!("hame-desktop-helper-{}", source_hash()))
}

/// Returns the compiled helper, compiling it with `swiftc` first if this
/// source has not been compiled into `cache_dir` before. Blocking.
pub fn ensure_helper(cache_dir: &Path, swiftc: &Path) -> Result<PathBuf, HelperError> {
    let binary = helper_binary(cache_dir);
    if binary.is_file() {
        return Ok(binary);
    }
    std::fs::create_dir_all(cache_dir).map_err(|e| {
        HelperError::new(
            "unsupported",
            format!("Cannot create {}: {e}", cache_dir.display()),
        )
    })?;
    let source = cache_dir.join(format!("hame-desktop-helper-{}.swift", source_hash()));
    std::fs::write(&source, HELPER_SOURCE).map_err(|e| {
        HelperError::new(
            "unsupported",
            format!("Cannot write the helper source: {e}"),
        )
    })?;
    let tmp = cache_dir.join(format!(
        ".hame-desktop-helper-{}.{}.tmp",
        source_hash(),
        std::process::id()
    ));
    let output = std::process::Command::new(swiftc)
        .args(SWIFTC_FLAGS)
        .arg("-o")
        .arg(&tmp)
        .arg(&source)
        .output()
        .map_err(|e| {
            HelperError::new(
                "unsupported",
                format!(
                    "Cannot run {} to build the desktop helper: {e}. Install the Xcode command \
                     line tools (xcode-select --install).",
                    swiftc.display()
                ),
            )
        })?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(HelperError::new(
            "unsupported",
            format!(
                "Building the desktop helper failed:\n{}",
                stderr.chars().take(2000).collect::<String>()
            ),
        ));
    }
    // Another process may have finished first; either binary is the same.
    if let Err(e) = std::fs::rename(&tmp, &binary) {
        let _ = std::fs::remove_file(&tmp);
        if !binary.is_file() {
            return Err(HelperError::new(
                "unsupported",
                format!("Cannot install the desktop helper: {e}"),
            ));
        }
    }
    prune_old_helpers(cache_dir);
    Ok(binary)
}

/// Removes helpers built from other sources. A process still running an
/// old one keeps its open file; only the name goes.
fn prune_old_helpers(cache_dir: &Path) {
    let current = format!("hame-desktop-helper-{}", source_hash());
    let Ok(entries) = std::fs::read_dir(cache_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("hame-desktop-helper-") && !name.starts_with(&current) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, HelperError>>>>>;

/// A running helper process.
pub struct Helper {
    tx: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: AtomicU64,
    /// One request at a time; `cancel` bypasses this.
    serial: tokio::sync::Mutex<()>,
    /// The id of the request now running, 0 when idle.
    in_flight: Arc<AtomicU64>,
    alive: Arc<AtomicBool>,
    pub hello: Hello,
    _child: Mutex<tokio::process::Child>,
}

/// Sends `cancel` for a request whose caller stopped waiting (its future
/// was dropped), so the helper does not keep typing for nobody.
struct CancelOnDrop<'a> {
    helper: &'a Helper,
    id: u64,
    armed: bool,
}

impl Drop for CancelOnDrop<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.helper.cancel(self.id);
        }
        let _ =
            self.helper
                .in_flight
                .compare_exchange(self.id, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
}

impl Helper {
    /// Starts `program` and performs the handshake.
    pub async fn start(
        program: &Path,
        args: &[String],
        env: &[(String, String)],
        hello_params: Value,
    ) -> Result<Self, HelperError> {
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            // Closing stdin is how the helper learns to clean up and exit;
            // a kill would skip releasing keys and restoring the clipboard.
            .kill_on_drop(false);
        let mut child = command.spawn().map_err(|e| {
            HelperError::new(
                "unsupported",
                format!("Cannot start the desktop helper: {e}"),
            )
        })?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        {
            let pending = Arc::clone(&pending);
            let alive = Arc::clone(&alive);
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some((id, outcome)) = parse_response(&line) {
                        let waiter = pending
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                        if let Some(waiter) = waiter {
                            let _ = waiter.send(outcome);
                        }
                    }
                }
                alive.store(false, Ordering::SeqCst);
                // Waiters see their sender dropped: the helper exited.
                pending.lock().unwrap_or_else(|e| e.into_inner()).clear();
            });
        }
        let mut helper = Self {
            tx,
            pending,
            next_id: AtomicU64::new(1),
            serial: tokio::sync::Mutex::new(()),
            in_flight: Arc::new(AtomicU64::new(0)),
            alive,
            hello: Hello {
                manifest: AdapterManifest {
                    name: String::new(),
                    version: String::new(),
                    protocol: 0,
                    channel: Channel::Desktop,
                    operations: Vec::new(),
                    platforms: Vec::new(),
                    cancellable: false,
                    nested_usage: NestedUsage::Unknown,
                },
                helper_protocol: 0,
                ax_trusted: false,
                screen_access: false,
                pid: 0,
                os: String::new(),
            },
            _child: Mutex::new(child),
        };
        let mut params = hello_params;
        params["protocol"] = json!(CONTRACT_VERSION);
        let result = helper
            .request("hello", params, Duration::from_secs(20))
            .await
            .map_err(|e| {
                HelperError::new(
                    "protocol",
                    format!("The desktop helper did not complete its handshake: {e}"),
                )
            })?;
        helper.hello = validate_hello(&result)?;
        Ok(helper)
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Asks the helper to stop request `id` at its next checkpoint.
    pub fn cancel(&self, id: u64) {
        let _ = self.tx.send(request_line(
            self.next_id.fetch_add(1, Ordering::SeqCst),
            "cancel",
            &json!({ "target": id }),
        ));
    }

    /// Cancels whatever request is running now, if any.
    pub fn cancel_current(&self) {
        let id = self.in_flight.load(Ordering::SeqCst);
        if id != 0 {
            self.cancel(id);
        }
    }

    /// Sends one request and waits for its response, at most `limit`.
    pub async fn request(
        &self,
        op: &str,
        params: Value,
        limit: Duration,
    ) -> Result<Value, HelperError> {
        let _turn = self.serial.lock().await;
        if !self.is_alive() {
            return Err(HelperError::new(
                "helper_exited",
                "The desktop helper is not running.",
            ));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);
        self.in_flight.store(id, Ordering::SeqCst);
        let mut guard = CancelOnDrop {
            helper: self,
            id,
            armed: true,
        };
        if self.tx.send(request_line(id, op, &params)).is_err() {
            guard.armed = false;
            return Err(HelperError::new(
                "helper_exited",
                "The desktop helper's input is closed.",
            ));
        }
        let outcome = match tokio::time::timeout(limit, rx).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => Err(HelperError::new(
                "helper_exited",
                format!("The desktop helper exited during {op}; its effect is unknown."),
            )),
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                // The guard sends the cancel.
                return Err(HelperError::new(
                    "timeout",
                    format!(
                        "The desktop helper did not finish {op} within {} s; it was asked to stop.",
                        limit.as_secs()
                    ),
                ));
            }
        };
        guard.armed = false;
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello_json(protocol: u32, helper_protocol: u32, nested: &str) -> Value {
        json!({
            "manifest": {
                "name": "hame-desktop-helper",
                "version": "0.1.0",
                "protocol": protocol,
                "channel": "desktop",
                "operations": ["observe"],
                "platforms": ["macos"],
                "cancellable": true,
                "nested_usage": nested,
            },
            "helper_protocol": helper_protocol,
            "ax_trusted": true,
            "screen_access": false,
            "pid": 42,
            "os": "Version 26.6.2",
        })
    }

    #[test]
    fn the_handshake_checks_versions_channel_and_nested_usage() {
        let hello = validate_hello(&hello_json(CONTRACT_VERSION, HELPER_PROTOCOL, "none")).unwrap();
        assert_eq!(hello.manifest.nested_usage, NestedUsage::None);
        assert!(hello.ax_trusted && !hello.screen_access);
        for bad in [
            hello_json(CONTRACT_VERSION + 1, HELPER_PROTOCOL, "none"),
            hello_json(CONTRACT_VERSION, HELPER_PROTOCOL + 1, "none"),
            hello_json(CONTRACT_VERSION, HELPER_PROTOCOL, "unknown"),
            json!({"manifest": {}}),
        ] {
            assert_eq!(validate_hello(&bad).unwrap_err().code, "protocol");
        }
        let mut other_channel = hello_json(CONTRACT_VERSION, HELPER_PROTOCOL, "none");
        other_channel["manifest"]["channel"] = json!("browser");
        assert!(validate_hello(&other_channel).is_err());
    }

    #[test]
    fn requests_and_responses_are_single_json_lines() {
        let line = request_line(7, "set_value", &json!({"value": "Tiếng Việt\nline 2"}));
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["id"], 7);
        assert_eq!(parsed["op"], "set_value");
        assert_eq!(parsed["value"], "Tiếng Việt\nline 2");

        let (id, ok) = parse_response(r#"{"id":3,"ok":true,"result":{"x":1}}"#).unwrap();
        assert_eq!((id, ok.unwrap()), (3, json!({"x": 1})));
        let (id, err) = parse_response(
            r#"{"id":4,"ok":false,"error":{"code":"interference","message":"Finder became frontmost","details":{"emitted":"Tiế","dispatched":true}}}"#,
        )
        .unwrap();
        let err = err.unwrap_err();
        assert_eq!(id, 4);
        assert_eq!(err.code, "interference");
        assert_eq!(err.emitted(), Some("Tiế"));
        assert!(err.dispatched());
        let (_, err) =
            parse_response(r#"{"id":5,"ok":false,"error":{"code":"stale_ref","message":"gone"}}"#)
                .unwrap();
        assert!(!err.unwrap_err().dispatched());
        assert!(parse_response("not json").is_none());
        assert!(parse_response(r#"{"ok":true}"#).is_none());
    }

    #[test]
    fn the_cache_key_follows_the_source() {
        let a = source_hash();
        assert_eq!(a.len(), 16);
        assert_eq!(a, source_hash());
        let dir = Path::new("/cache");
        assert_eq!(
            helper_binary(dir),
            dir.join(format!("hame-desktop-helper-{a}"))
        );
        // Builds of other sources are pruned; the current one and
        // unrelated files stay.
        let cache = tempfile::tempdir().unwrap();
        let current = helper_binary(cache.path());
        for name in [
            "hame-desktop-helper-0000000000000000",
            "hame-desktop-helper-0000000000000000.swift",
            "unrelated",
        ] {
            std::fs::write(cache.path().join(name), b"x").unwrap();
        }
        std::fs::write(&current, b"x").unwrap();
        prune_old_helpers(cache.path());
        let mut left: Vec<String> = std::fs::read_dir(cache.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            vec![format!("hame-desktop-helper-{a}"), "unrelated".into()]
        );
    }

    /// A stand-in helper written in shell: answers the handshake with a
    /// wrong protocol, or exits mid-request.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_wrong_handshake_or_a_dead_helper_is_reported() {
        let wrong = r#"read line; echo '{"id":1,"ok":true,"result":{"manifest":{"name":"x","version":"0","protocol":99,"channel":"desktop","operations":[],"platforms":[],"cancellable":true,"nested_usage":"none"},"helper_protocol":1,"ax_trusted":true,"screen_access":true,"pid":1}}'; cat >/dev/null"#;
        let err = Helper::start(
            Path::new("/bin/sh"),
            &["-c".into(), wrong.into()],
            &[],
            json!({}),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(err.code, "protocol");
        assert!(err.message.contains("99"), "{}", err.message);

        let dies = r#"read line; echo '{"id":1,"ok":true,"result":{"manifest":{"name":"x","version":"0","protocol":1,"channel":"desktop","operations":[],"platforms":[],"cancellable":true,"nested_usage":"none"},"helper_protocol":1,"ax_trusted":true,"screen_access":true,"pid":1}}'; read line; exit 3"#;
        let helper = Helper::start(
            Path::new("/bin/sh"),
            &["-c".into(), dies.into()],
            &[],
            json!({}),
        )
        .await
        .unwrap();
        let err = helper
            .request("type", json!({"text": "x"}), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(err.code, "helper_exited");
        assert!(err.dispatched());
        assert!(!helper.is_alive());
    }
}
