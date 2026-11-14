//! Checks the crate against SPEC.md: the worked rules one by one, a
//! randomized comparison with a straightforward model of the spec, and the
//! workload of rule 34.

use std::time::{Duration, Instant};

use textpos::{AnchorId, Bias, LineCol, Pos, PosError, TextIndex, Unit};

const UNITS: [Unit; 4] = [Unit::Byte, Unit::Utf16, Unit::Char, Unit::Display];

fn lc(line: usize, col: usize) -> LineCol {
    LineCol { line, col }
}

// ---------------------------------------------------------------------------
// A direct, linear model of the spec.

mod model {
    use textpos::{AnchorId, Bias, LineCol, Pos, PosError, Unit};

    /// The column after `c` when it starts at column `col`.
    fn advance(col: usize, c: char, unit: Unit) -> usize {
        match unit {
            Unit::Byte => col + c.len_utf8(),
            Unit::Utf16 => col + c.len_utf16(),
            Unit::Char => col + 1,
            Unit::Display if c == '\t' => (col / 4 + 1) * 4,
            Unit::Display => col + 1,
        }
    }

    /// Measure of `s`, which starts a line (or the text).
    pub fn measure(s: &str, unit: Unit) -> usize {
        s.chars().fold(0, |col, c| advance(col, c, unit))
    }

    pub struct Model {
        pub text: String,
        pub anchors: Vec<Option<(usize, Bias)>>,
    }

    impl Model {
        pub fn len_utf16(&self) -> usize {
            measure(&self.text, Unit::Utf16)
        }

        pub fn break_ends(&self) -> Vec<usize> {
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

        pub fn line_count(&self) -> usize {
            self.break_ends().len() + 1
        }

        pub fn line_start(&self, line: usize) -> Result<usize, PosError> {
            let ends = self.break_ends();
            let count = ends.len() + 1;
            if line >= count {
                return Err(PosError::LineOutOfRange { line, count });
            }
            Ok(if line == 0 { 0 } else { ends[line - 1] })
        }

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

        pub fn line_len(&self, line: usize, unit: Unit) -> Result<usize, PosError> {
            let s = self.line_start(line)?;
            let e = self.line_end(line)?;
            Ok(measure(&self.text[s..e], unit))
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

        pub fn line_col_to_offset(&self, pos: LineCol, unit: Unit) -> Result<usize, PosError> {
            let start = self.line_start(pos.line)?;
            let end = self.line_end(pos.line)?;
            let len = measure(&self.text[start..end], unit);
            if pos.col > len {
                return Err(PosError::ColumnOutOfRange { line: pos.line, col: pos.col, len });
            }
            let mut acc = 0;
            for (i, c) in self.text[start..end].char_indices() {
                if acc == pos.col {
                    return Ok(start + i);
                }
                let next = advance(acc, c, unit);
                if next > pos.col {
                    return Err(PosError::InsideCharacter { line: pos.line, col: pos.col });
                }
                acc = next;
            }
            Ok(end)
        }

        pub fn line_col_to_offset_clamped(&self, pos: LineCol, unit: Unit) -> usize {
            if pos.line >= self.line_count() {
                return self.text.len();
            }
            let start = self.line_start(pos.line).unwrap();
            let end = self.line_end(pos.line).unwrap();
            let mut acc = 0;
            for (i, c) in self.text[start..end].char_indices() {
                let next = advance(acc, c, unit);
                if next > pos.col {
                    return start + i;
                }
                acc = next;
            }
            end
        }

        pub fn offset_to_utf16(&self, offset: usize) -> Result<usize, PosError> {
            self.check_byte(offset)?;
            Ok(measure(&self.text[..offset], Unit::Utf16))
        }

        pub fn utf16_to_offset(&self, offset: usize) -> Result<usize, PosError> {
            let len = self.len_utf16();
            if offset > len {
                return Err(PosError::Utf16OutOfRange { offset, len });
            }
            let mut acc = 0;
            for (i, c) in self.text.char_indices() {
                if acc == offset {
                    return Ok(i);
                }
                let w = c.len_utf16();
                if acc + w > offset {
                    return Err(PosError::InsideSurrogatePair { offset });
                }
                acc += w;
            }
            Ok(self.text.len())
        }

        fn resolve(&self, pos: Pos) -> Result<usize, PosError> {
            match pos {
                Pos::Byte(o) => self.check_byte(o).map(|_| o),
                Pos::Utf16(u) => self.utf16_to_offset(u),
                Pos::LineCol(lc, unit) => self.line_col_to_offset(lc, unit),
            }
        }

        fn range(&self, start: Pos, end: Pos) -> Result<(usize, usize), PosError> {
            let s = self.resolve(start)?;
            let e = self.resolve(end)?;
            if s > e {
                return Err(PosError::ReversedRange { start: s, end: e });
            }
            Ok((s, e))
        }

        pub fn slice(&self, start: Pos, end: Pos) -> Result<String, PosError> {
            let (s, e) = self.range(start, end)?;
            Ok(self.text[s..e].to_string())
        }

        pub fn add_anchor(&mut self, at: Pos, bias: Bias) -> Result<AnchorId, PosError> {
            let o = self.resolve(at)?;
            self.anchors.push(Some((o, bias)));
            Ok(AnchorId(self.anchors.len() as u64 - 1))
        }

        pub fn anchor_offset(&self, id: AnchorId) -> Option<usize> {
            let slot = self.anchors.get(usize::try_from(id.0).ok()?)?;
            slot.map(|(o, _)| o)
        }

        pub fn remove_anchor(&mut self, id: AnchorId) -> bool {
            match usize::try_from(id.0).ok().and_then(|i| self.anchors.get_mut(i)) {
                Some(slot) => slot.take().is_some(),
                None => false,
            }
        }

        pub fn anchor_count(&self) -> usize {
            self.anchors.iter().filter(|a| a.is_some()).count()
        }

        /// Rule 31: the bytes [s, e) were replaced by k bytes.
        fn move_anchors(&mut self, s: usize, e: usize, k: usize) {
            for (a, bias) in self.anchors.iter_mut().flatten() {
                if *a > e {
                    *a = *a - (e - s) + k;
                } else if *a >= s {
                    *a = if *bias == Bias::Before { s } else { s + k };
                }
            }
        }

        pub fn insert(&mut self, at: Pos, text: &str) -> Result<usize, PosError> {
            let o = self.resolve(at)?;
            self.text.insert_str(o, text);
            self.move_anchors(o, o, text.len());
            Ok(o)
        }

        pub fn delete(&mut self, start: Pos, end: Pos) -> Result<String, PosError> {
            let (s, e) = self.range(start, end)?;
            self.move_anchors(s, e, 0);
            Ok(self.text.drain(s..e).collect())
        }

        pub fn replace(&mut self, start: Pos, end: Pos, text: &str) -> Result<String, PosError> {
            let (s, e) = self.range(start, end)?;
            let removed: String = self.text.drain(s..e).collect();
            self.text.insert_str(s, text);
            self.move_anchors(s, e, text.len());
            Ok(removed)
        }
    }
}

use model::Model;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const PIECES: [&str; 18] = [
    "a", "b", "Z", " ", "\t", "\t", "é", "€", "😀", "𝄞", "\r", "\n", "\r\n", "\r", "\n", "\u{2028}", "\u{85}", "\tx",
];

fn random_text(rng: &mut Rng, pieces: usize) -> String {
    (0..pieces).map(|_| PIECES[rng.below(PIECES.len())]).collect()
}

fn random_unit(rng: &mut Rng) -> Unit {
    UNITS[rng.below(4)]
}

fn random_pos(rng: &mut Rng, m: &Model) -> Pos {
    match rng.below(3) {
        0 => Pos::Byte(rng.below(m.text.len() + 3)),
        1 => Pos::Utf16(rng.below(m.len_utf16() + 3)),
        _ => {
            let line = rng.below(m.line_count() + 1);
            let len = m.line_len(line, Unit::Byte).unwrap_or(2);
            Pos::LineCol(lc(line, rng.below(len + 3)), random_unit(rng))
        }
    }
}

/// Compares every query of the index with the model, exhaustively for a
/// short text and on a sample for a long one.
fn compare_queries(ix: &TextIndex, m: &Model, rng: &mut Rng, ctx: &str) {
    assert_eq!(ix.len_bytes(), m.text.len(), "{ctx}: len_bytes");
    assert_eq!(ix.len_utf16(), m.len_utf16(), "{ctx}: len_utf16");
    assert_eq!(ix.len_chars(), m.text.chars().count(), "{ctx}: len_chars");
    let count = m.line_count();
    assert_eq!(ix.line_count(), count, "{ctx}: line_count");
    let short = m.text.len() <= 120;
    let offsets: Vec<usize> = if short {
        (0..=m.text.len() + 1).collect()
    } else {
        (0..40).map(|_| rng.below(m.text.len() + 2)).collect()
    };
    for &o in &offsets {
        for unit in UNITS {
            assert_eq!(ix.offset_to_line_col(o, unit), m.offset_to_line_col(o, unit), "{ctx}: offset_to_line_col({o}, {unit:?})");
        }
        assert_eq!(ix.offset_to_utf16(o), m.offset_to_utf16(o), "{ctx}: offset_to_utf16({o})");
    }
    let u16s: Vec<usize> = if short {
        (0..=m.len_utf16() + 1).collect()
    } else {
        (0..40).map(|_| rng.below(m.len_utf16() + 2)).collect()
    };
    for &u in &u16s {
        assert_eq!(ix.utf16_to_offset(u), m.utf16_to_offset(u), "{ctx}: utf16_to_offset({u})");
    }
    let lines: Vec<usize> = if short {
        (0..=count).collect()
    } else {
        (0..15).map(|_| rng.below(count + 1)).collect()
    };
    for &l in &lines {
        assert_eq!(ix.line_start(l), m.line_start(l), "{ctx}: line_start({l})");
        assert_eq!(ix.line_end(l), m.line_end(l), "{ctx}: line_end({l})");
        let blen = m.line_len(l, Unit::Byte).unwrap_or(1);
        for unit in UNITS {
            assert_eq!(ix.line_len(l, unit), m.line_len(l, unit), "{ctx}: line_len({l}, {unit:?})");
            let cols: Vec<usize> = if short || blen < 12 {
                (0..=blen + 1).collect()
            } else {
                (0..6).map(|_| rng.below(blen + 2)).collect()
            };
            for c in cols {
                assert_eq!(ix.line_col_to_offset(lc(l, c), unit), m.line_col_to_offset(lc(l, c), unit), "{ctx}: line_col_to_offset({l}:{c}, {unit:?})");
                assert_eq!(ix.line_col_to_offset_clamped(lc(l, c), unit), m.line_col_to_offset_clamped(lc(l, c), unit), "{ctx}: line_col_to_offset_clamped({l}:{c}, {unit:?})");
            }
        }
    }
}

fn random_bias(rng: &mut Rng) -> Bias {
    if rng.below(2) == 0 { Bias::Before } else { Bias::After }
}

fn compare_anchors(ix: &TextIndex, m: &Model, ctx: &str) {
    assert_eq!(ix.anchor_count(), m.anchor_count(), "{ctx}: anchor_count");
    for id in 0..m.anchors.len() as u64 + 2 {
        assert_eq!(ix.anchor_offset(AnchorId(id)), m.anchor_offset(AnchorId(id)), "{ctx}: anchor_offset({id})");
    }
    assert_eq!(ix.anchor_offset(AnchorId(u64::MAX)), None, "{ctx}: anchor_offset(u64::MAX)");
}

#[test]
fn randomized_against_the_spec() {
    for seed in 1..=400u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let pieces = match seed % 10 {
            0 if seed % 20 == 0 => 1500 + rng.below(3000),
            0 => 600 + rng.below(600),
            1 | 2 => 200 + rng.below(400),
            _ => rng.below(30),
        };
        let start = random_text(&mut rng, pieces);
        let mut m = Model { text: start.clone(), anchors: Vec::new() };
        let mut ix = TextIndex::new(&start);
        compare_queries(&ix, &m, &mut rng, &format!("seed {seed}, new"));
        for _ in 0..rng.below(12) {
            let (at, bias) = (random_pos(&mut rng, &m), random_bias(&mut rng));
            assert_eq!(ix.add_anchor(at, bias), m.add_anchor(at, bias), "seed {seed}: add_anchor({at:?}, {bias:?})");
        }
        for step in 0..40 {
            let ctx = format!("seed {seed}, step {step}");
            match rng.below(6) {
                0 | 1 => {
                    let (at, bias) = (random_pos(&mut rng, &m), random_bias(&mut rng));
                    assert_eq!(ix.add_anchor(at, bias), m.add_anchor(at, bias), "{ctx}: add_anchor({at:?}, {bias:?})");
                }
                2 => {
                    let id = AnchorId(rng.below(m.anchors.len() + 2) as u64);
                    assert_eq!(ix.remove_anchor(id), m.remove_anchor(id), "{ctx}: remove_anchor({id:?})");
                }
                _ => {}
            }
            let n = rng.below(6);
            let insert = random_text(&mut rng, n);
            let (a, b) = (random_pos(&mut rng, &m), random_pos(&mut rng, &m));
            // A range that is mostly in order and short.
            let (a, b) = match (a, rng.below(4)) {
                (Pos::Byte(o), 0 | 1) => (a, Pos::Byte(o + rng.below(8))),
                _ => (a, b),
            };
            match rng.below(4) {
                0 => assert_eq!(ix.insert(a, &insert), m.insert(a, &insert), "{ctx}: insert({a:?}, {insert:?})"),
                1 => assert_eq!(ix.delete(a, b), m.delete(a, b), "{ctx}: delete({a:?}, {b:?})"),
                2 => assert_eq!(ix.replace(a, b, &insert), m.replace(a, b, &insert), "{ctx}: replace({a:?}, {b:?}, {insert:?})"),
                _ => assert_eq!(ix.slice(a, b), m.slice(a, b), "{ctx}: slice({a:?}, {b:?})"),
            }
            if m.text.len() <= 120 || step % 8 == 7 {
                assert_eq!(ix.text(), m.text, "{ctx}: text");
            }
            compare_queries(&ix, &m, &mut rng, &ctx);
            compare_anchors(&ix, &m, &ctx);
        }
        assert_eq!(ix.text(), m.text, "seed {seed}: final text");
    }
}

// ---------------------------------------------------------------------------
// Rules checked one by one.

#[test]
fn measures_rule_2() {
    let ix = TextIndex::new("aé€😀");
    assert_eq!((ix.len_bytes(), ix.len_utf16(), ix.len_chars()), (10, 5, 4));
    let ix = TextIndex::new("");
    assert_eq!((ix.len_bytes(), ix.len_utf16(), ix.len_chars(), ix.line_count()), (0, 0, 0, 1));
    assert_eq!(ix.text(), "");
}

#[test]
fn display_columns_rule_2() {
    let mut ix = TextIndex::new("a\tb\r\n\t\tx\nabcd\t€\t");
    let d = Unit::Display;
    assert_eq!(ix.line_len(0, d), Ok(5));
    assert_eq!(ix.line_len(1, d), Ok(9));
    assert_eq!(ix.line_len(2, d), Ok(12));
    assert_eq!(ix.line_len(2, Unit::Byte), Ok(9));
    assert_eq!(ix.line_len(2, Unit::Char), Ok(7));
    assert_eq!(ix.line_col_to_offset(lc(0, 1), d), Ok(1));
    assert_eq!(ix.line_col_to_offset(lc(0, 2), d), Err(PosError::InsideCharacter { line: 0, col: 2 }));
    assert_eq!(ix.line_col_to_offset(lc(0, 3), d), Err(PosError::InsideCharacter { line: 0, col: 3 }));
    assert_eq!(ix.line_col_to_offset(lc(0, 4), d), Ok(2));
    assert_eq!(ix.line_col_to_offset(lc(0, 5), d), Ok(3));
    assert_eq!(ix.line_col_to_offset(lc(0, 6), d), Err(PosError::ColumnOutOfRange { line: 0, col: 6, len: 5 }));
    assert_eq!(ix.line_col_to_offset_clamped(lc(0, 3), d), 1);
    assert_eq!(ix.line_col_to_offset_clamped(lc(0, 9), d), 3);
    assert_eq!(ix.offset_to_line_col(2, d), Ok(lc(0, 4)));
    assert_eq!(ix.offset_to_line_col(7, d), Ok(lc(1, 8)));
    assert_eq!(ix.offset_to_line_col(6, d), Ok(lc(1, 4)));
    assert_eq!(ix.line_col_to_offset(lc(2, 8), d), Ok(14));
    assert_eq!(ix.line_col_to_offset(lc(2, 9), d), Ok(17));
    assert_eq!(ix.line_col_to_offset(lc(2, 10), d), Err(PosError::InsideCharacter { line: 2, col: 10 }));
    assert_eq!(ix.line_col_to_offset(lc(2, 12), d), Ok(18));
    assert_eq!(ix.line_col_to_offset_clamped(lc(2, 11), d), 17);
    assert_eq!(ix.offset_to_line_col(18, d), Ok(lc(2, 12)));
    // Text before a tab moves the columns after it only across a tab stop.
    ix.insert(Pos::Byte(9), "xyz").unwrap();
    assert_eq!(ix.line_len(2, d), Ok(12));
    ix.insert(Pos::LineCol(lc(2, 0), d), "q").unwrap();
    assert_eq!(ix.line_len(2, d), Ok(16));
    assert_eq!(ix.line_col_to_offset(lc(2, 12), d), Ok(9 + 9));
    // A tab after a break starts again from column 0.
    let ix = TextIndex::new("abc\r\tz");
    assert_eq!(ix.offset_to_line_col(5, d), Ok(lc(1, 4)));
    let long = "\t".repeat(20_000) + "é" + &"ab\t".repeat(10_000);
    let ix = TextIndex::new(&format!("x\n{long}\n"));
    assert_eq!(ix.line_len(1, d), Ok(80_000 + 1 + 3 + 40_000 - 4));
    assert_eq!(ix.line_col_to_offset(lc(1, 80_000), d), Ok(2 + 20_000));
    assert_eq!(ix.line_col_to_offset(lc(1, 80_001), d), Ok(2 + 20_002));
    assert_eq!(ix.line_col_to_offset(lc(1, 80_007), d), Err(PosError::InsideCharacter { line: 1, col: 80_007 }));
    assert_eq!(ix.line_col_to_offset_clamped(lc(1, 80_007), d), 2 + 20_007);
    assert_eq!(ix.offset_to_line_col(2 + 20_002 + 3 * 5_000, d), Ok(lc(1, 80_001 + 3 + 4 * 4_999)));
}

#[test]
fn line_breaks_rules_5_and_6() {
    for (text, count) in [
        ("", 1),
        ("a", 1),
        ("a\n", 2),
        ("\r\n", 2),
        ("\r", 2),
        ("\r\r\n", 3),
        ("\n\r", 3),
        ("\n\r\n\r", 4),
        ("a\r\nb", 2),
        ("a\u{2028}b\u{85}c\u{0B}d\u{0C}e", 1),
    ] {
        assert_eq!(TextIndex::new(text).line_count(), count, "{text:?}");
    }
    let ix = TextIndex::new("ab\r\ncd\ref\n\ng");
    assert_eq!(ix.line_count(), 5);
    let starts: Vec<_> = (0..5).map(|l| ix.line_start(l).unwrap()).collect();
    let ends: Vec<_> = (0..5).map(|l| ix.line_end(l).unwrap()).collect();
    assert_eq!(starts, vec![0, 4, 7, 10, 11]);
    assert_eq!(ends, vec![2, 6, 9, 10, 12]);
    assert_eq!(ix.line_start(5), Err(PosError::LineOutOfRange { line: 5, count: 5 }));
    assert_eq!(ix.line_end(9), Err(PosError::LineOutOfRange { line: 9, count: 5 }));
    assert_eq!(ix.line_len(usize::MAX, Unit::Char), Err(PosError::LineOutOfRange { line: usize::MAX, count: 5 }));
}

#[test]
fn line_and_column_rules_9_10_13() {
    let ix = TextIndex::new("a😀\r\nb\rc");
    // bytes: a=0, 😀=1..5, \r=5, \n=6, b=7, \r=8, c=9, end=10
    assert_eq!(ix.offset_to_line_col(5, Unit::Byte), Ok(lc(0, 5)));
    assert_eq!(ix.offset_to_line_col(5, Unit::Utf16), Ok(lc(0, 3)));
    assert_eq!(ix.offset_to_line_col(5, Unit::Char), Ok(lc(0, 2)));
    assert_eq!(ix.offset_to_line_col(6, Unit::Byte), Err(PosError::InsideLineBreak { offset: 6 }));
    assert_eq!(ix.offset_to_line_col(7, Unit::Utf16), Ok(lc(1, 0)));
    assert_eq!(ix.offset_to_line_col(8, Unit::Char), Ok(lc(1, 1)));
    assert_eq!(ix.offset_to_line_col(9, Unit::Char), Ok(lc(2, 0)));
    assert_eq!(ix.offset_to_line_col(10, Unit::Byte), Ok(lc(2, 1)));
    assert_eq!(ix.offset_to_line_col(11, Unit::Byte), Err(PosError::ByteOutOfRange { offset: 11, len: 10 }));
    assert_eq!(ix.offset_to_line_col(2, Unit::Byte), Err(PosError::NotCharBoundary { offset: 2 }));
    assert_eq!(ix.offset_to_line_col(usize::MAX, Unit::Char), Err(PosError::ByteOutOfRange { offset: usize::MAX, len: 10 }));
    // A CR at the very end is a break; the end is on the empty last line.
    let ix = TextIndex::new("x\r");
    assert_eq!(ix.offset_to_line_col(2, Unit::Byte), Ok(lc(1, 0)));
    assert_eq!(ix.offset_to_line_col(1, Unit::Byte), Ok(lc(0, 1)));
}

#[test]
fn line_col_to_offset_rule_14() {
    let ix = TextIndex::new("a😀é\r\n€");
    // bytes: a=0, 😀=1..5, é=5..7, \r=7, \n=8, €=9..12
    assert_eq!(ix.line_col_to_offset(lc(0, 3), Unit::Utf16), Ok(5));
    assert_eq!(ix.line_col_to_offset(lc(0, 2), Unit::Utf16), Err(PosError::InsideCharacter { line: 0, col: 2 }));
    assert_eq!(ix.line_col_to_offset(lc(0, 2), Unit::Char), Ok(5));
    assert_eq!(ix.line_col_to_offset(lc(0, 3), Unit::Char), Ok(7));
    assert_eq!(ix.line_col_to_offset(lc(0, 4), Unit::Utf16), Ok(7));
    assert_eq!(ix.line_col_to_offset(lc(0, 5), Unit::Utf16), Err(PosError::ColumnOutOfRange { line: 0, col: 5, len: 4 }));
    assert_eq!(ix.line_col_to_offset(lc(0, 8), Unit::Byte), Err(PosError::ColumnOutOfRange { line: 0, col: 8, len: 7 }));
    assert_eq!(ix.line_col_to_offset(lc(0, 6), Unit::Byte), Err(PosError::InsideCharacter { line: 0, col: 6 }));
    assert_eq!(ix.line_col_to_offset(lc(1, 1), Unit::Byte), Err(PosError::InsideCharacter { line: 1, col: 1 }));
    assert_eq!(ix.line_col_to_offset(lc(1, 3), Unit::Byte), Ok(12));
    assert_eq!(ix.line_col_to_offset(lc(1, 1), Unit::Char), Ok(12));
    assert_eq!(ix.line_col_to_offset(lc(2, 0), Unit::Char), Err(PosError::LineOutOfRange { line: 2, count: 2 }));
    assert_eq!(ix.line_col_to_offset(lc(2, 99), Unit::Char), Err(PosError::LineOutOfRange { line: 2, count: 2 }));
    assert_eq!(ix.line_col_to_offset(lc(0, usize::MAX), Unit::Utf16), Err(PosError::ColumnOutOfRange { line: 0, col: usize::MAX, len: 4 }));
    assert_eq!(ix.line_len(0, Unit::Byte), Ok(7));
    assert_eq!(ix.line_len(0, Unit::Utf16), Ok(4));
    assert_eq!(ix.line_len(0, Unit::Char), Ok(3));
}

#[test]
fn clamped_rule_15() {
    let ix = TextIndex::new("a😀é\r\n€");
    assert_eq!(ix.line_col_to_offset_clamped(lc(0, 2), Unit::Utf16), 1);
    assert_eq!(ix.line_col_to_offset_clamped(lc(0, 6), Unit::Byte), 5);
    assert_eq!(ix.line_col_to_offset_clamped(lc(0, 3), Unit::Byte), 1);
    assert_eq!(ix.line_col_to_offset_clamped(lc(0, 99), Unit::Char), 7);
    assert_eq!(ix.line_col_to_offset_clamped(lc(0, usize::MAX), Unit::Utf16), 7);
    assert_eq!(ix.line_col_to_offset_clamped(lc(1, 2), Unit::Byte), 9);
    assert_eq!(ix.line_col_to_offset_clamped(lc(1, 0), Unit::Char), 9);
    assert_eq!(ix.line_col_to_offset_clamped(lc(2, 0), Unit::Char), 12);
    assert_eq!(ix.line_col_to_offset_clamped(lc(usize::MAX, usize::MAX), Unit::Byte), 12);
}

#[test]
fn utf16_rules_16_17() {
    let ix = TextIndex::new("a😀\r\nb");
    // utf16: a=0, 😀=1..3, \r=3, \n=4, b=5, end=6
    assert_eq!(ix.offset_to_utf16(5), Ok(3));
    assert_eq!(ix.offset_to_utf16(6), Ok(4));
    assert_eq!(ix.offset_to_utf16(8), Ok(6));
    assert_eq!(ix.offset_to_utf16(3), Err(PosError::NotCharBoundary { offset: 3 }));
    assert_eq!(ix.offset_to_utf16(9), Err(PosError::ByteOutOfRange { offset: 9, len: 8 }));
    assert_eq!(ix.utf16_to_offset(1), Ok(1));
    assert_eq!(ix.utf16_to_offset(2), Err(PosError::InsideSurrogatePair { offset: 2 }));
    assert_eq!(ix.utf16_to_offset(3), Ok(5));
    assert_eq!(ix.utf16_to_offset(4), Ok(6));
    assert_eq!(ix.utf16_to_offset(6), Ok(8));
    assert_eq!(ix.utf16_to_offset(7), Err(PosError::Utf16OutOfRange { offset: 7, len: 6 }));
    assert_eq!(ix.utf16_to_offset(usize::MAX), Err(PosError::Utf16OutOfRange { offset: usize::MAX, len: 6 }));
}

#[test]
fn ranges_and_slices_rules_18_to_20() {
    let ix = TextIndex::new("a😀\r\nb");
    assert_eq!(ix.slice(Pos::Byte(1), Pos::Utf16(3)), Ok("😀".to_string()));
    assert_eq!(ix.slice(Pos::Utf16(4), Pos::LineCol(lc(1, 1), Unit::Char)), Ok("\nb".to_string()));
    assert_eq!(ix.slice(Pos::Byte(6), Pos::Byte(6)), Ok(String::new()));
    assert_eq!(ix.slice(Pos::Byte(2), Pos::Byte(99)), Err(PosError::NotCharBoundary { offset: 2 }));
    assert_eq!(ix.slice(Pos::Byte(99), Pos::Byte(2)), Err(PosError::ByteOutOfRange { offset: 99, len: 8 }));
    assert_eq!(ix.slice(Pos::Byte(5), Pos::Byte(1)), Err(PosError::ReversedRange { start: 5, end: 1 }));
    assert_eq!(ix.slice(Pos::Byte(0), Pos::Utf16(2)), Err(PosError::InsideSurrogatePair { offset: 2 }));
    assert_eq!(
        ix.slice(Pos::LineCol(lc(0, 2), Unit::Utf16), Pos::Byte(0)),
        Err(PosError::InsideCharacter { line: 0, col: 2 })
    );
    assert_eq!(
        ix.slice(Pos::LineCol(lc(1, 0), Unit::Char), Pos::LineCol(lc(0, 9), Unit::Char)),
        Err(PosError::ColumnOutOfRange { line: 0, col: 9, len: 2 })
    );
    assert_eq!(
        ix.slice(Pos::LineCol(lc(1, 0), Unit::Char), Pos::LineCol(lc(0, 2), Unit::Char)),
        Err(PosError::ReversedRange { start: 7, end: 5 })
    );
}

#[test]
fn edits_rules_21_to_24() {
    let mut ix = TextIndex::new("hello\nworld");
    assert_eq!(ix.insert(Pos::LineCol(lc(1, 0), Unit::Byte), "big "), Ok(6));
    assert_eq!(ix.text(), "hello\nbig world");
    assert_eq!(ix.delete(Pos::Byte(0), Pos::Utf16(6)), Ok("hello\n".to_string()));
    assert_eq!(ix.text(), "big world");
    assert_eq!(ix.line_count(), 1);
    assert_eq!(ix.replace(Pos::Byte(0), Pos::Byte(3), "😀\r\n"), Ok("big".to_string()));
    assert_eq!(ix.text(), "😀\r\n world");
    assert_eq!(ix.line_count(), 2);
    assert_eq!(ix.insert(Pos::Byte(ix.len_bytes()), ""), Ok(12));
    assert_eq!(ix.text(), "😀\r\n world");
    // Errors change nothing.
    assert_eq!(ix.insert(Pos::Byte(2), "x"), Err(PosError::NotCharBoundary { offset: 2 }));
    assert_eq!(ix.insert(Pos::Utf16(1), "x"), Err(PosError::InsideSurrogatePair { offset: 1 }));
    assert_eq!(ix.insert(Pos::LineCol(lc(0, 3), Unit::Char), "x"), Err(PosError::ColumnOutOfRange { line: 0, col: 3, len: 1 }));
    assert_eq!(ix.delete(Pos::Byte(4), Pos::Byte(0)), Err(PosError::ReversedRange { start: 4, end: 0 }));
    assert_eq!(ix.replace(Pos::Byte(0), Pos::Byte(14), "x"), Err(PosError::ByteOutOfRange { offset: 14, len: 12 }));
    assert_eq!(ix.delete(Pos::LineCol(lc(5, 0), Unit::Byte), Pos::Byte(0)), Err(PosError::LineOutOfRange { line: 5, count: 2 }));
    assert_eq!(ix.text(), "😀\r\n world");
    assert_eq!(ix.line_count(), 2);
    // Positions inside a line break are fine for edits.
    assert_eq!(ix.insert(Pos::Byte(5), "x"), Ok(5));
    assert_eq!(ix.text(), "😀\rx\n world");
    assert_eq!(ix.line_count(), 3);
}

#[test]
fn edits_join_and_split_breaks_rule_25() {
    let mut ix = TextIndex::new("a\r\nb");
    ix.insert(Pos::Utf16(2), "x").unwrap();
    assert_eq!((ix.text().as_str(), ix.line_count()), ("a\rx\nb", 3));
    assert_eq!(ix.offset_to_line_col(3, Unit::Byte), Ok(lc(1, 1)));
    ix.delete(Pos::Byte(2), Pos::Byte(3)).unwrap();
    assert_eq!((ix.text().as_str(), ix.line_count()), ("a\r\nb", 2));
    assert_eq!(ix.offset_to_line_col(2, Unit::Byte), Err(PosError::InsideLineBreak { offset: 2 }));
    assert_eq!(ix.line_end(0), Ok(1));
    ix.delete(Pos::Byte(2), Pos::Byte(3)).unwrap();
    assert_eq!((ix.text().as_str(), ix.line_count()), ("a\rb", 2));
    assert_eq!(ix.line_start(1), Ok(2));
    let mut ix = TextIndex::new("x\r");
    ix.insert(Pos::Byte(2), "\n").unwrap();
    assert_eq!(ix.line_count(), 2);
    assert_eq!(ix.line_end(0), Ok(1));
    assert_eq!(ix.offset_to_line_col(2, Unit::Byte), Err(PosError::InsideLineBreak { offset: 2 }));
    ix.insert(Pos::Byte(1), "\r").unwrap();
    assert_eq!((ix.text().as_str(), ix.line_count()), ("x\r\r\n", 3));
    ix.delete(Pos::Byte(0), Pos::Byte(4)).unwrap();
    assert_eq!((ix.text().as_str(), ix.line_count(), ix.len_utf16()), ("", 1, 0));
    // Joining across a long text.
    let long = format!("{}\r{}", "a".repeat(5000), "b".repeat(5000));
    let mut ix = TextIndex::new(&format!("{long}\n"));
    assert_eq!(ix.line_count(), 3);
    ix.delete(Pos::Byte(5001), Pos::Byte(10001)).unwrap();
    assert_eq!(ix.line_count(), 2);
    assert_eq!(ix.line_end(0), Ok(5000));
    assert_eq!(ix.line_start(1), Ok(5002));
}

#[test]
fn long_lines_and_many_lines() {
    let line = "é😀a".repeat(3000); // 21,000 bytes, 12,000 UTF-16 units, 9,000 chars
    let text = format!("{line}\r\n{line}\r{line}\n");
    let ix = TextIndex::new(&text);
    assert_eq!(ix.line_count(), 4);
    assert_eq!(ix.line_len(1, Unit::Utf16), Ok(12_000));
    assert_eq!(ix.line_start(2), Ok(21_000 * 2 + 3));
    assert_eq!(ix.line_col_to_offset(lc(1, 8_999), Unit::Char), Ok(21_002 + 20_999));
    assert_eq!(ix.line_col_to_offset(lc(1, 11_999), Unit::Utf16), Ok(21_002 + 20_999));
    assert_eq!(ix.line_col_to_offset(lc(1, 11_998), Unit::Utf16), Err(PosError::InsideCharacter { line: 1, col: 11_998 }));
    assert_eq!(ix.offset_to_line_col(21_002 + 7 * 1000 + 2, Unit::Utf16), Ok(lc(1, 4 * 1000 + 1)));
    assert_eq!(ix.offset_to_line_col(21_002 + 7 * 1000 + 3, Unit::Utf16), Err(PosError::NotCharBoundary { offset: 21_002 + 7 * 1000 + 3 }));
    assert_eq!(ix.utf16_to_offset(12_002 + 4), Ok(21_002 + 7));
    // "€\r" followed by "\n" is one CRLF break: two breaks per three pieces.
    let many: String = (0..30_000).map(|i| if i % 3 == 0 { "x\r\n" } else if i % 3 == 1 { "€\r" } else { "\n" }).collect();
    let ix = TextIndex::new(&many);
    assert_eq!(ix.line_count(), 20_001);
    let m = Model { text: many, anchors: Vec::new() };
    for l in (0..20_001).step_by(997) {
        assert_eq!(ix.line_start(l), m.line_start(l), "line_start({l})");
        assert_eq!(ix.line_end(l), m.line_end(l), "line_end({l})");
    }
}

#[test]
fn anchors_rules_29_to_32() {
    let mut ix = TextIndex::new("hello world");
    assert_eq!(ix.anchor_count(), 0);
    assert_eq!(ix.anchor_offset(AnchorId(0)), None);
    let a = ix.add_anchor(Pos::Byte(5), Bias::Before).unwrap();
    let b = ix.add_anchor(Pos::Utf16(5), Bias::After).unwrap();
    assert_eq!((a, b), (AnchorId(0), AnchorId(1)));
    assert_eq!(ix.add_anchor(Pos::Byte(99), Bias::After), Err(PosError::ByteOutOfRange { offset: 99, len: 11 }));
    assert_eq!(ix.add_anchor(Pos::LineCol(lc(1, 0), Unit::Char), Bias::After), Err(PosError::LineOutOfRange { line: 1, count: 1 }));
    let c = ix.add_anchor(Pos::LineCol(lc(0, 11), Unit::Char), Bias::Before).unwrap();
    assert_eq!(c, AnchorId(2));
    assert_eq!(ix.anchor_count(), 3);
    // Insertion at the anchors' offset.
    ix.insert(Pos::Byte(5), ",").unwrap();
    assert_eq!(ix.text(), "hello, world");
    assert_eq!([a, b, c].map(|x| ix.anchor_offset(x)), [Some(5), Some(6), Some(12)]);
    // A deleted range that contains anchors.
    ix.delete(Pos::Byte(3), Pos::Byte(7)).unwrap();
    assert_eq!(ix.text(), "helworld");
    assert_eq!([a, b, c].map(|x| ix.anchor_offset(x)), [Some(3), Some(3), Some(8)]);
    // Anchors at the start of a replaced range.
    ix.replace(Pos::Byte(3), Pos::Byte(4), "XY").unwrap();
    assert_eq!(ix.text(), "helXYorld");
    assert_eq!([a, b, c].map(|x| ix.anchor_offset(x)), [Some(3), Some(5), Some(9)]);
    // Anchors at the end of a replaced range.
    let d = ix.add_anchor(Pos::Byte(5), Bias::Before).unwrap();
    assert_eq!(d, AnchorId(3));
    ix.replace(Pos::Byte(3), Pos::Byte(5), "Q").unwrap();
    assert_eq!(ix.text(), "helQorld");
    assert_eq!([a, b, c, d].map(|x| ix.anchor_offset(x)), [Some(3), Some(4), Some(8), Some(3)]);
    // Failed edits and empty edits move nothing.
    assert!(ix.delete(Pos::Byte(5), Pos::Byte(1)).is_err());
    assert!(ix.insert(Pos::Byte(50), "zzz").is_err());
    ix.insert(Pos::Byte(4), "").unwrap();
    ix.delete(Pos::Byte(8), Pos::Byte(8)).unwrap();
    assert_eq!([a, b, c, d].map(|x| ix.anchor_offset(x)), [Some(3), Some(4), Some(8), Some(3)]);
    // An insert at the end of the text.
    ix.insert(Pos::Byte(8), "!").unwrap();
    assert_eq!([b, c].map(|x| ix.anchor_offset(x)), [Some(4), Some(8)]);
    // Removal; ids are never reused.
    assert!(ix.remove_anchor(a));
    assert!(!ix.remove_anchor(a));
    assert!(!ix.remove_anchor(AnchorId(77)));
    assert!(!ix.remove_anchor(AnchorId(u64::MAX)));
    assert_eq!(ix.anchor_offset(a), None);
    assert_eq!(ix.anchor_count(), 3);
    assert_eq!(ix.add_anchor(Pos::Byte(0), Bias::After), Ok(AnchorId(4)));
    ix.insert(Pos::Byte(0), "ab").unwrap();
    assert_eq!([b, c, d, AnchorId(4)].map(|x| ix.anchor_offset(x)), [Some(6), Some(10), Some(5), Some(2)]);
    // Deleting everything.
    ix.delete(Pos::Byte(0), Pos::Byte(ix.len_bytes())).unwrap();
    assert_eq!([b, c, d, AnchorId(4)].map(|x| ix.anchor_offset(x)), [Some(0); 4]);
    // An anchor may end up inside a line break.
    let mut ix = TextIndex::new("\rx\n");
    let e = ix.add_anchor(Pos::Byte(1), Bias::After).unwrap();
    assert_eq!(e, AnchorId(0));
    ix.delete(Pos::Byte(1), Pos::Byte(2)).unwrap();
    assert_eq!(ix.anchor_offset(e), Some(1));
    assert_eq!(ix.offset_to_line_col(1, Unit::Byte), Err(PosError::InsideLineBreak { offset: 1 }));
}

// ---------------------------------------------------------------------------
// Rule 34: the workload.

const TIME_LIMIT: Duration = Duration::from_secs(20);
const ANCHORS: usize = 1_000_000;

struct Hash(u64);

impl Hash {
    fn word(&mut self, w: u64) {
        for b in w.to_le_bytes() {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
    }
    fn err(&mut self, e: &PosError) {
        let words: [u64; 4] = match *e {
            PosError::ByteOutOfRange { offset, len } => [1, offset as u64, len as u64, 0],
            PosError::NotCharBoundary { offset } => [2, offset as u64, 0, 0],
            PosError::InsideLineBreak { offset } => [3, offset as u64, 0, 0],
            PosError::Utf16OutOfRange { offset, len } => [4, offset as u64, len as u64, 0],
            PosError::InsideSurrogatePair { offset } => [5, offset as u64, 0, 0],
            PosError::LineOutOfRange { line, count } => [6, line as u64, count as u64, 0],
            PosError::ColumnOutOfRange { line, col, len } => [7, line as u64, col as u64, len as u64],
            PosError::InsideCharacter { line, col } => [8, line as u64, col as u64, 0],
            PosError::ReversedRange { start, end } => [9, start as u64, end as u64, 0],
        };
        for w in words {
            self.word(w);
        }
    }
    fn num(&mut self, r: Result<usize, PosError>) {
        match r {
            Ok(v) => {
                self.word(0);
                self.word(v as u64);
            }
            Err(e) => self.err(&e),
        }
    }
    fn text(&mut self, r: Result<String, PosError>) {
        match r {
            Ok(s) => {
                self.word(0);
                for b in s.bytes() {
                    self.word(b as u64);
                }
            }
            Err(e) => self.err(&e),
        }
    }
}

fn workload_text(rng: &mut Rng) -> String {
    const WORDS: [&str; 8] = ["let", "x", "=", "value", "é", "€", "😀", "\t"];
    let mut text = String::with_capacity(8 << 20);
    let mut long_lines = 0;
    while text.len() < 8 << 20 {
        // After every 2 MiB, one line of about 1.2 MiB.
        let n = if text.len() >= (long_lines + 1) << 21 {
            long_lines += 1;
            280_000
        } else {
            rng.below(12)
        };
        for _ in 0..n {
            let w = if rng.below(10) == 0 { WORDS[4 + rng.below(4)] } else { WORDS[rng.below(4)] };
            text.push_str(w);
            text.push(' ');
        }
        text.push_str(match rng.below(20) {
            0..=15 => "\n",
            16..=18 => "\r\n",
            _ => "\r",
        });
    }
    text
}

const EDIT_PIECES: [&str; 8] = ["a", "bc", "é", "😀", "\t", " ", "€", "xyz"];
const BREAKS: [&str; 3] = ["\r", "\n", "\r\n"];

/// A short text; line breaks are rare, so that the long lines stay long.
fn small_text(rng: &mut Rng) -> String {
    let mut s = String::new();
    for _ in 0..rng.below(5) {
        if rng.below(4000) == 0 {
            s.push_str(BREAKS[rng.below(3)]);
        } else {
            s.push_str(EDIT_PIECES[rng.below(EDIT_PIECES.len())]);
        }
    }
    s
}

#[test]
fn workload_rule_34() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let text = workload_text(&mut rng);
    let started = Instant::now();
    let mut ix = TextIndex::new(&text);
    drop(text);
    let mut h = Hash(0xcbf2_9ce4_8422_2325);
    // Anchors all over the text.
    let mut ids = 0usize;
    for j in 0..ANCHORS {
        let bias = if j % 2 == 0 { Bias::Before } else { Bias::After };
        match ix.add_anchor(Pos::Byte(rng.below(ix.len_bytes() + 1)), bias) {
            Ok(id) => {
                assert_eq!(id, AnchorId(ids as u64), "rule 29: anchor ids");
                ids += 1;
            }
            Err(e) => h.err(&e),
        }
    }
    for i in 0..100_000usize {
        assert!(started.elapsed() < TIME_LIMIT, "rule 34: only {i} of 100,000 rounds done after {TIME_LIMIT:?}");
        let len = ix.len_bytes();
        let len16 = ix.len_utf16();
        let lines = ix.line_count();
        let unit = random_unit(&mut rng);
        match i % 5 {
            0 => {
                let s = small_text(&mut rng);
                h.num(ix.insert(Pos::Byte(rng.below(len + 1)), &s));
            }
            1 => {
                let p = rng.below(len + 1);
                h.text(ix.delete(Pos::Byte(p), Pos::Byte(p + rng.below(17))));
            }
            2 => {
                let s = small_text(&mut rng);
                h.num(ix.insert(Pos::LineCol(lc(rng.below(lines), rng.below(50)), unit), &s));
            }
            3 => {
                let u = rng.below(len16 + 1);
                h.text(ix.delete(Pos::Utf16(u), Pos::Utf16(u + rng.below(9))));
            }
            _ => {
                let s = small_text(&mut rng);
                let (line, col) = match ix.offset_to_line_col(rng.below(len + 1), unit) {
                    Ok(p) => (p.line, p.col),
                    Err(_) => (rng.below(lines), rng.below(8)),
                };
                let a = Pos::LineCol(lc(line, col), unit);
                let b = Pos::LineCol(lc(line, col + 8 + rng.below(8)), unit);
                h.text(ix.replace(a, b, &s));
            }
        }
        let len = ix.len_bytes();
        let len16 = ix.len_utf16();
        let lines = ix.line_count();
        h.word(len as u64);
        h.word(len16 as u64);
        h.word(lines as u64);
        h.word(ix.len_chars() as u64);
        for q in 0..4 {
            let unit = random_unit(&mut rng);
            match (i + q) % 11 {
                0 => match ix.offset_to_line_col(rng.below(len + 1), unit) {
                    Ok(p) => {
                        h.word(p.line as u64);
                        h.word(p.col as u64);
                    }
                    Err(e) => h.err(&e),
                },
                1 => h.num(ix.line_col_to_offset(lc(rng.below(lines + 1), rng.below(60)), unit)),
                2 => h.word(ix.line_col_to_offset_clamped(lc(rng.below(lines + 2), rng.below(60)), unit) as u64),
                // The line of a random offset (often a long one), at a column
                // anywhere in it.
                9 | 10 => {
                    let line = match ix.offset_to_line_col(rng.below(len + 1), Unit::Byte) {
                        Ok(p) => p.line,
                        Err(_) => rng.below(lines),
                    };
                    let col = rng.below(1_500_000);
                    if (i + q) % 2 == 0 {
                        h.num(ix.line_col_to_offset(lc(line, col), unit));
                    } else {
                        h.word(ix.line_col_to_offset_clamped(lc(line, col), unit) as u64);
                    }
                }
                3 => h.num(ix.utf16_to_offset(rng.below(len16 + 2))),
                4 => h.num(ix.offset_to_utf16(rng.below(len + 2))),
                5 => {
                    let l = rng.below(lines + 1);
                    h.num(ix.line_start(l));
                    h.num(ix.line_end(l));
                }
                6 => h.num(ix.line_len(rng.below(lines + 1), unit)),
                7 => {
                    let p = rng.below(len + 1);
                    h.text(ix.slice(Pos::Byte(p), Pos::Byte(p + rng.below(30))));
                }
                _ => {
                    let line = match ix.offset_to_line_col(rng.below(len + 1), Unit::Char) {
                        Ok(p) => p.line,
                        Err(_) => rng.below(lines),
                    };
                    h.num(ix.line_len(line, unit));
                }
            }
        }
        // Anchors: one added, one removed, two looked up.
        let bias = if i % 2 == 0 { Bias::After } else { Bias::Before };
        match ix.add_anchor(Pos::Utf16(rng.below(len16 + 1)), bias) {
            Ok(id) => {
                h.word(id.0);
                ids += 1;
            }
            Err(e) => h.err(&e),
        }
        h.word(ix.remove_anchor(AnchorId(rng.below(ids + 1) as u64)) as u64);
        for _ in 0..2 {
            h.word(ix.anchor_offset(AnchorId(rng.below(ids + 1) as u64)).map_or(u64::MAX, |o| o as u64));
        }
        h.word(ix.anchor_count() as u64);
        // Display columns, often on a long line.
        match ix.offset_to_line_col(rng.below(len + 1), Unit::Display) {
            Ok(p) => {
                h.word(p.line as u64);
                h.word(p.col as u64);
                h.num(ix.line_col_to_offset(lc(p.line, rng.below(p.col + 2)), Unit::Display));
            }
            Err(e) => h.err(&e),
        }
    }
    let end = ix.text();
    assert!(started.elapsed() < TIME_LIMIT, "rule 34: the workload took more than {TIME_LIMIT:?}");
    for b in end.bytes() {
        h.word(b as u64);
    }
    assert_eq!(h.0, EXPECTED_WORKLOAD_HASH, "rule 34: some answer in the workload is wrong");
}

const EXPECTED_WORKLOAD_HASH: u64 = 1_443_080_889_920_164_580;
