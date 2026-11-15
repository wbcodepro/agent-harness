//! Strings that come from the relay or another device: ids are restricted
//! to a safe alphabet, and anything printed loses control and bidirectional
//! formatting characters (which could rewrite or reorder what the terminal
//! shows, e.g. to fake a pairing prompt).

/// Longest device or stream id.
pub const MAX_ID: usize = 128;

/// Device and stream ids: 1 to 128 of `[A-Za-z0-9_-]`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Bidirectional formatting characters: embeddings, overrides, isolates and
/// marks.
pub fn is_bidi_format(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// `text` without control characters (C0, DEL, C1) or bidirectional
/// formatting characters, safe to print on a terminal.
pub fn printable(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() && !is_bidi_format(*c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_printing() {
        assert!(valid_id("dev7-62fb_x"));
        assert!(valid_id("0b5e1c1e-6c2d-4b7e-9d8a-3f1a2b3c4d5e"));
        for bad in ["", "a b", "a/b", "é", "a\nb", &"x".repeat(129)] {
            assert!(!valid_id(bad), "{bad:?}");
        }
        assert_eq!(
            printable("ok\u{1b}[2J\u{202E}gnp.exe\u{2066}\u{9b}!"),
            "ok[2Jgnp.exe!"
        );
    }
}
