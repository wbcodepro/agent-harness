//! Approval binding (§4): a remote answer to an approval question must name
//! the tool call id and the sha256 of its arguments' canonical JSON
//! (RFC 8785), and both must match the question. hame web's approval event
//! carries `tool_call_id`, `tool`, `arguments` and `args_sha256`, and its
//! approval route checks an answer against them; devices recompute the
//! hash from the arguments they show (sync-js `approvalAnswer`). The
//! canonical form is hame web's `jcs` module, the one implementation in the
//! workspace.

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::keys::hex;

/// `sha256` (lowercase hex) of the arguments' canonical JSON, as hame web
/// computes it. None for arguments that have no canonical form (integers
/// beyond 2^53).
pub fn args_sha256(arguments: &Value) -> Option<String> {
    let canonical = hame_web::jcs::canonical(arguments).ok()?;
    Some(hex(&Sha256::digest(canonical.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_hash_ignores_member_order() {
        assert_eq!(
            args_sha256(&json!({"path": "a.txt", "content": "x"})),
            args_sha256(&json!({"content": "x", "path": "a.txt"}))
        );
        assert_eq!(args_sha256(&json!(9007199254740993u64)), None);
    }
}
