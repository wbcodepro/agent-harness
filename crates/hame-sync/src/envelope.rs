//! The end-to-end envelope (§4 of docs/design/APPS-AND-SYNC.md).
//!
//! Wire layout (then base64url without padding in relay frames):
//!
//! ```text
//! 0x01                      version
//! u16 big endian            header length (at most 1024)
//! header                    UTF-8 JSON {"v":1,"from","to","stream","counter"}
//! 32 bytes                  ephemeral X25519 public key
//! 24 bytes                  XChaCha20 nonce
//! n + 16 bytes              XChaCha20-Poly1305 ciphertext and tag
//! 64 bytes                  Ed25519 signature
//! ```
//!
//! - key = HKDF-SHA256(ikm = X25519(ephemeral, recipient X25519),
//!   salt = ephemeral public ‖ recipient X25519 public,
//!   info = "hame-sync v1 envelope key"), 32 bytes;
//! - ciphertext = XChaCha20-Poly1305(key, nonce, plaintext,
//!   associated data = the header bytes exactly as sent);
//! - signature = Ed25519(sender, "hame-sync v1 signature" ‖ every byte
//!   before the signature), that is header ‖ ciphertext with their framing.
//!
//! The header is carried as the bytes that were signed and authenticated,
//! so neither side ever re-serialises it.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::SyncError;
use crate::keys::{Identity, PublicIdentity, b64, unb64, verify_signature};

pub const VERSION: u8 = 1;
pub const HKDF_INFO: &[u8] = b"hame-sync v1 envelope key";
pub const SIGNATURE_CONTEXT: &[u8] = b"hame-sync v1 signature";
/// Longest header, in bytes.
pub const MAX_HEADER: usize = 1024;
/// Longest device or stream id in a header.
pub const MAX_ID: usize = 128;
/// Largest plaintext an envelope carries (a 256 KiB blob chunk in base64
/// inside its JSON frame fits with room to spare).
pub const MAX_PLAINTEXT: usize = 512 * 1024;
/// Largest envelope as base64url text: the relay's 1 MiB limit.
pub const MAX_ENVELOPE_TEXT: usize = 1024 * 1024;

const OVERHEAD_AFTER_HEADER: usize = 32 + 24 + 16 + 64;

/// Who an envelope is from and for, which stream it belongs to, and the
/// sender's counter. Never holds anything a relay should not see.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub v: u32,
    pub from: String,
    pub to: String,
    pub stream: String,
    pub counter: u64,
}

impl Header {
    pub fn new(from: &str, to: &str, stream: &str, counter: u64) -> Self {
        Self {
            v: 1,
            from: from.into(),
            to: to.into(),
            stream: stream.into(),
            counter,
        }
    }

    fn check(&self) -> Result<(), SyncError> {
        if self.v != 1 {
            return Err(SyncError::Malformed("unknown envelope version"));
        }
        for id in [&self.from, &self.to, &self.stream] {
            if id.is_empty() || id.len() > MAX_ID {
                return Err(SyncError::Malformed("bad id in header"));
            }
        }
        // Counters stay exact in JavaScript numbers.
        if self.counter > (1u64 << 53) - 1 {
            return Err(SyncError::Malformed("counter out of range"));
        }
        Ok(())
    }
}

fn derive_key(
    shared: &[u8; 32],
    ephemeral: &[u8; 32],
    recipient: &[u8; 32],
) -> Zeroizing<[u8; 32]> {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(ephemeral);
    salt[32..].copy_from_slice(recipient);
    let mut key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(&salt), shared)
        .expand(HKDF_INFO, &mut *key)
        .expect("32 bytes is a valid HKDF length");
    key
}

/// Seals a plaintext from `sender` to the holder of `recipient_x`, and
/// returns the envelope as base64url text.
pub fn seal(
    sender: &Identity,
    recipient_x: &[u8; 32],
    header: &Header,
    plaintext: &[u8],
) -> Result<String, SyncError> {
    let mut ephemeral = Zeroizing::new([0u8; 32]);
    let mut nonce = [0u8; 24];
    OsRng.fill_bytes(&mut *ephemeral);
    OsRng.fill_bytes(&mut nonce);
    seal_with(sender, recipient_x, header, plaintext, &ephemeral, &nonce).map(|bytes| b64(&bytes))
}

/// [`seal`] with a given ephemeral key and nonce: deterministic, for test
/// vectors. Never call it with a reused ephemeral key.
pub fn seal_with(
    sender: &Identity,
    recipient_x: &[u8; 32],
    header: &Header,
    plaintext: &[u8],
    ephemeral_seed: &[u8; 32],
    nonce: &[u8; 24],
) -> Result<Vec<u8>, SyncError> {
    header.check()?;
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(SyncError::TooLarge);
    }
    let header_bytes = serde_json::to_vec(header).expect("a header serialises");
    if header_bytes.len() > MAX_HEADER {
        return Err(SyncError::Malformed("header too long"));
    }
    let ephemeral = Identity::from_seeds(&[0u8; 32], ephemeral_seed);
    let ephemeral_public = ephemeral.public().x;
    let shared = ephemeral.agree(recipient_x)?;
    let key = derive_key(&shared, &ephemeral_public, recipient_x);
    let ciphertext = XChaCha20Poly1305::new((&*key).into())
        .encrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad: &header_bytes,
            },
        )
        .map_err(|_| SyncError::Crypto("encryption failed"))?;

    let mut out =
        Vec::with_capacity(3 + header_bytes.len() + OVERHEAD_AFTER_HEADER + plaintext.len());
    out.push(VERSION);
    out.extend_from_slice(&(header_bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(&header_bytes);
    out.extend_from_slice(&ephemeral_public);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ciphertext);
    let signature = sender.sign(&signed_message(&out));
    out.extend_from_slice(&signature);
    Ok(out)
}

fn signed_message(before_signature: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(SIGNATURE_CONTEXT.len() + before_signature.len());
    message.extend_from_slice(SIGNATURE_CONTEXT);
    message.extend_from_slice(before_signature);
    message
}

/// A parsed envelope whose signature and contents are not checked yet.
#[derive(Debug)]
pub struct Sealed {
    pub header: Header,
    bytes: Vec<u8>,
    header_end: usize,
}

impl Sealed {
    /// Parses base64url envelope text, refusing oversize input before
    /// decoding it.
    pub fn parse(text: &str) -> Result<Self, SyncError> {
        if text.len() > MAX_ENVELOPE_TEXT {
            return Err(SyncError::TooLarge);
        }
        Self::from_bytes(unb64(text)?)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, SyncError> {
        if bytes.len() < 3 || bytes[0] != VERSION {
            return Err(SyncError::Malformed("not a version 1 envelope"));
        }
        let header_len = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
        if header_len > MAX_HEADER {
            return Err(SyncError::Malformed("header too long"));
        }
        let header_end = 3 + header_len;
        if bytes.len() < header_end + OVERHEAD_AFTER_HEADER {
            return Err(SyncError::Malformed("envelope too short"));
        }
        if bytes.len() - header_end - OVERHEAD_AFTER_HEADER > MAX_PLAINTEXT {
            return Err(SyncError::TooLarge);
        }
        let header: Header = serde_json::from_slice(&bytes[3..header_end])
            .map_err(|_| SyncError::Malformed("bad header"))?;
        header.check()?;
        Ok(Self {
            header,
            bytes,
            header_end,
        })
    }

    fn signature(&self) -> [u8; 64] {
        self.bytes[self.bytes.len() - 64..]
            .try_into()
            .expect("64 bytes")
    }

    /// Checks the sender's signature against its pinned signing key.
    pub fn verify(&self, sender_ed: &[u8; 32]) -> Result<(), SyncError> {
        let message = signed_message(&self.bytes[..self.bytes.len() - 64]);
        verify_signature(sender_ed, &message, &self.signature())
    }

    /// Decrypts with the recipient's identity. Does not check the
    /// signature: call [`Sealed::verify`] first, or use [`Sealed::open`].
    pub fn decrypt(&self, recipient: &Identity) -> Result<Zeroizing<Vec<u8>>, SyncError> {
        let body = &self.bytes[self.header_end..self.bytes.len() - 64];
        let ephemeral: [u8; 32] = body[..32].try_into().expect("32 bytes");
        let nonce = &body[32..56];
        let ciphertext = &body[56..];
        let shared = recipient.agree(&ephemeral)?;
        let key = derive_key(&shared, &ephemeral, &recipient.public().x);
        XChaCha20Poly1305::new((&*key).into())
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: &self.bytes[3..self.header_end],
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| SyncError::Crypto("decryption failed"))
    }

    /// Everything a recipient checks, in order: addressed to `my_id`,
    /// signed by the pinned `sender` key, a counter above `last_counter`,
    /// and contents that decrypt. The caller records the counter only
    /// after this succeeds.
    pub fn open(
        &self,
        me: &Identity,
        my_id: &str,
        sender: &PublicIdentity,
        last_counter: Option<u64>,
    ) -> Result<Zeroizing<Vec<u8>>, SyncError> {
        if self.header.to != my_id {
            return Err(SyncError::WrongRecipient);
        }
        self.verify(&sender.ed)?;
        if let Some(last) = last_counter
            && self.header.counter <= last
        {
            return Err(SyncError::Replay {
                counter: self.header.counter,
                last,
            });
        }
        self.decrypt(me)
    }
}

/// A sender's monotonic counter. Starts from the clock in microseconds, so
/// a sender that lost its state (a private browser window) still counts
/// upwards after a restart.
#[derive(Debug, Default)]
pub struct Counter {
    last: u64,
}

impl Counter {
    pub fn starting_after(last: u64) -> Self {
        Self { last }
    }

    pub fn last(&self) -> u64 {
        self.last
    }

    /// The next counter value: above the last one and at least the clock.
    pub fn advance(&mut self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0);
        self.last = now.max(self.last + 1);
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (Identity, Identity) {
        (Identity::generate(), Identity::generate())
    }

    #[test]
    fn a_sealed_envelope_opens_for_its_recipient() {
        let (alice, bob) = pair();
        let header = Header::new("alice", "bob", "7", 10);
        let text = seal(&alice, &bob.public().x, &header, b"secret").unwrap();
        let sealed = Sealed::parse(&text).unwrap();
        assert_eq!(sealed.header, header);
        let plain = sealed.open(&bob, "bob", &alice.public(), Some(9)).unwrap();
        assert_eq!(&plain[..], b"secret");
    }

    #[test]
    fn every_check_refuses() {
        let (alice, bob) = pair();
        let mallory = Identity::generate();
        let header = Header::new("alice", "bob", "7", 10);
        let text = seal(&alice, &bob.public().x, &header, b"secret").unwrap();
        let sealed = Sealed::parse(&text).unwrap();
        assert!(matches!(
            sealed.open(&bob, "carol", &alice.public(), None),
            Err(SyncError::WrongRecipient)
        ));
        assert!(matches!(
            sealed.open(&bob, "bob", &mallory.public(), None),
            Err(SyncError::BadSignature)
        ));
        assert!(matches!(
            sealed.open(&bob, "bob", &alice.public(), Some(10)),
            Err(SyncError::Replay { .. })
        ));
        // Not decryptable by anyone else, even with a valid signature.
        assert!(sealed.decrypt(&mallory).is_err());

        // Any flipped byte breaks the signature.
        let mut bytes = unb64(&text).unwrap();
        for at in [5, bytes.len() / 2, bytes.len() - 70, bytes.len() - 1] {
            bytes[at] ^= 1;
            if let Ok(tampered) = Sealed::from_bytes(bytes.clone()) {
                assert!(tampered.open(&bob, "bob", &alice.public(), None).is_err());
            }
            bytes[at] ^= 1;
        }
    }

    #[test]
    fn a_re_signed_tampered_ciphertext_fails_to_decrypt() {
        let (alice, bob) = pair();
        let header = Header::new("alice", "bob", "7", 10);
        let mut bytes = unb64(&seal(&alice, &bob.public().x, &header, b"secret").unwrap()).unwrap();
        let len = bytes.len();
        bytes[len - 65] ^= 1;
        let signature = alice.sign(&signed_message(&bytes[..len - 64]));
        bytes[len - 64..].copy_from_slice(&signature);
        let sealed = Sealed::from_bytes(bytes).unwrap();
        assert!(matches!(
            sealed.open(&bob, "bob", &alice.public(), None),
            Err(SyncError::Crypto(_))
        ));
    }

    #[test]
    fn oversize_is_refused_both_ways() {
        let (alice, bob) = pair();
        let header = Header::new("alice", "bob", "7", 1);
        assert!(matches!(
            seal(
                &alice,
                &bob.public().x,
                &header,
                &vec![0; MAX_PLAINTEXT + 1]
            ),
            Err(SyncError::TooLarge)
        ));
        assert!(seal(&alice, &bob.public().x, &header, &vec![0; MAX_PLAINTEXT]).is_ok());
        assert!(matches!(
            Sealed::parse(&"A".repeat(MAX_ENVELOPE_TEXT + 1)),
            Err(SyncError::TooLarge)
        ));
    }

    #[test]
    fn counters_increase() {
        let mut counter = Counter::starting_after(1 << 52);
        let a = counter.advance();
        let b = counter.advance();
        assert!(b > a);
        let mut fresh = Counter::default();
        assert!(fresh.advance() > 1_600_000_000_000_000);
    }
}
