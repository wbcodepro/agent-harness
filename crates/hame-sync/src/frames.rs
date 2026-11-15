//! Frames: the relay WebSocket frames (§3, visible to the relay) and the
//! remote API frames (§2, only ever inside an encrypted envelope).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Artifacts travel in chunks of this many bytes (before base64).
pub const BLOB_CHUNK: usize = 256 * 1024;

/// A text frame on the relay WebSocket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum RelayFrame {
    Send {
        to: String,
        envelope: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        push: bool,
    },
    Sent {
        to: String,
        seq: u64,
    },
    Deliver {
        seq: u64,
        from: String,
        envelope: String,
    },
    Ack {
        seq: u64,
    },
    Resume {
        after: u64,
    },
    Presence {
        device: String,
        online: bool,
    },
    /// `to` names the recipient when the error answers a `send`.
    Error {
        code: String,
        #[serde(default)]
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
    },
    /// Client → relay: a fresh access token for this connection
    /// (`token`); relay → client: the renewal was accepted (`expires_at`).
    Auth {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<i64>,
    },
    Ping,
    Pong,
}

/// A remote API frame, the plaintext of an envelope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Frame {
    /// An API call: `method` and `path` (under `/api/`), JSON `body`.
    Req {
        id: u64,
        method: String,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<Value>,
    },
    /// The answer to a `req`. `body` is the JSON the API returned, or its
    /// text as a string. For an artifact it is
    /// `{"blob":{"sha256","mime","size","chunks"}}` and `blob` frames with
    /// the same id follow.
    Res { id: u64, status: u16, body: Value },
    /// Subscribes to an event stream, skipping records whose sequence
    /// number is `after` or lower. Records are numbered from 0, so `-1` (the
    /// default) means from the start.
    Sub {
        id: u64,
        path: String,
        #[serde(default = "from_start")]
        after: i64,
    },
    /// One event of a subscription. `seq` is the record's sequence number;
    /// for events that are not records, the last record's (-1 before any).
    Evt { id: u64, seq: i64, data: Value },
    /// A subscription ended (the stream closed, was cancelled or failed).
    End {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Stops a subscription or abandons a request.
    Cancel { id: u64 },
    /// One chunk of an artifact.
    Blob {
        id: u64,
        sha256: String,
        chunk: u32,
        of: u32,
        data: String,
    },
    /// A pairing request from a device that scanned the host's QR code.
    Pair {
        ed: String,
        x: String,
        label: String,
        mac: String,
    },
    /// The host accepted the device: its signed grant.
    Paired { grant: String, sig: String },
    /// The host refused the pairing.
    PairRefused { reason: String },
    /// The host's answer to a valid pairing request: a fresh random nonce
    /// (base64url, 32 bytes) that goes into the SAS. It is chosen after the
    /// device committed to its keys, so the digits cannot be ground.
    PairChallenge { nonce: String },
    /// Removes a device from the host's allowlist (from any enrolled
    /// device). Answered with a `res`.
    Revoke { id: u64, device: String },
    /// Lists the host's enrolled devices. Answered with a `res`.
    Devices { id: u64 },
}

fn from_start() -> i64 {
    -1
}

impl Frame {
    /// The stream id this frame belongs to, which the envelope header
    /// carries too.
    pub fn stream(&self) -> String {
        match self {
            Frame::Req { id, .. }
            | Frame::Res { id, .. }
            | Frame::Sub { id, .. }
            | Frame::Evt { id, .. }
            | Frame::End { id, .. }
            | Frame::Cancel { id }
            | Frame::Blob { id, .. }
            | Frame::Revoke { id, .. }
            | Frame::Devices { id } => id.to_string(),
            Frame::Pair { .. }
            | Frame::Paired { .. }
            | Frame::PairRefused { .. }
            | Frame::PairChallenge { .. } => "pair".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frames_have_the_documented_shapes() {
        assert_eq!(
            serde_json::to_value(RelayFrame::Send {
                to: "d".into(),
                envelope: "e".into(),
                push: false
            })
            .unwrap(),
            json!({"op":"send","to":"d","envelope":"e"})
        );
        assert_eq!(
            serde_json::from_value::<RelayFrame>(json!({"op":"ping"})).unwrap(),
            RelayFrame::Ping
        );
        assert_eq!(
            serde_json::to_value(Frame::Sub {
                id: 8,
                path: "/api/sessions/abc/events".into(),
                after: 120
            })
            .unwrap(),
            json!({"t":"sub","id":8,"path":"/api/sessions/abc/events","after":120})
        );
        assert_eq!(
            serde_json::from_value::<Frame>(json!({"t":"end","id":8})).unwrap(),
            Frame::End { id: 8, error: None }
        );
        assert_eq!(
            serde_json::from_value::<Frame>(json!({"t":"pair_refused","reason":"no"}))
                .unwrap()
                .stream(),
            "pair"
        );
    }
}
