//! Approval binding (§4): a remote answer to an approval question must name
//! the tool call id and the sha256 of its exact arguments, and both must
//! match the question the host asked. An answer naming anything else is
//! refused before it reaches the agent.
//!
//! The hame web API's approval event carries the tool call id and the
//! arguments; the connector adds `args_sha256`, the sha256 of the arguments'
//! canonical JSON (RFC 8785), before relaying it. A device shows the
//! arguments, recomputes the hash from them and sends it back.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::keys::hex;

/// Questions kept at most; the oldest are forgotten first (an answer to a
/// forgotten question is refused, which is the safe choice).
const MAX_PENDING: usize = 1024;

/// `sha256` (lowercase hex) of the arguments' canonical JSON (RFC 8785).
/// None for arguments that have no canonical form (integers beyond 2^53).
pub fn args_sha256(arguments: &Value) -> Option<String> {
    let canonical = crate::jcs::canonical(arguments).ok()?;
    Some(hex(&Sha256::digest(canonical.as_bytes())))
}

/// The session id in `/api/sessions/{id}/approval`, if `path` is one.
/// The query string is ignored, as the router ignores it.
pub fn approval_session(path: &str) -> Option<&str> {
    let path = path.split('?').next().unwrap_or(path);
    let rest = path.strip_prefix("/api/sessions/")?;
    let (session, tail) = rest.split_once('/')?;
    (tail == "approval" && !session.is_empty()).then_some(session)
}

/// Whether a path might reach an approval route in a form
/// [`approval_session`] does not recognise (encoded characters, extra
/// segments): such a request is refused rather than passed on unchecked.
pub fn looks_like_approval(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path).to_ascii_lowercase();
    path.contains("approval") || path.contains('%')
}

/// The session id in `/api/sessions/{id}/events`, if `path` is one.
pub fn events_session(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/api/sessions/")?;
    let (session, tail) = rest.split_once('/')?;
    let tail = tail.split('?').next().unwrap_or(tail);
    (tail == "events" && !session.is_empty()).then_some(session)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Pending {
    tool_call_id: String,
    args_sha256: String,
    order: u64,
}

#[derive(Default)]
struct Table {
    questions: HashMap<(String, u64), Pending>,
    next: u64,
}

/// The approval questions the host has shown remote devices.
#[derive(Default)]
pub struct PendingApprovals {
    inner: Mutex<Table>,
}

impl PendingApprovals {
    /// Notes an event of `session`'s stream; approval questions are kept.
    /// Notes an event of `session`'s stream. An approval question gets its
    /// `args_sha256` added (any value already there is replaced) and is kept;
    /// one without a tool call id or arguments cannot be answered remotely.
    pub fn observe(&self, session: &str, event: &mut Value) {
        if event["kind"] != "approval" {
            return;
        }
        let hash = args_sha256(&event["arguments"]);
        if let Some(object) = event.as_object_mut() {
            match &hash {
                Some(hash) => object.insert("args_sha256".into(), Value::String(hash.clone())),
                None => object.remove("args_sha256"),
            };
        }
        let (Some(id), Some(call), Some(hash)) = (
            event["id"].as_u64(),
            event["tool_call_id"].as_str().map(str::to_string),
            hash,
        ) else {
            return;
        };
        let call = call.as_str();
        let hash = hash.as_str();
        let mut table = self.inner.lock().expect("approvals");
        table.next += 1;
        let order = table.next;
        let map = &mut table.questions;
        map.insert(
            (session.to_string(), id),
            Pending {
                tool_call_id: call.into(),
                args_sha256: hash.into(),
                order,
            },
        );
        if map.len() > MAX_PENDING
            && let Some(oldest) = map
                .iter()
                .min_by_key(|(_, pending)| pending.order)
                .map(|(key, _)| key.clone())
        {
            map.remove(&oldest);
        }
    }

    /// Checks an answer `{id, allow, tool_call_id, args_sha256}` for
    /// `session` against the question it names, and forgets the question
    /// when it matches.
    pub fn take_matching(&self, session: &str, answer: &Value) -> Result<(), String> {
        let id = answer["id"]
            .as_u64()
            .ok_or("the answer names no question id")?;
        if !answer["allow"].is_boolean() {
            return Err("the answer must say allow true or false".into());
        }
        let call = answer["tool_call_id"]
            .as_str()
            .ok_or("a remote answer must name the tool call id")?;
        let hash = answer["args_sha256"]
            .as_str()
            .ok_or("a remote answer must name the sha256 of the arguments")?;
        let mut table = self.inner.lock().expect("approvals");
        let key = (session.to_string(), id);
        match table.questions.get(&key) {
            None => Err("no such question is waiting".into()),
            Some(pending) if pending.tool_call_id != call || pending.args_sha256 != hash => {
                Err("the answer names a different tool call or arguments than the question".into())
            }
            Some(_) => {
                table.questions.remove(&key);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn answers_must_name_the_same_call_and_arguments() {
        let pending = PendingApprovals::default();
        let hash = args_sha256(&json!({"path": "a.txt"})).unwrap();
        let mut event = json!({"kind":"approval","id":1,"question":"Allow write?","tool_call_id":"c1","arguments":{"path":"a.txt"},"args_sha256":"forged"});
        pending.observe("s", &mut event);
        assert_eq!(
            event["args_sha256"],
            hash.as_str(),
            "the connector's hash replaces any other"
        );
        let answer = |call: &str, hash: &str| json!({"id":1,"allow":true,"tool_call_id":call,"args_sha256":hash});
        assert!(
            pending
                .take_matching("s", &json!({"id":1,"allow":true}))
                .is_err()
        );
        assert!(pending.take_matching("s", &answer("c2", &hash)).is_err());
        assert!(
            pending
                .take_matching("s", &answer("c1", &"0".repeat(64)))
                .is_err()
        );
        assert!(
            pending
                .take_matching("other", &answer("c1", &hash))
                .is_err()
        );
        pending.take_matching("s", &answer("c1", &hash)).unwrap();
        assert!(pending.take_matching("s", &answer("c1", &hash)).is_err());
    }

    #[test]
    fn paths_are_recognised() {
        assert_eq!(approval_session("/api/sessions/abc/approval"), Some("abc"));
        assert_eq!(approval_session("/api/sessions/abc/prompt"), None);
        assert_eq!(
            approval_session("/api/sessions/abc/approval?x=1"),
            Some("abc")
        );
        assert!(looks_like_approval("/api/sessions/abc/%61pproval"));
        assert!(looks_like_approval("/api/sessions/abc/APPROVAL"));
        assert!(!looks_like_approval("/api/sessions/abc/prompt"));
        assert_eq!(events_session("/api/sessions/abc/events"), Some("abc"));
    }
}
