use crate::error::ParseError;
use crate::lexer::Lexer;
use crate::record::{Field, Record};

/// Parses a whole log. Lines that are empty or hold only spaces, and lines
/// whose first character is `#`, are skipped (they still count for line
/// numbers). Stops at the first line that does not parse.
pub fn parse(input: &str) -> Result<Vec<Record>, ParseError> {
    let mut records = Vec::new();
    for (number, line) in input.split('\n').enumerate() {
        if line.starts_with('#') || line.chars().all(|c| c == ' ') {
            continue;
        }
        let mut lexer = Lexer::new(line);
        let mut fields = Vec::new();
        while let Some(field) = lexer.next_field() {
            match field {
                Ok((key, value)) => fields.push(Field { key, value }),
                Err((col, kind)) => return Err(ParseError { line: number + 1, col, kind }),
            }
        }
        records.push(Record::new(number + 1, line.to_string(), fields));
    }
    Ok(records)
}
