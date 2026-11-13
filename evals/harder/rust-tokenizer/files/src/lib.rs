#[derive(Debug, PartialEq)]
pub enum Token {
    Number(f64),
    Ident(String),
    Op(char),
    LParen,
    RParen,
}

/// Splits an arithmetic expression into tokens. Whitespace separates
/// tokens and is otherwise ignored. Errors name the offending character
/// and its byte offset, as `unexpected 'x' at 3`.
pub fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    todo!()
}
