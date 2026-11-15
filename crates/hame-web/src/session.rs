//! A session as the web server holds it: its event channel, approval
//! questions and grants, its queue of messages for the running turn, and
//! (only while turns run, and for a while after) the agent and the
//! session's write lock.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use hame_agent::{Session, Steering, ToolRequest};
use hame_app::{Prompter, Runtime};
use hame_kernel::BoxFuture;
use hame_session::{ArtifactRef, Block, Record, TurnEndReason};
use hame_tools::Effect;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Notify, broadcast, oneshot};

use crate::meta::Mode;

/// One event for the pages watching a session. Records carry their `seq`,
/// which becomes the SSE event id, so a page that reconnects resumes after
/// the last record it saw.
#[derive(Clone)]
pub struct Outgoing {
    pub seq: Option<u64>,
    pub data: Arc<str>,
}

impl Outgoing {
    pub fn event(event: &Value) -> Self {
        Self {
            seq: None,
            data: event.to_string().into(),
        }
    }

    pub fn record(record: &Record) -> Self {
        Self {
            seq: Some(record.seq),
            data: json!({"kind": "record", "record": record})
                .to_string()
                .into(),
        }
    }
}

/// An approval question waiting for an answer.
pub struct Waiting {
    pub answer: oneshot::Sender<Option<bool>>,
    /// The question as it was sent, for pages that connect later.
    pub event: Value,
    pub tool: String,
    pub tool_call_id: String,
    /// `None` when the arguments have no canonical JSON form.
    pub args_sha256: Option<String>,
}

/// What the pages watching a session and its agent share.
pub struct Channel {
    pub events: broadcast::Sender<Outgoing>,
    pub approvals: Mutex<BTreeMap<u64, Waiting>>,
    next_approval: AtomicU64,
    /// Tools the user allowed for the rest of this session (until the mode
    /// changes or the grant is revoked).
    pub always: Mutex<BTreeSet<String>>,
    pub mode: Mutex<Mode>,
    /// The session's folder, which "Accept edits" keeps writes inside.
    pub cwd: PathBuf,
    /// Set while a remote device drives the running turns: it started them,
    /// or a message it queued has been handed to them. Session-wide grants
    /// given on this computer do not answer for such turns.
    pub remote_run: AtomicBool,
    /// Who queued each waiting message, in queue order: `None` for this
    /// computer, the device's label for a remote one. Kept in step with the
    /// steering queue (see [`Channel::settle_origins`]).
    pub origins: Mutex<Vec<Option<String>>>,
}

impl Channel {
    pub fn new(mode: Mode, cwd: PathBuf) -> Self {
        Self {
            events: broadcast::channel(4096).0,
            approvals: Mutex::new(BTreeMap::new()),
            next_approval: AtomicU64::new(1),
            always: Mutex::new(BTreeSet::new()),
            mode: Mutex::new(mode),
            cwd,
            remote_run: AtomicBool::new(false),
            origins: Mutex::new(Vec::new()),
        }
    }

    /// Brings `origins` in step with a queue that now holds `waiting`
    /// messages. The agent takes the whole queue at once and new messages
    /// join at the end, so the first `len - waiting` origins are those of
    /// messages it has taken: if one came from a remote device, the turns
    /// that received it are remote-driven from then on.
    pub fn settle_origins(&self, origins: &mut Vec<Option<String>>, waiting: usize) {
        let taken = origins.len().saturating_sub(waiting);
        if origins[..taken].iter().any(Option::is_some) {
            self.remote_run.store(true, Ordering::SeqCst);
        }
        origins.drain(..taken);
    }

    /// Whether questions now come from turns a remote device drives, or may
    /// soon: one it started or fed, or one a remote message still waits for.
    pub fn remote_driven(&self) -> bool {
        self.remote_run.load(Ordering::SeqCst)
            || self
                .origins
                .lock()
                .expect("origins")
                .iter()
                .any(Option::is_some)
    }

    pub fn send(&self, event: Value) {
        // No page listening is fine: the log has everything.
        let _ = self.events.send(Outgoing::event(&event));
    }

    pub fn send_record(&self, record: &Record) {
        let _ = self.events.send(Outgoing::record(record));
    }

    /// The queue and who queued each message.
    pub fn send_steering(&self, steering: &Steering) {
        let mut origins = self.origins.lock().expect("origins");
        let queued = steering.pending();
        self.settle_origins(&mut origins, queued.len());
        let from = origins.clone();
        drop(origins);
        self.send(json!({"kind": "steering", "queued": queued, "origins": from}));
    }

    /// Every question still waiting, in the order asked.
    pub fn pending(&self) -> Vec<Value> {
        self.approvals
            .lock()
            .expect("approvals")
            .values()
            .map(|waiting| waiting.event.clone())
            .collect()
    }

    /// The whole list of waiting questions, which replaces what a page has.
    pub fn send_approvals(&self) {
        self.send(json!({"kind": "approvals", "pending": self.pending()}));
    }

    pub fn grants(&self) -> Vec<String> {
        self.always
            .lock()
            .expect("always")
            .iter()
            .cloned()
            .collect()
    }

    pub fn send_grants(&self) {
        self.send(json!({"kind": "grants", "grants": self.grants()}));
    }
}

/// Stops one run of turns. Set before the run starts, so a stop asked for
/// at any moment of it (even before the first turn has begun, or between a
/// turn and the one its queued messages start) is honoured.
#[derive(Default)]
pub struct TurnCancel {
    stopped: AtomicBool,
    notify: Notify,
}

impl TurnCancel {
    pub fn cancel(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    pub async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

#[derive(Default)]
pub struct Uploads {
    pub images: HashMap<String, ArtifactRef>,
    pub files: HashSet<String>,
}

/// The agent of a session and the session's write lock.
pub struct Live {
    pub session: Session,
    pub runtime: Runtime,
    pub idle_since: Instant,
}

pub type LiveSlot = Arc<tokio::sync::Mutex<Option<Live>>>;

pub struct Entry {
    pub id: String,
    pub cwd: PathBuf,
    pub channel: Arc<Channel>,
    /// `None` while the session is only viewed: then no lock is held and a
    /// terminal can resume it.
    pub live: LiveSlot,
    pub busy: AtomicBool,
    /// Held while `busy` changes and while a message is queued, so a
    /// message is never queued for a turn that is already over.
    pub gate: Mutex<()>,
    pub steering: Steering,
    pub cancel: Mutex<Option<Arc<TurnCancel>>>,
    pub uploads: Mutex<Uploads>,
    /// Why the last run of turns failed, for a page that connects later.
    pub last_error: Mutex<Option<String>>,
    pub deleted: AtomicBool,
}

impl Entry {
    pub fn new(id: String, cwd: PathBuf, mode: Mode) -> Self {
        Self {
            channel: Arc::new(Channel::new(mode, cwd.clone())),
            id,
            cwd,
            live: Arc::new(tokio::sync::Mutex::new(None)),
            busy: AtomicBool::new(false),
            gate: Mutex::new(()),
            steering: Steering::default(),
            cancel: Mutex::new(None),
            uploads: Mutex::new(Uploads::default()),
            last_error: Mutex::new(None),
            deleted: AtomicBool::new(false),
        }
    }
}

pub fn effect_name(effect: Effect) -> &'static str {
    match effect {
        Effect::ReadOnly => "read-only",
        Effect::Write => "write",
        Effect::Execute => "execute",
        Effect::Control => "control",
    }
}

/// `sha256` (lowercase hex) of the arguments' canonical JSON (RFC 8785),
/// which an answer must name (APPS-AND-SYNC.md §4); the relay connector and
/// devices compute the same. `None` for arguments with no canonical form
/// (integers beyond 2^53).
pub fn args_sha256(arguments: &Value) -> Option<String> {
    let canonical = crate::jcs::canonical(arguments).ok()?;
    Some(
        Sha256::digest(canonical.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// `path` with `.` and `..` worked out without touching the disk.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Whether `path` (relative to `cwd`, or absolute) is inside `cwd` once
/// links in the part that exists are followed. A path with a `..`
/// component never counts as inside: where `link/..` leads depends on the
/// link, so such a write is asked about.
pub fn inside(cwd: &Path, path: &str) -> bool {
    if Path::new(path)
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return false;
    }
    let Ok(root) = hame_kernel::real_path(cwd) else {
        return false;
    };
    let target = lexical(&cwd.join(path));
    // Follow links in the longest part that exists; the rest is new.
    let mut existing = target.clone();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => return false,
        }
    }
    let Ok(mut real) = hame_kernel::real_path(&existing) else {
        return false;
    };
    for name in rest.into_iter().rev() {
        real.push(name);
    }
    real.starts_with(&root)
}

/// The files a write call names: `path`/`file_path`, or the files of an
/// `apply_patch` patch.
pub fn written_paths(request: &ToolRequest) -> Vec<String> {
    let input = &request.input;
    let mut paths = Vec::new();
    for key in ["path", "file_path"] {
        if let Some(path) = input.get(key).and_then(Value::as_str) {
            paths.push(path.to_string());
        }
    }
    if let Some(patch) = input.get("patch").and_then(Value::as_str) {
        for line in patch.lines() {
            for prefix in [
                "*** Update File: ",
                "*** Add File: ",
                "*** Delete File: ",
                "*** Move to: ",
            ] {
                if let Some(path) = line.strip_prefix(prefix) {
                    paths.push(path.trim().to_string());
                }
            }
        }
    }
    paths
}

pub struct PagePrompter(pub Arc<Channel>);

impl Prompter for PagePrompter {
    fn ask<'a>(
        &'a self,
        request: &'a ToolRequest,
        question: String,
    ) -> BoxFuture<'a, Option<bool>> {
        Box::pin(async move {
            let channel = &self.0;
            let mode = *channel.mode.lock().expect("mode");
            // Accept edits: only file changes, and only inside the folder.
            if mode == Mode::AcceptEdits && request.effect == Effect::Write {
                let paths = written_paths(request);
                if !paths.is_empty() && paths.iter().all(|path| inside(&channel.cwd, path)) {
                    return Some(true);
                }
            }
            if !channel.remote_driven()
                && channel
                    .always
                    .lock()
                    .expect("always")
                    .contains(&request.name)
            {
                return Some(true);
            }
            let id = channel.next_approval.fetch_add(1, Ordering::Relaxed);
            let (answer, answered) = oneshot::channel();
            // The tool call and its exact arguments, so a remote device can
            // show what it approves, and the hash an answer must name.
            let hash = args_sha256(&request.input);
            let event = json!({
                "kind": "approval",
                "id": id,
                "question": question,
                "tool": request.name,
                "tool_call_id": request.call_id,
                "arguments": request.input,
                "args_sha256": hash,
                "effect": effect_name(request.effect),
                "scope": format!("{} for the rest of this session", request.name),
            });
            channel.approvals.lock().expect("approvals").insert(
                id,
                Waiting {
                    answer,
                    event: event.clone(),
                    tool: request.name.clone(),
                    tool_call_id: request.call_id.clone(),
                    args_sha256: hash,
                },
            );
            channel.send(event);
            // A page that goes away without answering has not approved.
            answered.await.ok().flatten()
        })
    }
}

/// Runs a turn, then another for messages queued too late for it, until
/// none are left or the run is stopped. Holds the session's agent for the
/// whole run.
pub async fn run_turns(
    entry: Arc<Entry>,
    mut live: tokio::sync::OwnedMutexGuard<Option<Live>>,
    mut content: Vec<Block>,
    cancel: Arc<TurnCancel>,
) {
    loop {
        let mut error = None;
        if !cancel.is_cancelled() {
            entry
                .channel
                .send(json!({"kind": "turn", "state": "started"}));
            let state = live.as_mut().expect("the run holds the agent");
            let outcome = tokio::select! {
                result = state.runtime.agent.run_turn(&mut state.session, content) => Some(result),
                _ = cancel.cancelled() => None,
            };
            error = match outcome {
                Some(Ok(())) => None,
                Some(Err(e)) => Some(e.to_string()),
                None => state
                    .session
                    .close_open_turn(TurnEndReason::Cancelled)
                    .err()
                    .map(|e| e.to_string()),
            };
            state.idle_since = Instant::now();
            // Nobody is left to answer questions from a turn that is over.
            entry.channel.approvals.lock().expect("approvals").clear();
            entry.channel.send_approvals();
        }
        let gate = entry.gate.lock().expect("gate");
        if error.is_none() && !cancel.is_cancelled() {
            let late = entry.steering.take();
            if !late.is_empty() {
                drop(gate);
                entry.channel.send_steering(&entry.steering);
                entry
                    .channel
                    .send(json!({"kind": "turn", "state": "ended", "error": null}));
                content = vec![Block::text(late.join("\n\n"))];
                continue;
            }
        }
        // What stays queued after a failure or a stop is kept, shown, and
        // handed to the next turn; it is never dropped.
        *entry.last_error.lock().expect("last error") = error.clone();
        *entry.cancel.lock().expect("cancel") = None;
        entry.channel.remote_run.store(false, Ordering::SeqCst);
        entry.busy.store(false, Ordering::SeqCst);
        drop(gate);
        drop(live);
        entry.channel.send_steering(&entry.steering);
        entry
            .channel
            .send(json!({"kind": "turn", "state": "ended", "error": error}));
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_inside_the_folder_are_told_apart_from_ones_outside() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(work.join("src")).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        assert!(inside(&work, "src/lib.rs"));
        assert!(inside(&work, "new/dir/file.txt"));
        assert!(
            !inside(&work, "./src/../README.md"),
            "`..` is always asked about"
        );
        assert!(!inside(&work, "../outside/x"));
        assert!(!inside(&work, &outside.join("x").to_string_lossy()));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, work.join("link")).unwrap();
            assert!(
                !inside(&work, "link/x"),
                "a link out of the folder is outside"
            );
            std::fs::create_dir_all(outside.join("sub")).unwrap();
            std::os::unix::fs::symlink(outside.join("sub"), work.join("deep")).unwrap();
            // Lexically inside, but the system follows the link first.
            assert!(!inside(&work, "deep/../escaped.txt"));
        }
    }

    #[test]
    fn origins_follow_the_queue_and_a_taken_remote_message_marks_the_run() {
        let channel = Channel::new(Mode::Ask, PathBuf::from("."));
        let mut origins = vec![None, Some("phone".to_string())];
        // Nothing taken: unchanged.
        channel.settle_origins(&mut origins, 2);
        assert_eq!(origins.len(), 2);
        assert!(!channel.remote_run.load(Ordering::SeqCst));
        // The agent took both; a new local one waits.
        origins.push(None);
        channel.settle_origins(&mut origins, 1);
        assert_eq!(origins, vec![None]);
        assert!(channel.remote_run.load(Ordering::SeqCst));
        // A remote message still waiting keeps questions remote.
        channel.remote_run.store(false, Ordering::SeqCst);
        *channel.origins.lock().unwrap() = vec![Some("phone".to_string())];
        assert!(channel.remote_driven());
        channel.origins.lock().unwrap().clear();
        assert!(!channel.remote_driven());
    }

    #[tokio::test]
    async fn a_stop_asked_for_before_anyone_waits_is_not_lost() {
        let cancel = TurnCancel::default();
        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), cancel.cancelled())
            .await
            .expect("cancelled returns at once");
    }
}
