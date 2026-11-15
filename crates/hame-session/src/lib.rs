//! The session log: the single source of truth for an hame session.
//!
//! A session is an append-only sequence of [`Event`]s stored as JSONL. What
//! the model sees is a projection of that log ([`derive_history`]), never a
//! separate mutable transcript, so resume, fork, compaction and replay all
//! work from the same durable facts.

pub mod artifact;
pub mod contract;
mod model;
mod projection;
mod recover;
mod store;

pub use artifact::{ArtifactRef, ArtifactSource, ArtifactStore};
pub use model::{
    AttemptReason, Block, Delta, Event, FORMAT_VERSION, Header, ParentRef, Record, SessionId,
    StopReason, ToolSpec, TurnEndReason, Usage, assemble, blocks_text,
};
pub use projection::{
    History, HistoryOptions, KEEP_IMAGES, MASK_MIN_BYTES, MASK_NOTE_BYTES, Message, Role,
    derive_history, derive_history_with, image_note, session_artifacts,
};
pub use recover::{
    INTERRUPTED_ACTIONS, INTERRUPTED_NOT_STARTED, INTERRUPTED_RUNNING, describe_uncertain_actions,
    plan_close, plan_recovery,
};
pub use store::{Clock, CreateOptions, GcReport, SessionStore, SessionWriter, SystemClock};

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("session log is corrupt at line {line}: {reason}")]
    Corrupt { line: usize, reason: String },
    #[error("session format version {found} is newer than this build supports ({supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("session {0} is open in another process")]
    Locked(SessionId),
    #[error("session {0} not found")]
    NotFound(SessionId),
    #[error("session {0} already exists")]
    AlreadyExists(SessionId),
    #[error("invalid session id {0:?}: use 1-64 characters from A-Z, a-z, 0-9, '_' and '-'")]
    InvalidId(String),
    #[error("invalid event: {0}")]
    InvalidEvent(&'static str),
    #[error("cannot fork at seq {at_seq}: the session ends at seq {last_seq}")]
    ForkOutOfRange { at_seq: u64, last_seq: u64 },
}
