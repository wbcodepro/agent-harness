# kvlog design

## Log format (unchanged)

- The input is split into lines at `\n`. Lines are numbered from 1.
- A line that is empty or holds only spaces is skipped. So is a line whose
  first character is `#`. Skipped lines still count for line numbers.
- Any other line is a sequence of fields separated by one or more spaces.
  Leading and trailing spaces are allowed.
- A field is `key=value`. A key is one or more of the ASCII characters
  `A-Z a-z 0-9 _ . -`.
- A bare value runs up to the next space or the end of the line. It may be
  empty and may contain any character except a space, including `"`, `=`
  and `\`, which have no special meaning in it.
- A value that starts with `"` is quoted and ends at the next `"` that is
  not escaped. Inside the quotes `\\`, `\"`, `\n` and `\t` stand for a
  backslash, a quote, a newline and a tab. The closing quote must be
  followed by a space or the end of the line.

Errors (`ParseError { line, col, kind }`, both numbers 1-based, `col`
counted in characters, not bytes):

| kind | when | col |
|---|---|---|
| `BadKey` | a field does not start with a key character | that character |
| `MissingEquals` | the key is not followed by `=` | the character after the key (one past the end of the line if the line ends there) |
| `BadEscape` | in a quoted value, `\` is followed by anything but `\`, `"`, `n`, `t`, or by the end of the line | the backslash |
| `UnterminatedQuote` | the line ends inside a quoted value | the opening quote |
| `TrailingAfterQuote` | the closing quote is followed by a character other than a space | that character |

A line is read left to right and the first error found is the one reported.

## Zero-copy API (to be built)

Today every layer copies: the lexer collects `char`s and builds `String`s,
records own their text, the index clones whole records, and the getters
clone again. The goal is that parsing borrows from the input buffer and
nothing downstream copies. The public API becomes:

```rust
// record.rs
pub struct Field<'a> {
    pub key: &'a str,
    pub value: Cow<'a, str>,          // std::borrow::Cow
}
pub struct Record<'a> { /* private */ }
impl<'a> Record<'a> {
    pub fn new(line: usize, raw: &'a str, fields: Vec<Field<'a>>) -> Record<'a>;
    pub fn line(&self) -> usize;
    pub fn raw(&self) -> &'a str;
    pub fn fields(&self) -> &[Field<'a>];
    pub fn get(&self, key: &str) -> Option<&str>;   // first field with this key
}

// parser.rs
pub fn parse(input: &str) -> Result<Vec<Record<'_>>, ParseError>;
pub fn records(input: &str) -> Records<'_>;
// Records<'a> implements Iterator<Item = Result<Record<'a>, ParseError>>.

// index.rs
pub struct Index<'r, 'a> { /* private */ }
impl<'r, 'a> Index<'r, 'a> {
    pub fn build(records: &'r [Record<'a>], key: &str) -> Index<'r, 'a>;
    pub fn get(&self, value: &str) -> &[&'r Record<'a>];
    pub fn values(&self) -> Vec<&'r str>;
}

// stats.rs
pub fn distinct_keys<'a>(records: &[Record<'a>]) -> Vec<&'a str>;
```

`Field`, `Record`, `Records`, `Index`, `parse`, `records`, `distinct_keys`,
`ParseError` and `ErrorKind` are exported from the crate root. `Field` and
`Record` keep deriving `Debug`, `Clone`, `PartialEq` and `Eq`. The lexer is
internal; shape it as you see fit.

### Rules

1. **Borrowed text.** `Record::raw()` and every `Field::key` are sub-slices
   of the input string passed to `parse`/`records`: same memory, not equal
   copies.
2. **Values.** A bare value is `Cow::Borrowed` and a sub-slice of the input
   (an empty bare value is `Cow::Borrowed("")`). A quoted value that
   contains no backslash is `Cow::Borrowed` and is the sub-slice between
   the quotes. Only a quoted value that contains at least one escape is
   `Cow::Owned`, holding the unescaped text.
3. **Lifetimes.** Everything borrowed from the input is usable for as long
   as the input is, no matter what else has been dropped: a `&str` obtained
   from `raw()`, from a field's key, from inside a `Cow::Borrowed` value, or
   from `distinct_keys` must outlive the `Record`s, the `Vec` holding them,
   the `Records` iterator and any `Index`.
4. **`records`.** The iterator yields one item per line that is not
   skipped, in order: `Ok(record)`, or `Err(error)` for a line that does not
   parse, after which it carries on with the next line. `parse` returns the
   records, or the first error.
5. **`Index`.** `build` does not copy records. `get` returns references to
   the very elements of the slice given to `build`, in slice order, and an
   empty slice for an unknown value. `values` returns the distinct values of
   the key, sorted, each borrowed from one of the records. What `get` and
   `values` return stays usable after the `Index` is dropped, for as long as
   the slice of records is borrowed.
6. **`distinct_keys`** returns the distinct keys, sorted, as sub-slices of
   the input.
7. No `unsafe`, and no leaking of memory to obtain long lifetimes.
8. The format and the errors above are unchanged. Mind that `col` counts
   characters while slices are cut at byte offsets.
