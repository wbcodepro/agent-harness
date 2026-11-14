/// What went wrong on a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// A field does not start with a key character.
    BadKey,
    /// The key is not followed by `=`.
    MissingEquals,
    /// A backslash in a quoted value is not followed by `\`, `"`, `n` or `t`.
    BadEscape,
    /// A quoted value has no closing quote.
    UnterminatedQuote,
    /// The closing quote is followed by something other than a space.
    TrailingAfterQuote,
}

/// A parse failure. `line` and `col` are 1-based; `col` counts characters,
/// not bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub col: usize,
    pub kind: ErrorKind,
}
