//! Device and host identities: an Ed25519 signing key and an X25519 key.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey as XPublic, StaticSecret};
use zeroize::Zeroizing;

use crate::SyncError;

/// Base64url without padding, the encoding of every key and byte string
/// in this protocol.
pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decodes base64url without padding.
pub fn unb64(text: &str) -> Result<Vec<u8>, SyncError> {
    URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| SyncError::Malformed("bad base64url"))
}

/// Decodes a base64url field that must be exactly 32 bytes long.
pub fn unb64_32(text: &str) -> Result<[u8; 32], SyncError> {
    unb64(text)?
        .try_into()
        .map_err(|_| SyncError::Malformed("a key must be 32 bytes"))
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A device's or host's two private keys. Never leaves the machine.
pub struct Identity {
    signing: SigningKey,
    exchange: StaticSecret,
}

/// The public half of an identity, as pinned by the other side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PublicIdentity {
    pub ed: [u8; 32],
    pub x: [u8; 32],
}

impl Identity {
    /// A new random identity.
    pub fn generate() -> Self {
        let mut ed = Zeroizing::new([0u8; 32]);
        let mut x = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(&mut *ed);
        OsRng.fill_bytes(&mut *x);
        Self::from_seeds(&ed, &x)
    }

    /// The identity with these private key seeds (Ed25519 seed, X25519
    /// scalar), for stored keys and test vectors.
    pub fn from_seeds(ed: &[u8; 32], x: &[u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(ed),
            exchange: StaticSecret::from(*x),
        }
    }

    pub fn ed_seed(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.signing.to_bytes())
    }

    pub fn x_seed(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.exchange.to_bytes())
    }

    pub fn public(&self) -> PublicIdentity {
        PublicIdentity {
            ed: self.signing.verifying_key().to_bytes(),
            x: XPublic::from(&self.exchange).to_bytes(),
        }
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing.sign(message).to_bytes()
    }

    /// X25519 with a peer's public key. Refuses a result of all zeros
    /// (a low-order peer key), which would make the derived key public.
    pub fn agree(&self, peer: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>, SyncError> {
        let shared = self.exchange.diffie_hellman(&XPublic::from(*peer));
        if !shared.was_contributory() {
            return Err(SyncError::Crypto("low-order key"));
        }
        Ok(Zeroizing::new(shared.to_bytes()))
    }
}

impl PublicIdentity {
    /// Lowercase hex of `SHA256(ed ‖ x)`, the same value the relay lists
    /// as a device's `fingerprint`.
    pub fn fingerprint(&self) -> String {
        hex(&Sha256::new()
            .chain_update(self.ed)
            .chain_update(self.x)
            .finalize())
    }

    /// The fingerprint in groups of four digits, for people to compare.
    pub fn display_fingerprint(&self) -> String {
        group_fingerprint(&self.fingerprint())
    }
}

/// Splits a hex fingerprint into groups of four digits.
pub fn group_fingerprint(fingerprint: &str) -> String {
    fingerprint
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Checks an Ed25519 signature (strict verification: no malleable or
/// small-order forms).
pub fn verify_signature(
    public: &[u8; 32],
    message: &[u8],
    signature: &[u8; 64],
) -> Result<(), SyncError> {
    let key = VerifyingKey::from_bytes(public).map_err(|_| SyncError::BadSignature)?;
    let signature = ed25519_dalek::Signature::from_bytes(signature);
    key.verify_strict(message, &signature)
        .map_err(|_| SyncError::BadSignature)
}

/// An identity as stored on disk (`identity.json`, mode 0600).
#[derive(Serialize, Deserialize)]
pub struct StoredIdentity {
    pub ed_seed: String,
    pub x_seed: String,
}

impl StoredIdentity {
    pub fn from_identity(identity: &Identity) -> Self {
        Self {
            ed_seed: b64(&*identity.ed_seed()),
            x_seed: b64(&*identity.x_seed()),
        }
    }

    pub fn identity(&self) -> Result<Identity, SyncError> {
        let ed = Zeroizing::new(unb64_32(&self.ed_seed)?);
        let x = Zeroizing::new(unb64_32(&self.x_seed)?);
        Ok(Identity::from_seeds(&ed, &x))
    }
}

/// 32 random bytes.
pub fn random32() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_verify_and_tampering_is_caught() {
        let id = Identity::generate();
        let signature = id.sign(b"hello");
        verify_signature(&id.public().ed, b"hello", &signature).unwrap();
        assert!(verify_signature(&id.public().ed, b"hellO", &signature).is_err());
        let other = Identity::generate();
        assert!(verify_signature(&other.public().ed, b"hello", &signature).is_err());
    }

    #[test]
    fn a_low_order_peer_key_is_refused() {
        let id = Identity::generate();
        assert!(id.agree(&[0u8; 32]).is_err());
    }

    #[test]
    fn fingerprints_match_the_relay_form() {
        let public = Identity::from_seeds(&[1; 32], &[2; 32]).public();
        let mut both = public.ed.to_vec();
        both.extend_from_slice(&public.x);
        assert_eq!(public.fingerprint(), hex(&Sha256::digest(&both)));
        assert_eq!(public.display_fingerprint().split(' ').count(), 16);
    }

    #[test]
    fn stored_identity_round_trips() {
        let id = Identity::generate();
        let back = StoredIdentity::from_identity(&id).identity().unwrap();
        assert_eq!(back.public(), id.public());
    }
}
