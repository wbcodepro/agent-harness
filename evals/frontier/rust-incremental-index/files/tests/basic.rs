//! A few of the examples in SPEC.md. Extend as you see fit.
use textpos::{AnchorId, Bias, LineCol, Pos, PosError, TextIndex, Unit};

#[test]
fn measures_and_lines() {
    let ix = TextIndex::new("aé€😀");
    assert_eq!((ix.len_bytes(), ix.len_utf16(), ix.len_chars()), (10, 5, 4));
    let ix = TextIndex::new("ab\r\ncd\ref\n\ng");
    assert_eq!(ix.line_count(), 5);
    let starts: Vec<_> = (0..5).map(|l| ix.line_start(l).unwrap()).collect();
    let ends: Vec<_> = (0..5).map(|l| ix.line_end(l).unwrap()).collect();
    assert_eq!(starts, vec![0, 4, 7, 10, 11]);
    assert_eq!(ends, vec![2, 6, 9, 10, 12]);
}

#[test]
fn error_table_start() {
    let ix = TextIndex::new("a😀\r\nb");
    assert_eq!(ix.offset_to_line_col(99, Unit::Byte), Err(PosError::ByteOutOfRange { offset: 99, len: 8 }));
    assert_eq!(ix.offset_to_line_col(6, Unit::Byte), Err(PosError::InsideLineBreak { offset: 6 }));
    assert_eq!(ix.offset_to_line_col(5, Unit::Utf16), Ok(LineCol { line: 0, col: 3 }));
    assert_eq!(ix.utf16_to_offset(2), Err(PosError::InsideSurrogatePair { offset: 2 }));
    assert_eq!(
        ix.line_col_to_offset(LineCol { line: 0, col: 4 }, Unit::Utf16),
        Err(PosError::ColumnOutOfRange { line: 0, col: 4, len: 3 })
    );
}

#[test]
fn edits_join_breaks() {
    let mut ix = TextIndex::new("a\rb\nc");
    assert_eq!(ix.line_count(), 3);
    assert_eq!(ix.delete(Pos::Byte(2), Pos::Byte(3)), Ok("b".to_string()));
    assert_eq!(ix.text(), "a\r\nc");
    assert_eq!(ix.line_count(), 2);
}

#[test]
fn anchors_follow_edits() {
    let mut ix = TextIndex::new("hello world");
    let a = ix.add_anchor(Pos::Byte(5), Bias::Before).unwrap();
    let b = ix.add_anchor(Pos::Byte(5), Bias::After).unwrap();
    let c = ix.add_anchor(Pos::Byte(11), Bias::Before).unwrap();
    assert_eq!((a, b, c), (AnchorId(0), AnchorId(1), AnchorId(2)));
    ix.insert(Pos::Byte(5), ",").unwrap();
    assert_eq!([a, b, c].map(|x| ix.anchor_offset(x)), [Some(5), Some(6), Some(12)]);
    ix.delete(Pos::Byte(3), Pos::Byte(7)).unwrap();
    assert_eq!([a, b, c].map(|x| ix.anchor_offset(x)), [Some(3), Some(3), Some(8)]);
}
