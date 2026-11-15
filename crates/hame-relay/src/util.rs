//! Small helpers: time, randomness, hashing and encodings.

use base64::Engine as _;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::{DecodePaddingMode, general_purpose};
use rand::RngCore as _;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `N` bytes from the thread-local CSPRNG (seeded from the operating system).
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    rand::rng().fill_bytes(&mut buf);
    buf
}

/// An opaque 256-bit token, base64url encoded, with a short prefix that
/// names its purpose (helps secret scanners and debugging; carries no data).
pub fn random_token(prefix: &str) -> String {
    format!("{prefix}{}", b64url(&random_bytes::<32>()))
}

/// Lower-case hex SHA-256.
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Raw SHA-256.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// The stored form of a secret token: its SHA-256.
pub fn token_hash(token: &str) -> String {
    sha256_hex(token.as_bytes())
}

const B64URL_LENIENT: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// base64url without padding.
pub fn b64url(data: &[u8]) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// base64url, padding optional.
pub fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    B64URL_LENIENT.decode(s).ok()
}

/// Constant-time comparison of two byte strings.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Trim and lower-case an email address and check its basic shape.
/// Returns `None` for anything that is not plausibly an address.
pub fn normalize_email(raw: &str) -> Option<String> {
    let email = raw.trim().to_lowercase();
    if email.is_empty() || email.len() > 254 {
        return None;
    }
    if email.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (local, domain) = email.rsplit_once('@')?;
    if local.is_empty() || local.len() > 64 || local.contains('@') {
        return None;
    }
    if domain.len() < 3
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
        || domain.starts_with('-')
    {
        return None;
    }
    Some(email)
}

/// Hex SHA-256 fingerprint of a device's public keys (Ed25519 then X25519),
/// shown to users to compare keys out of band.
pub fn key_fingerprint(ed25519: &[u8], x25519: &[u8]) -> String {
    let mut data = Vec::with_capacity(ed25519.len() + x25519.len());
    data.extend_from_slice(ed25519);
    data.extend_from_slice(x25519);
    sha256_hex(&data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_are_normalized() {
        assert_eq!(
            normalize_email("  Alice@Example.COM ").as_deref(),
            Some("alice@example.com")
        );
        assert!(normalize_email("no-at-sign").is_none());
        assert!(normalize_email("a@b").is_none());
        assert!(normalize_email("a b@example.com").is_none());
        assert!(normalize_email("@example.com").is_none());
    }

    #[test]
    fn tokens_are_256_bit_and_distinct() {
        let a = random_token("x_");
        let b = random_token("x_");
        assert_ne!(a, b);
        assert_eq!(b64url_decode(&a[2..]).unwrap().len(), 32);
        assert_eq!(token_hash(&a).len(), 64);
    }

    #[test]
    fn base64url_padding_is_optional() {
        assert_eq!(b64url_decode("YQ").unwrap(), b"a");
        assert_eq!(b64url_decode("YQ==").unwrap(), b"a");
        assert!(b64url_decode("Y+Q/").is_none());
    }
}
