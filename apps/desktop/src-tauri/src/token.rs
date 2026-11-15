//! The token the interface calls the in-process server with.
//!
//! A fresh one is made at every launch from the operating system's random
//! source and never written anywhere: the server keeps it, and the web view
//! receives it through an initialization script (see `bridge`), never in an
//! address.

/// 256 random bits as 64 lowercase hex digits.
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system's random source works");
    hex(&bytes)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn a_token_is_64_hex_digits_and_new_every_time() {
        let tokens: HashSet<String> = (0..64).map(|_| new_token()).collect();
        assert_eq!(tokens.len(), 64, "no two tokens are alike");
        for token in &tokens {
            assert_eq!(token.len(), 64);
            assert!(
                token
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            );
        }
    }

    #[test]
    fn hex_encodes_every_byte_with_two_digits() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(hex(&[]), "");
    }

    #[test]
    fn the_server_accepts_the_token() {
        // hame-web takes at least 32 letters, digits, '-' or '_'.
        let token = new_token();
        assert!(token.len() >= 32);
        assert!(token.bytes().all(|b| b.is_ascii_alphanumeric()));
    }
}
