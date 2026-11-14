# textpos: specification

The crate `textpos` provides `TextIndex`, an editable text that converts
quickly between byte offsets, line/column positions and UTF-16 offsets.
The public API is fixed: the types `Unit`, `LineCol`, `Pos`, `PosError`,
`Bias` and `AnchorId` and the methods of `TextIndex` named in rules 11 to
24 and 29 to 31, with exactly the signatures given in `src/lib.rs`. Only the Rust standard library may be
used; add no dependencies.

Every rule below is binding. Where an example and a rule seem to disagree,
the rule wins (the examples have been checked against the rules). In the
examples, `"\r"` is CR (U+000D) and `"\n"` is LF (U+000A).

## Text and measures

1. A `TextIndex` holds one valid UTF-8 text. `TextIndex::new(text)` creates
   an index holding a copy of `text`; `text()` returns the whole current
   text as a `String`.
2. A piece of text is measured in one of four units: `Unit::Byte` counts
   UTF-8 bytes; `Unit::Utf16` counts UTF-16 code units, which is one for a
   character below U+10000 and two (a surrogate pair) for a character from
   U+10000 up; `Unit::Char` counts Unicode scalar values (Rust `char`s).
   For example `"aé€😀"` is 10 bytes, 5 UTF-16 units and 4 chars.
   `Unit::Display` counts *display columns* and is used only for columns
   (rules 10, 12, 14, 15 and 19): reading a line from its start at column
   0, a tab (U+0009) moves the column from `c` to the next multiple of 4
   that is greater than `c` (so from 0, 1, 2 or 3 to 4, and from 4 to 8),
   and every other character moves it from `c` to `c + 1`, whatever its
   size. So `"a\tb"` is 5 display columns wide and the `b` starts at
   column 4, `"abcd\t"` is 8 wide, and `"\t\t"` is 8 wide. A character
   *covers* the columns from where it starts up to where it ends, so a tab
   that starts at column 1 covers columns 1 to 4, and columns 2 and 3 fall
   *inside* it.
3. A *byte offset* is a number of bytes from the start of the text. It is
   *valid* when it is at most `len_bytes()` and does not fall inside the
   UTF-8 encoding of a character (what `str::is_char_boundary` accepts).
4. A *UTF-16 offset* is a number of UTF-16 code units from the start of the
   text. It is *valid* when it is at most `len_utf16()` and does not fall
   between the two code units of a surrogate pair. The valid UTF-16 offset
   that corresponds to a valid byte offset `o` is the number of UTF-16
   units in the text before `o`; this is a one-to-one correspondence.

## Lines

5. A *line break* is one of: the two characters CR LF (`"\r\n"`), which
   together are a single break; an LF that does not follow a CR; a CR that
   is not followed by an LF (also a CR that is the last character of the
   text). No other character is a line break: U+2028, U+0085, form feed and
   vertical tab are ordinary characters.
6. The text is divided into lines by its breaks: a text with `k` breaks
   has `k + 1` lines. So `""` has one empty line; `"a\n"` has the two lines
   `a` and the empty line; `"\r\r\n"` has three lines; `"\n\r"` has three
   lines; `"a\r\nb"` has two lines.
7. Lines are numbered from 0 and columns are counted from 0.
8. `line_start(l)` is the byte offset of the first byte of line `l`: 0 for
   line 0, otherwise the offset just after the `l`-th break (counting breaks
   from 1). `line_end(l)` is the byte offset at which the break that ends
   line `l` begins, or `len_bytes()` for the last line. The *content* of
   line `l` is the text from `line_start(l)` to `line_end(l)`; it never
   contains a CR or an LF. For `"ab\r\ncd\ref\n\ng"` the line starts are
   0, 4, 7, 10, 11 and the line ends are 2, 6, 9, 10, 12.
9. The byte offset between the CR and the LF of a CR LF break is *inside a
   line break*. No other offset is: the offset just after a lone CR is the
   start of the next line. An offset inside a line break is still a valid
   byte offset (rule 3); only `offset_to_line_col` refuses it (rule 13).
10. The *line and column* of a valid byte offset `o` that is not inside a
    line break: the line is the number of breaks that end at or before `o`,
    and the column is the measure, in the requested unit, of the text from
    `line_start(line)` to `o` (for `Unit::Display`, the display column at
    which `o` lies when the line is read from column 0). So `line_end(l)` has the column equal to the
    length of line `l`, and the offset just after a break has column 0 of
    the next line.

## Queries

11. `len_bytes()`, `len_utf16()` and `len_chars()` return the measure of the
    whole text in each of the first three units (there is no display
    length of a whole text). `line_count()` returns the number of lines,
    which is at least 1.
12. `line_start(line)`, `line_end(line)` and `line_len(line, unit)` return
    the values of rule 8; `line_len` is the measure of the line's content in
    `unit`. For `line >= line_count()` each returns
    `Err(LineOutOfRange { line, count })` with `count = line_count()`.
13. `offset_to_line_col(offset, unit)` returns the line and column of rule
    10 as `LineCol { line, col }`. Its errors, checked in this order:
    `offset > len_bytes()` gives `ByteOutOfRange { offset, len }` with
    `len = len_bytes()`; an offset inside a character gives
    `NotCharBoundary { offset }`; an offset inside a line break gives
    `InsideLineBreak { offset }`.
14. `line_col_to_offset(pos, unit)` returns the byte offset `o` on line
    `pos.line` for which the measure in `unit` of the text from
    `line_start(pos.line)` to `o` is `pos.col`. Its errors, checked in this
    order: `pos.line >= line_count()` gives `LineOutOfRange { line, count }`;
    `pos.col` greater than `line_len(pos.line, unit)` gives
    `ColumnOutOfRange { line, col, len }` with `len = line_len(pos.line,
    unit)`; a column that falls inside a character (for `Unit::Byte`,
    inside its UTF-8 encoding; for `Unit::Utf16`, between the two halves of
    a surrogate pair; for `Unit::Display`, inside a tab as rule 2 defines
    it) gives `InsideCharacter { line, col }`. With `Unit::Char` there is
    never an `InsideCharacter`. A column never
    reaches into the line break: `pos.col == line_len(..)` gives
    `line_end(pos.line)`.
15. `line_col_to_offset_clamped(pos, unit)` never fails. If
    `pos.line >= line_count()` it returns `len_bytes()`. Otherwise, if
    `pos.col >= line_len(pos.line, unit)` it returns `line_end(pos.line)`;
    if the column falls inside a character (as in rule 14) it returns the
    offset where that character starts; otherwise it returns what `line_col_to_offset` would.
16. `offset_to_utf16(offset)` returns the UTF-16 offset of rule 4. Its
    errors, in this order: `ByteOutOfRange { offset, len }` and
    `NotCharBoundary { offset }`, as in rule 13. An offset inside a line
    break is accepted.
17. `utf16_to_offset(offset)` returns the byte offset that corresponds to
    the UTF-16 offset. Its errors, in this order: `offset > len_utf16()`
    gives `Utf16OutOfRange { offset, len }` with `len = len_utf16()`; an
    offset between the two halves of a surrogate pair gives
    `InsideSurrogatePair { offset }`. A result inside a line break is fine.
18. `slice(start, end)` returns the text between two positions: the bytes
    from `s` to `e`, where `s` and `e` are resolved by rules 19 and 20.

## Positions and edits

19. A `Pos` resolves to a byte offset as follows. `Pos::Byte(o)` is `o`
    itself, with the errors of rule 16 (an offset inside a line break is
    accepted). `Pos::Utf16(u)` is `utf16_to_offset(u)`, with its errors.
    `Pos::LineCol(lc, unit)` is `line_col_to_offset(lc, unit)`, with its
    errors; it is never clamped.
20. A range `(start, end)` is resolved like this: `start` first, and if it
    fails its error is returned; then `end`, and if it fails its error is
    returned; then, if `s > e`, the error is `ReversedRange { start: s,
    end: e }` with the two resolved byte offsets. `s == e` is an empty
    range. All positions of one call are resolved against the text as it
    is before that call.
21. `insert(at, text)` inserts `text` at the resolved offset `o` and returns
    `Ok(o)`. Inserting `""` is allowed and leaves the text unchanged.
22. `delete(start, end)` removes the bytes from `s` to `e` and returns the
    removed text.
23. `replace(start, end, text)` removes the bytes from `s` to `e`, inserts
    `text` at `s`, and returns the removed text.
24. A call that returns an error changes nothing.
25. After every edit, every query answers for the new text exactly as rules
    1 to 18 say for that text, as if the index had been created anew from
    it. Edits can join a CR and an LF into one break or separate them:
    inserting `"x"` at the offset inside the break of `"a\r\nb"` gives
    `"a\rx\nb"`, which has three lines; deleting `"b"` from `"a\rb\nc"`
    gives `"a\r\nc"`, which has two; appending `"\n"` to `"x\r"` gives one
    CR LF break; deleting the LF of a CR LF break leaves a lone CR, which
    is still a break. The same holds for breaks formed by text that was
    inserted in several pieces.

## Errors

26. `PosError` in `src/lib.rs` is the complete list of errors. Keep its
    variants and field names. Every error carries exactly the values the
    rules name: offsets, lines and columns as the caller passed them, and
    `len` and `count` as the current totals.
27. When several errors apply to one call, the one reported is the first in
    the order given by the rule for that call. For the text `"a😀\r\nb"`
    (8 bytes, 6 UTF-16 units, lines `a😀` and `b`):

    | call | result |
    |---|---|
    | `offset_to_line_col(99, Byte)` | `ByteOutOfRange { offset: 99, len: 8 }` |
    | `offset_to_line_col(2, Byte)` | `NotCharBoundary { offset: 2 }` |
    | `offset_to_line_col(6, Byte)` | `InsideLineBreak { offset: 6 }` |
    | `offset_to_line_col(5, Utf16)` | `Ok(LineCol { line: 0, col: 3 })` |
    | `offset_to_utf16(6)` | `Ok(4)` |
    | `utf16_to_offset(2)` | `InsideSurrogatePair { offset: 2 }` |
    | `line_col_to_offset({0, 2}, Utf16)` | `InsideCharacter { line: 0, col: 2 }` |
    | `line_col_to_offset({0, 4}, Utf16)` | `ColumnOutOfRange { line: 0, col: 4, len: 3 }` |
    | `line_col_to_offset({0, 2}, Char)` | `Ok(5)` |
    | `line_col_to_offset({5, 0}, Byte)` | `LineOutOfRange { line: 5, count: 2 }` |
    | `line_col_to_offset_clamped({0, 2}, Byte)` | `1` |
    | `line_col_to_offset_clamped({0, 9}, Byte)` | `5` |
    | `line_col_to_offset_clamped({7, 0}, Char)` | `8` |
    | `delete(Byte(2), Byte(99))` | `NotCharBoundary { offset: 2 }` |
    | `delete(Byte(99), Byte(2))` | `ByteOutOfRange { offset: 99, len: 8 }` |
    | `delete(Byte(5), Byte(1))` | `ReversedRange { start: 5, end: 1 }` |

28. No method panics, whatever the argument values, including `usize::MAX`
    for any offset, line or column.

## Anchors

29. `add_anchor(at, bias)` places an *anchor* at the byte offset that `at`
    resolves to by rule 19 (with the errors of rule 19, and a call that
    fails changes nothing) and returns its id. The first anchor ever added
    to an index gets `AnchorId(0)`, and each later one the next number;
    a failed call uses up no id, and ids are never reused, also after
    `remove_anchor`.
30. `anchor_offset(id)` returns the current byte offset of the anchor, or
    `None` if no anchor with that id exists (never added, or removed).
    `remove_anchor(id)` removes the anchor and returns `true`, or returns
    `false` and does nothing if no anchor with that id exists.
    `anchor_count()` returns the number of anchors that exist.
31. A successful `insert`, `delete` or `replace` removes the bytes from `s`
    to `e` and puts `k` bytes in their place (for `insert`, `s == e`; for
    `delete`, `k = 0`). It moves every anchor, whose offset is `a` before
    the edit, as follows:
    - if `a < s`, the anchor stays at `a`;
    - if `a > e`, it moves to `a - (e - s) + k`;
    - if `s <= a <= e` (also at either end of the removed bytes, and at the
      offset of an insert), it moves to `s` if its bias is `Bias::Before`
      and to `s + k` if its bias is `Bias::After`.

    So an anchor at the offset of an insert stays in front of the inserted
    text with `Before` and ends up behind it with `After`; an edit that
    changes nothing (inserting `""`, deleting an empty range) moves
    nothing; and a call that fails moves nothing. Example: in
    `"hello world"` with anchors at 5 (`Before`), 5 (`After`) and 11,
    inserting `","` at 5 puts them at 5, 6 and 12; then deleting the bytes
    from 3 to 7 (`"lo, "`) puts them at 3, 3 and 8.
32. An anchor's offset is always a valid byte offset (rule 3) of the
    current text, but it can be inside a line break (rule 9): an `After`
    anchor at 1 in `"\rx\n"` is still at 1 after deleting the `x`.

## Complexity

33. Let `n` be the current length of the text in bytes. `new` takes O(n)
    time and `text()` O(n). `slice` takes O(log n + the length of the
    result). `len_bytes`, `len_utf16`, `len_chars` and `line_count` take
    O(log n) or less. Every other query takes O(log n) time, in every unit:
    in particular its cost must not grow with the length of the text, the
    number of lines or the length of a line, also for `Unit::Display`, where
    the column of a character depends on the tabs before it on its line.
    `insert`, `delete` and `replace` take O(log n + k + log A + m) time,
    where `k` is the number of bytes inserted plus removed, `A` the number
    of anchors and `m` the number of anchors at offsets from `s` to `e`
    (rule 31). `add_anchor` takes the time of resolving its position plus
    O(log A); `anchor_offset` and `remove_anchor` take O(log A);
    `anchor_count` O(1). Memory stays O(n + A).
34. Concretely, this workload must finish in under 20 seconds in a release
    build (`cargo test --release`) on an ordinary computer, with every
    answer correct: `TextIndex::new` on a text of 8 MiB with about 300,000
    lines, mostly ASCII with some tabs and some 2-, 3- and 4-byte characters
    and with LF, CR LF and lone CR breaks, where three of the lines are each
    about a million bytes long; then `add_anchor` at a million offsets
    spread over the text; then 100,000 rounds, each making one edit
    (`insert`, `delete` or `replace` of at most 16 bytes, given by any kind
    of `Pos` and any unit, at positions spread over the whole text) followed
    by one `add_anchor`, one `remove_anchor`, two `anchor_offset` and one
    `anchor_count`, then the `len_*` and `line_count` queries and at least
    six further queries of rules 12 to 18, in all four units, at positions
    spread over the whole text (so many of them fall on the long lines);
    then one call of `text()`. A suitable implementation does this in about
    two seconds.
