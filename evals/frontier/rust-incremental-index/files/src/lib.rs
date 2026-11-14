//! `textpos`: a text index that converts between byte offsets, line/column
//! positions and UTF-16 offsets, and supports incremental edits.
//! SPEC.md is the specification.
//!
//! State of this file: the types are final. The methods that are written
//! out behave as the spec says but are linear, scanning the whole text on
//! every call, which rules 33 and 34 do not allow. The methods that panic
//! with `todo!` are not written yet.

use std::fmt;

/// The unit in which a column or a measure is counted: rule 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
    /// UTF-8 bytes.
    Byte,
    /// UTF-16 code units.
    Utf16,
    /// Unicode scalar values (Rust `char`s).
    Char,
    /// Display columns: a tab advances to the next multiple of 4, every
    /// other character counts 1. Only for columns (rule 2).
    Display,
}

/// A 0-based line and a 0-based column: rule 7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineCol {
    pub line: usize,
    pub col: usize,
}

/// A position given to `slice`, `insert`, `delete` and `replace`: rule 19.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pos {
    Byte(usize),
    Utf16(usize),
    LineCol(LineCol, Unit),
}

/// Every error the index reports: rules 26 and 27.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PosError {
    ByteOutOfRange { offset: usize, len: usize },
    NotCharBoundary { offset: usize },
    InsideLineBreak { offset: usize },
    Utf16OutOfRange { offset: usize, len: usize },
    InsideSurrogatePair { offset: usize },
    LineOutOfRange { line: usize, count: usize },
    ColumnOutOfRange { line: usize, col: usize, len: usize },
    InsideCharacter { line: usize, col: usize },
    ReversedRange { start: usize, end: usize },
}

impl fmt::Display for PosError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for PosError {}

/// Measure of `s`, which starts a line (or the text): rule 2.
fn measure(s: &str, unit: Unit) -> usize {
    s.chars().fold(0, |col, c| match unit {
        Unit::Byte => col + c.len_utf8(),
        Unit::Utf16 => col + c.len_utf16(),
        Unit::Char => col + 1,
        Unit::Display if c == '\t' => (col / 4 + 1) * 4,
        Unit::Display => col + 1,
    })
}

/// Which side of an edit at its offset an anchor keeps to: rule 31.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Bias {
    Before,
    After,
}

/// The name of an anchor: rule 29.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AnchorId(pub u64);

pub struct TextIndex {
    text: String,
}

impl TextIndex {
    /// Rule 1.
    pub fn new(text: &str) -> Self {
        TextIndex { text: text.to_string() }
    }

    /// Rule 1.
    pub fn text(&self) -> String {
        self.text.clone()
    }

    /// Rule 11.
    pub fn len_bytes(&self) -> usize {
        self.text.len()
    }

    /// Rule 11.
    pub fn len_utf16(&self) -> usize {
        measure(&self.text, Unit::Utf16)
    }

    /// Rule 11.
    pub fn len_chars(&self) -> usize {
        measure(&self.text, Unit::Char)
    }

    /// Byte offsets just after each line break, in order (rule 5).
    fn break_ends(&self) -> Vec<usize> {
        let b = self.text.as_bytes();
        let mut ends = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\n' {
                ends.push(i + 1);
            } else if b[i] == b'\r' {
                if i + 1 < b.len() && b[i + 1] == b'\n' {
                    i += 1;
                }
                ends.push(i + 1);
            }
            i += 1;
        }
        ends
    }

    /// Rule 11.
    pub fn line_count(&self) -> usize {
        self.break_ends().len() + 1
    }

    /// Rules 8 and 12.
    pub fn line_start(&self, line: usize) -> Result<usize, PosError> {
        let ends = self.break_ends();
        let count = ends.len() + 1;
        if line >= count {
            return Err(PosError::LineOutOfRange { line, count });
        }
        Ok(if line == 0 { 0 } else { ends[line - 1] })
    }

    /// Rules 8 and 12.
    pub fn line_end(&self, line: usize) -> Result<usize, PosError> {
        let ends = self.break_ends();
        let count = ends.len() + 1;
        if line >= count {
            return Err(PosError::LineOutOfRange { line, count });
        }
        if line + 1 == count {
            return Ok(self.text.len());
        }
        let next = ends[line];
        let b = self.text.as_bytes();
        let crlf = next >= 2 && b[next - 1] == b'\n' && b[next - 2] == b'\r';
        Ok(next - if crlf { 2 } else { 1 })
    }

    /// Rule 12.
    pub fn line_len(&self, line: usize, unit: Unit) -> Result<usize, PosError> {
        let _ = (line, unit);
        todo!("SPEC.md rule 12")
    }

    fn check_byte(&self, offset: usize) -> Result<(), PosError> {
        if offset > self.text.len() {
            return Err(PosError::ByteOutOfRange { offset, len: self.text.len() });
        }
        if !self.text.is_char_boundary(offset) {
            return Err(PosError::NotCharBoundary { offset });
        }
        Ok(())
    }

    /// Rules 10 and 13.
    pub fn offset_to_line_col(&self, offset: usize, unit: Unit) -> Result<LineCol, PosError> {
        self.check_byte(offset)?;
        let b = self.text.as_bytes();
        if offset > 0 && offset < b.len() && b[offset - 1] == b'\r' && b[offset] == b'\n' {
            return Err(PosError::InsideLineBreak { offset });
        }
        let ends = self.break_ends();
        let line = ends.iter().filter(|&&e| e <= offset).count();
        let start = if line == 0 { 0 } else { ends[line - 1] };
        Ok(LineCol { line, col: measure(&self.text[start..offset], unit) })
    }

    /// Rule 14.
    pub fn line_col_to_offset(&self, pos: LineCol, unit: Unit) -> Result<usize, PosError> {
        let _ = (pos, unit);
        todo!("SPEC.md rule 14")
    }

    /// Rule 15.
    pub fn line_col_to_offset_clamped(&self, pos: LineCol, unit: Unit) -> usize {
        let _ = (pos, unit);
        todo!("SPEC.md rule 15")
    }

    /// Rule 16.
    pub fn offset_to_utf16(&self, offset: usize) -> Result<usize, PosError> {
        self.check_byte(offset)?;
        Ok(measure(&self.text[..offset], Unit::Utf16))
    }

    /// Rule 17.
    pub fn utf16_to_offset(&self, offset: usize) -> Result<usize, PosError> {
        let _ = offset;
        todo!("SPEC.md rule 17")
    }

    /// Rules 18 to 20.
    pub fn slice(&self, start: Pos, end: Pos) -> Result<String, PosError> {
        let _ = (start, end);
        todo!("SPEC.md rule 18")
    }

    /// Rule 29.
    pub fn add_anchor(&mut self, at: Pos, bias: Bias) -> Result<AnchorId, PosError> {
        let _ = (at, bias);
        todo!("SPEC.md rule 29")
    }

    /// Rule 30.
    pub fn anchor_offset(&self, id: AnchorId) -> Option<usize> {
        let _ = id;
        todo!("SPEC.md rule 30")
    }

    /// Rule 30.
    pub fn remove_anchor(&mut self, id: AnchorId) -> bool {
        let _ = id;
        todo!("SPEC.md rule 30")
    }

    /// Rule 30.
    pub fn anchor_count(&self) -> usize {
        todo!("SPEC.md rule 30")
    }

    /// Rules 21 and 31.
    pub fn insert(&mut self, at: Pos, text: &str) -> Result<usize, PosError> {
        let _ = (at, text);
        todo!("SPEC.md rule 21")
    }

    /// Rules 22 and 31.
    pub fn delete(&mut self, start: Pos, end: Pos) -> Result<String, PosError> {
        let _ = (start, end);
        todo!("SPEC.md rule 22")
    }

    /// Rules 23 and 31.
    pub fn replace(&mut self, start: Pos, end: Pos, text: &str) -> Result<String, PosError> {
        let _ = (start, end, text);
        todo!("SPEC.md rule 23")
    }
}
