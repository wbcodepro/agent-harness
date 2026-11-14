use crate::error::ErrorKind;

/// Splits one line into `(key, value)` pairs.
pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'
}

impl Lexer {
    pub fn new(line: &str) -> Lexer {
        Lexer { chars: line.chars().collect(), pos: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// The next field, `None` at the end of the line. An error carries the
    /// 1-based column (in characters) and the kind.
    pub fn next_field(&mut self) -> Option<Result<(String, String), (usize, ErrorKind)>> {
        while self.peek() == Some(' ') {
            self.pos += 1;
        }
        self.peek()?;

        let mut key = String::new();
        while let Some(c) = self.peek() {
            if !is_key_char(c) {
                break;
            }
            key.push(c);
            self.pos += 1;
        }
        if key.is_empty() {
            return Some(Err((self.pos + 1, ErrorKind::BadKey)));
        }
        if self.peek() != Some('=') {
            return Some(Err((self.pos + 1, ErrorKind::MissingEquals)));
        }
        self.pos += 1;

        let mut value = String::new();
        if self.peek() == Some('"') {
            let open = self.pos;
            self.pos += 1;
            loop {
                match self.peek() {
                    None => return Some(Err((open + 1, ErrorKind::UnterminatedQuote))),
                    Some('"') => {
                        self.pos += 1;
                        break;
                    }
                    Some('\\') => {
                        let unescaped = match self.chars.get(self.pos + 1) {
                            Some('\\') => '\\',
                            Some('"') => '"',
                            Some('n') => '\n',
                            Some('t') => '\t',
                            _ => return Some(Err((self.pos + 1, ErrorKind::BadEscape))),
                        };
                        value.push(unescaped);
                        self.pos += 2;
                    }
                    Some(c) => {
                        value.push(c);
                        self.pos += 1;
                    }
                }
            }
            match self.peek() {
                None | Some(' ') => {}
                Some(_) => return Some(Err((self.pos + 1, ErrorKind::TrailingAfterQuote))),
            }
        } else {
            while let Some(c) = self.peek() {
                if c == ' ' {
                    break;
                }
                value.push(c);
                self.pos += 1;
            }
        }
        Some(Ok((key, value)))
    }
}
