use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::SessionError;
use crate::artifact::{ArtifactError, ArtifactRef, ArtifactStore};
use crate::contract::{ActionJournal, ActionReceipt};
use crate::model::{Event, FORMAT_VERSION, Header, ParentRef, Record, SessionId};
use crate::projection::session_artifacts;
use crate::recover::plan_recovery;

const LOG_FILE: &str = "session.v1.jsonl";
const LOCK_FILE: &str = "lock";

/// Source of record timestamps. Injected so tests and replays are
/// deterministic.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct CreateOptions {
    /// Generated when absent.
    pub id: Option<SessionId>,
    pub cwd: String,
}

/// Sessions on disk: one directory per session under `root`, each holding an
/// append-only JSONL log.
#[derive(Clone)]
pub struct SessionStore {
    root: PathBuf,
    clock: Arc<dyn Clock>,
}

impl SessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_clock(root, Arc::new(SystemClock))
    }

    pub fn with_clock(root: impl Into<PathBuf>, clock: Arc<dyn Clock>) -> Self {
        Self {
            root: root.into(),
            clock,
        }
    }

    /// The directory session directories live in.
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// The artifact store shared by every session in this store, so a fork
    /// or an import keeps its images: `<root>/../artifacts`, or
    /// `<root>/artifacts` when the root has no parent.
    pub fn artifacts(&self) -> crate::ArtifactStore {
        let base = self.root.parent().unwrap_or(&self.root);
        crate::ArtifactStore::new(base.join("artifacts"))
    }

    /// Where a session's action journal lives, beside its log.
    pub fn journal_path(&self, id: &SessionId) -> PathBuf {
        self.root
            .join(id.as_str())
            .join(crate::contract::ACTION_JOURNAL_FILE)
    }

    /// Where the log of session `id` is (whether or not it exists).
    pub fn log_path(&self, id: &SessionId) -> PathBuf {
        self.root.join(id.as_str()).join(LOG_FILE)
    }

    fn journal(&self, id: &SessionId) -> Result<Arc<ActionJournal>, SessionError> {
        Ok(Arc::new(ActionJournal::open(self.journal_path(id))?))
    }

    /// Artifacts the session refers to that cannot be read back intact:
    /// missing from the store, or with bytes that no longer match their
    /// hash. History still derives without them; a provider is sent a note
    /// in place of each.
    pub fn check_artifacts(
        &self,
        id: &SessionId,
    ) -> Result<Vec<(ArtifactRef, ArtifactError)>, SessionError> {
        let records = self.read(id)?;
        let store = self.artifacts();
        Ok(session_artifacts(&records)
            .into_iter()
            .filter_map(|artifact| match store.get(&artifact) {
                Ok(_) => None,
                Err(e) => Some((artifact, e)),
            })
            .collect())
    }

    /// Deletes artifacts that no session refers to and that are older than
    /// `min_age`, and temporary files left by writes that never finished.
    ///
    /// An artifact is written before the event that refers to it, so a
    /// crash in between leaves an unreferenced file; `min_age` keeps this
    /// from deleting one a running session has just written and is about to
    /// log. Every file in every session directory (logs of forks and
    /// imports included, and action journals) is searched for artifact ids,
    /// whatever its format version, so nothing a log mentions is deleted.
    /// A session directory that cannot be read stops the collection.
    pub fn gc_artifacts(&self, min_age: Duration) -> Result<GcReport, SessionError> {
        self.collect_artifacts(min_age, true)
    }

    /// What [`Self::gc_artifacts`] would delete, deleting nothing.
    pub fn gc_artifacts_dry_run(&self, min_age: Duration) -> Result<GcReport, SessionError> {
        self.collect_artifacts(min_age, false)
    }

    fn collect_artifacts(&self, min_age: Duration, delete: bool) -> Result<GcReport, SessionError> {
        let referenced = self.referenced_ids()?;
        let store = self.artifacts();
        let mut report = GcReport::default();
        let shards = match fs::read_dir(store.root()) {
            Ok(shards) => shards,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(report),
            Err(e) => return Err(e.into()),
        };
        let now = SystemTime::now();
        let old_enough = |meta: &fs::Metadata| {
            meta.modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= min_age)
        };
        for shard in shards {
            let shard = shard?.path();
            if !shard.is_dir() {
                continue;
            }
            for entry in fs::read_dir(&shard)? {
                let entry = entry?;
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                let meta = entry.metadata()?;
                if !meta.is_file() {
                    continue;
                }
                if name.starts_with('.') && name.ends_with(".tmp") {
                    if old_enough(&meta) {
                        if delete {
                            fs::remove_file(&path)?;
                        }
                        report.temp_removed += 1;
                    }
                    continue;
                }
                if store.path_of(&name).ok().as_deref() != Some(path.as_path()) {
                    // Not something the store wrote; leave it alone.
                    continue;
                }
                if referenced.contains(&name) || !old_enough(&meta) {
                    report.kept += 1;
                    continue;
                }
                if delete {
                    fs::remove_file(&path)?;
                }
                report.removed += 1;
                report.bytes_freed += meta.len();
            }
            if delete {
                // Only succeeds when the shard is empty.
                let _ = fs::remove_dir(&shard);
            }
        }
        Ok(report)
    }

    /// Every artifact id mentioned in any file of any session directory.
    fn referenced_ids(&self) -> Result<std::collections::HashSet<String>, SessionError> {
        let mut ids = std::collections::HashSet::new();
        let sessions = match fs::read_dir(&self.root) {
            Ok(sessions) => sessions,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(ids),
            Err(e) => return Err(e.into()),
        };
        for session in sessions {
            let dir = session?.path();
            if !dir.is_dir() {
                continue;
            }
            for file in fs::read_dir(&dir)? {
                let path = file?.path();
                if path.is_file() && path.file_name() != Some(LOCK_FILE.as_ref()) {
                    collect_hex_ids(&fs::read(&path)?, &mut ids);
                }
            }
        }
        Ok(ids)
    }

    pub fn create(&self, options: CreateOptions) -> Result<SessionWriter, SessionError> {
        self.create_with_parent(options, None)
    }

    fn create_with_parent(
        &self,
        options: CreateOptions,
        parent: Option<ParentRef>,
    ) -> Result<SessionWriter, SessionError> {
        let id = options.id.unwrap_or_else(SessionId::generate);
        fs::create_dir_all(&self.root)?;
        let dir = self.root.join(id.as_str());
        match fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                return Err(SessionError::AlreadyExists(id));
            }
            Err(e) => return Err(e.into()),
        }
        let held = lock(&dir, &id)?;
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create_new(true)
            .open(dir.join(LOG_FILE))?;
        let mut writer = SessionWriter {
            file,
            _lock: held,
            id: id.clone(),
            next_seq: 0,
            clock: Arc::clone(&self.clock),
            artifacts: self.artifacts(),
            journal: self.journal(&id)?,
            receipts_seen: 0,
        };
        let ts = writer.clock.now_ms();
        writer.write(
            ts,
            Event::Session(Header {
                version: FORMAT_VERSION,
                id,
                cwd: options.cwd,
                parent,
            }),
            true,
        )?;
        // Make the new directory entries durable, not just the file contents.
        sync_dir(&dir);
        sync_dir(&self.root);
        Ok(writer)
    }

    /// Reads a session without locking it. A record still being written by
    /// another process is not returned.
    pub fn read(&self, id: &SessionId) -> Result<Vec<Record>, SessionError> {
        let bytes = match fs::read(self.log_path(id)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(SessionError::NotFound(id.clone()));
            }
            Err(e) => return Err(e.into()),
        };
        Ok(parse_log(&bytes)?.records)
    }

    /// Marks the log as written in this build's format, before an event
    /// only this version knows (such as `mode/changed`) is appended to an
    /// older log: an older build then refuses the log as newer instead of
    /// reporting it as corrupt. Only the header record is rewritten, into a
    /// temporary file that is synced and renamed over the log, under the
    /// session's lock (`Locked` while the session is open elsewhere).
    /// Returns whether anything changed.
    pub fn upgrade_format(&self, id: &SessionId) -> Result<bool, SessionError> {
        let dir = self.root.join(id.as_str());
        let _held = lock(&dir, id)?;
        let path = self.log_path(id);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(SessionError::NotFound(id.clone()));
            }
            Err(e) => return Err(e.into()),
        };
        let corrupt = |reason: String| SessionError::Corrupt { line: 1, reason };
        let end = bytes
            .iter()
            .position(|b| *b == b'\n')
            .ok_or_else(|| corrupt("the header is incomplete".into()))?;
        let mut record: Record =
            serde_json::from_slice(&bytes[..end]).map_err(|e| corrupt(e.to_string()))?;
        let Event::Session(header) = &mut record.event else {
            return Err(corrupt("the first record is not the header".into()));
        };
        if header.version >= FORMAT_VERSION {
            return Ok(false);
        }
        header.version = FORMAT_VERSION;
        let mut upgraded = serde_json::to_vec(&record).map_err(io::Error::other)?;
        upgraded.extend_from_slice(&bytes[end..]);
        let temp = dir.join(format!("{LOG_FILE}.upgrade.tmp"));
        {
            let mut file = File::create(&temp)?;
            file.write_all(&upgraded)?;
            file.sync_all()?;
        }
        fs::rename(&temp, &path)?;
        sync_dir(&dir);
        Ok(true)
    }

    /// Opens a session for appending, taking its exclusive lock.
    ///
    /// A partially written last record is discarded, and anything the log
    /// left open (see [`plan_recovery`]) is closed, so the returned records
    /// always describe a well-formed session with no turn in progress.
    ///
    /// Actions the journal shows as dispatched but never settled are marked
    /// `outcome_unknown` there, and every settled receipt the log does not
    /// have yet is appended to it, before the open turn is closed; the
    /// interrupted tool calls' results then name those actions. Nothing is
    /// ever replayed.
    pub fn open(&self, id: &SessionId) -> Result<(Vec<Record>, SessionWriter), SessionError> {
        // The lock comes first, so two processes cannot both repair and
        // extend the log.
        let held = lock(&self.root.join(id.as_str()), id)?;
        let mut file = match OpenOptions::new()
            .read(true)
            .append(true)
            .open(self.log_path(id))
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(SessionError::NotFound(id.clone()));
            }
            Err(e) => return Err(e.into()),
        };

        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let parsed = parse_log(&bytes)?;
        if parsed.valid_len < bytes.len() as u64 {
            // Cut off the torn tail through its own handle: a handle
            // opened for appending is not allowed to truncate on Windows.
            let cutter = OpenOptions::new().write(true).open(self.log_path(id))?;
            cutter.set_len(parsed.valid_len)?;
            cutter.sync_data()?;
        }

        let mut records = parsed.records;
        let mut writer = SessionWriter {
            file,
            _lock: held,
            id: id.clone(),
            next_seq: records.len() as u64,
            clock: Arc::clone(&self.clock),
            artifacts: self.artifacts(),
            journal: self.journal(id)?,
            receipts_seen: 0,
        };
        writer.journal.settle_after_crash(writer.clock.now_ms())?;
        let journal = ActionJournal::read(writer.journal.path())?;
        for receipt in unlogged_receipts(&records, &journal) {
            records.push(writer.append(Event::ActionReceipt {
                receipt: Box::new(receipt),
            })?);
        }
        writer.receipts_seen = journal.len();
        for event in plan_recovery(&records) {
            records.push(writer.append(event)?);
        }
        Ok((records, writer))
    }

    /// Starts a new session from the first `at_seq + 1` records of `source`.
    ///
    /// Copied records keep their sequence numbers and timestamps. Forking in
    /// the middle of a turn closes that turn in the fork as interrupted.
    ///
    /// The fork shares the artifact store, so its images stay readable. It
    /// also shares the world: forking undoes no external effect. Every
    /// settled receipt of the source's journal is copied into the fork's
    /// journal, so an adapter still refuses to repeat an effect that may
    /// have happened, and the ones the copied records do not show (made
    /// after the fork point) are appended to the fork's log.
    pub fn fork(
        &self,
        source: &SessionId,
        at_seq: u64,
        options: CreateOptions,
    ) -> Result<(Vec<Record>, SessionWriter), SessionError> {
        let source_records = self.read(source)?;
        let last_seq = source_records.len() as u64 - 1;
        if at_seq > last_seq {
            return Err(SessionError::ForkOutOfRange { at_seq, last_seq });
        }
        let mut writer = self.create_with_parent(
            options,
            Some(ParentRef {
                session: source.clone(),
                seq: at_seq,
            }),
        )?;
        for record in &source_records[1..=at_seq as usize] {
            writer.write(record.ts, record.event.clone(), false)?;
        }
        writer.file.sync_data()?;

        let mut records = parse_log(&fs::read(self.log_path(&writer.id))?)?.records;
        for event in plan_recovery(&records) {
            records.push(writer.append(event)?);
        }
        let carried: Vec<ActionReceipt> = ActionJournal::read(&self.journal_path(source))?
            .into_iter()
            .filter(|receipt| receipt.status.is_settled())
            .collect();
        for receipt in &carried {
            writer.journal.record(receipt)?;
        }
        for receipt in unlogged_receipts(&records, &carried) {
            records.push(writer.append(Event::ActionReceipt {
                receipt: Box::new(receipt),
            })?);
        }
        writer.receipts_seen = carried.len();
        Ok((records, writer))
    }

    /// Headers of every readable session, oldest id first. Directories that
    /// do not hold a readable log of a supported version are skipped.
    pub fn list(&self) -> Result<Vec<Header>, SessionError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut headers = Vec::new();
        for entry in entries {
            let path = entry?.path().join(LOG_FILE);
            if let Some(header) = read_header(&path) {
                headers.push(header);
            }
        }
        headers.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(headers)
    }
}

/// What [`SessionStore::gc_artifacts`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Unreferenced artifacts deleted.
    pub removed: usize,
    pub bytes_freed: u64,
    /// Artifacts kept: referenced, or too recent to judge.
    pub kept: usize,
    /// Temporary files of interrupted writes deleted.
    pub temp_removed: usize,
}

/// Collects every run of exactly 64 lowercase hex digits in `bytes`.
fn collect_hex_ids(bytes: &[u8], ids: &mut std::collections::HashSet<String>) {
    let hex = |b: &u8| b.is_ascii_digit() || (b'a'..=b'f').contains(b);
    let mut start = None;
    for (index, byte) in bytes.iter().chain(std::iter::once(&b' ')).enumerate() {
        match (hex(byte), start) {
            (true, None) => start = Some(index),
            (false, Some(from)) => {
                if index - from == 64 {
                    ids.insert(String::from_utf8_lossy(&bytes[from..index]).into_owned());
                }
                start = None;
            }
            _ => {}
        }
    }
}

/// Settled receipts in `journal` that `records` do not already hold.
fn unlogged_receipts(records: &[Record], journal: &[ActionReceipt]) -> Vec<ActionReceipt> {
    let mut logged: Vec<&ActionReceipt> = records
        .iter()
        .filter_map(|record| match &record.event {
            Event::ActionReceipt { receipt } => Some(&**receipt),
            _ => None,
        })
        .collect();
    let mut missing = Vec::new();
    for receipt in journal.iter().filter(|r| r.status.is_settled()) {
        match logged.iter().position(|seen| *seen == receipt) {
            Some(index) => {
                logged.swap_remove(index);
            }
            None => missing.push(receipt.clone()),
        }
    }
    missing
}

/// The only handle that can append to a session. Holds the session's
/// exclusive lock until dropped.
pub struct SessionWriter {
    file: File,
    /// Holds the session's exclusive lock for as long as the writer lives.
    _lock: File,
    id: SessionId,
    next_seq: u64,
    clock: Arc<dyn Clock>,
    artifacts: ArtifactStore,
    journal: Arc<ActionJournal>,
    /// Journal receipts already looked at by [`Self::take_settled_receipts`].
    receipts_seen: usize,
}

impl SessionWriter {
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    /// The artifact store this session's images and files are kept in,
    /// shared with every session of the same store.
    pub fn artifacts(&self) -> &ArtifactStore {
        &self.artifacts
    }

    /// The session's action journal, for tools that act outside the
    /// sandbox.
    pub fn journal(&self) -> &Arc<ActionJournal> {
        &self.journal
    }

    /// Receipts that settled in the journal since the last call (or since
    /// the session was opened), in journal order. The caller appends them
    /// to the log as `action/receipt` events.
    pub fn take_settled_receipts(&mut self) -> Result<Vec<ActionReceipt>, SessionError> {
        let all = ActionJournal::read(self.journal.path())?;
        let fresh = all
            .iter()
            .skip(self.receipts_seen)
            .filter(|receipt| receipt.status.is_settled())
            .cloned()
            .collect();
        self.receipts_seen = all.len();
        Ok(fresh)
    }

    /// Marks actions that were dispatched and will now never be seen to
    /// settle (their tool call was abandoned) as `outcome_unknown`, then
    /// returns every receipt settled since the last look, as
    /// [`Self::take_settled_receipts`] does.
    pub fn settle_abandoned_actions(&mut self) -> Result<Vec<ActionReceipt>, SessionError> {
        self.journal.settle_after_crash(self.clock.now_ms())?;
        self.take_settled_receipts()
    }

    /// Sequence number the next appended record will get.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Appends one event. Settled events are fsynced before this returns;
    /// stream deltas are handed to the OS without an fsync.
    pub fn append(&mut self, event: Event) -> Result<Record, SessionError> {
        match &event {
            Event::Session(_) => {
                return Err(SessionError::InvalidEvent(
                    "the session header is written once, at creation",
                ));
            }
            Event::Compaction { up_to_seq, .. } if *up_to_seq >= self.next_seq => {
                return Err(SessionError::InvalidEvent(
                    "a compaction cannot cover records that do not exist yet",
                ));
            }
            _ => {}
        }
        let sync = event.is_settled();
        self.write(self.clock.now_ms(), event, sync)
    }

    fn write(&mut self, ts: u64, event: Event, sync: bool) -> Result<Record, SessionError> {
        let record = Record {
            seq: self.next_seq,
            ts,
            event,
        };
        let mut line = serde_json::to_vec(&record).map_err(io::Error::from)?;
        line.push(b'\n');
        // One write per record: a crash leaves either the whole line or a
        // prefix without its newline, which `parse_log` treats as absent.
        self.file.write_all(&line)?;
        if sync {
            self.file.sync_data()?;
        }
        self.next_seq += 1;
        Ok(record)
    }
}

struct ParsedLog {
    records: Vec<Record>,
    /// Byte length of the complete records; anything after is a torn tail.
    valid_len: u64,
}

fn parse_log(bytes: &[u8]) -> Result<ParsedLog, SessionError> {
    let mut records: Vec<Record> = Vec::new();
    let mut offset = 0;
    while let Some(len) = bytes[offset..].iter().position(|b| *b == b'\n') {
        let line = records.len() + 1;
        let record: Record = serde_json::from_slice(&bytes[offset..offset + len]).map_err(|e| {
            SessionError::Corrupt {
                line,
                reason: e.to_string(),
            }
        })?;
        if record.seq != records.len() as u64 {
            return Err(SessionError::Corrupt {
                line,
                reason: format!("expected seq {}, found {}", records.len(), record.seq),
            });
        }
        match (&record.event, records.is_empty()) {
            (Event::Session(header), true) => {
                if header.version > FORMAT_VERSION {
                    return Err(SessionError::UnsupportedVersion {
                        found: header.version,
                        supported: FORMAT_VERSION,
                    });
                }
            }
            (Event::Session(_), false) => {
                return Err(SessionError::Corrupt {
                    line,
                    reason: "session header repeated".into(),
                });
            }
            (_, true) => {
                return Err(SessionError::Corrupt {
                    line,
                    reason: "first record is not a session header".into(),
                });
            }
            (_, false) => {}
        }
        offset += len + 1;
        records.push(record);
    }
    if records.is_empty() {
        return Err(SessionError::Corrupt {
            line: 1,
            reason: "missing session header".into(),
        });
    }
    Ok(ParsedLog {
        records,
        valid_len: offset as u64,
    })
}

fn read_header(path: &Path) -> Option<Header> {
    let mut line = String::new();
    BufReader::new(File::open(path).ok()?)
        .read_line(&mut line)
        .ok()?;
    if !line.ends_with('\n') {
        return None;
    }
    match serde_json::from_str::<Record>(&line).ok()?.event {
        Event::Session(header) if header.version <= FORMAT_VERSION => Some(header),
        _ => None,
    }
}

/// Takes the session's exclusive lock.
///
/// The lock is on a file of its own, not on the log: where file locks are
/// mandatory (Windows), a locked log could not be read by anyone else, and
/// readers must never be blocked.
fn lock(dir: &Path, id: &SessionId) -> Result<File, SessionError> {
    let file = match OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(SessionError::NotFound(id.clone()));
        }
        Err(e) => return Err(e.into()),
    };
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(SessionError::Locked(id.clone())),
        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

/// Best effort: not every platform can open or sync a directory.
fn sync_dir(dir: &Path) {
    if let Ok(handle) = File::open(dir) {
        let _ = handle.sync_all();
    }
}
