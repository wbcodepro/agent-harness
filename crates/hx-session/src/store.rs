use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::SessionError;
use crate::model::{Event, FORMAT_VERSION, Header, ParentRef, Record, SessionId};
use crate::recover::plan_recovery;

const LOG_FILE: &str = "session.v1.jsonl";

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

    fn log_path(&self, id: &SessionId) -> PathBuf {
        self.root.join(id.as_str()).join(LOG_FILE)
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
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create_new(true)
            .open(dir.join(LOG_FILE))?;
        lock(&file, &id)?;
        let mut writer = SessionWriter {
            file,
            id: id.clone(),
            next_seq: 0,
            clock: Arc::clone(&self.clock),
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

    /// Opens a session for appending, taking its exclusive lock.
    ///
    /// A partially written last record is discarded, and anything the log
    /// left open (see [`plan_recovery`]) is closed, so the returned records
    /// always describe a well-formed session with no turn in progress.
    pub fn open(&self, id: &SessionId) -> Result<(Vec<Record>, SessionWriter), SessionError> {
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
        lock(&file, id)?;

        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let parsed = parse_log(&bytes)?;
        if parsed.valid_len < bytes.len() as u64 {
            file.set_len(parsed.valid_len)?;
            file.sync_data()?;
        }

        let mut records = parsed.records;
        let mut writer = SessionWriter {
            file,
            id: id.clone(),
            next_seq: records.len() as u64,
            clock: Arc::clone(&self.clock),
        };
        for event in plan_recovery(&records) {
            records.push(writer.append(event)?);
        }
        Ok((records, writer))
    }

    /// Starts a new session from the first `at_seq + 1` records of `source`.
    ///
    /// Copied records keep their sequence numbers and timestamps. Forking in
    /// the middle of a turn closes that turn in the fork as interrupted.
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

/// The only handle that can append to a session. Holds the session's
/// exclusive lock until dropped.
pub struct SessionWriter {
    file: File,
    id: SessionId,
    next_seq: u64,
    clock: Arc<dyn Clock>,
}

impl SessionWriter {
    pub fn id(&self) -> &SessionId {
        &self.id
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

fn lock(file: &File, id: &SessionId) -> Result<(), SessionError> {
    match file.try_lock() {
        Ok(()) => Ok(()),
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
