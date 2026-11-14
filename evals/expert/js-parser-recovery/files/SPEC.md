# Expression language: target behaviour

`parser.js` exports `tokenize`, `parse` and `show`. This document describes
what `parse` must do. Positions are 0-based indexes into the source string.

## Result of `parse`

`parse(src)` never throws for any string. It returns

    { ast, errors }

`ast` is always a tree (see "Tree"). `errors` is an array of
`{ code, pos }` objects sorted by `pos`, smallest first. For a source
without mistakes `errors` is empty.

There is at most one error per position: when an error would be reported at
a position that already has one, the later one is dropped.

## Tokens

- A number is one or more digits, optionally followed by `.` and one or more
  digits: `12`, `3.25`. In `1.` and `.5` the `.` is not part of a number.
- A name is a letter or `_` followed by letters, digits and `_`.
- The operators and punctuation are `+ - * / ** ?? ( ) ,`. The longest
  token wins: `**` is one token, and `***` is `**` followed by `*`.
- Spaces, tabs and newlines separate tokens and are otherwise ignored.
- Any other character is invalid. This includes `.` outside a number and a
  single `?`. Each invalid character is reported as `E_CHAR` at its position
  and then treated like a space, so `1 $ + 2` yields the tree of `1 + 2` and
  `1$2` is the two tokens `1` and `2`.

## Grammar

From lowest to highest precedence:

    expr     := coalesce
    coalesce := additive ( "??" additive )*         left-associative
    additive := term ( ("+" | "-") term )*          left-associative
    term     := unary ( ("*" | "/") unary )*        left-associative
    unary    := "-" unary | power
    power    := primary ( "**" unary )?             right-associative
    primary  := NUMBER
              | NAME
              | NAME "(" ( expr ( "," expr )* )? ")"
              | "(" expr ")"

So `**` binds tighter than unary minus on its left (`-2 ** 2` is
`-(2 ** 2)`), its right operand may itself start with a minus
(`2 ** -1`), and `a ** b ** c` is `a ** (b ** c)`. `??` binds loosest:
`a ?? b + 1` is `a ?? (b + 1)`, and `a ?? b ?? c` is `(a ?? b) ?? c`.

## Tree

    { type: 'num',  value }            value is a JavaScript number
    { type: 'var',  name }
    { type: 'neg',  arg }
    { type: 'bin',  op, left, right }  op is '+', '-', '*', '/', '**' or '??'
    { type: 'call', name, args }       args is an array
    { type: 'error' }                  stands in for a missing operand

A parenthesised group has no node of its own. `show` renders an error node
as `<error>`; all other nodes are rendered as they are today, for example
`(** 2 (neg x))` and `(?? a b)`.

## Syntax errors and recovery

After an error the parser carries on as described here, so that later
mistakes are reported too. "The end of the input" has position
`src.length`.

**`E_OPERAND`: an operand is missing.** Where the grammar requires a
`primary` and the next token is not a number, a name or `(`, report
`E_OPERAND` at the position of that token (or of the end of the input). Do
not consume the token. The missing operand becomes an `error` node and
parsing continues as if an operand had been read.

**`E_CLOSE`: a `)` is missing.** This applies in two places: after the
expression of a parenthesised group when the next token is not `)`, and
after an argument of a call when the next token is neither `,` nor `)`.
Report `E_CLOSE` at the position of that token (or of the end of the
input). Then skip tokens up to and including the `)` that closes the group
or call: while skipping, every `(` opens a nested level and every `)`
closes one, and the `)` met outside any nested level is the one that ends
the skipping. If the input ends first, skipping stops there. Skipped tokens
produce no syntax errors (their `E_CHAR` errors, if any, are still
reported). The group yields the expression that was parsed before the
error; the call keeps the arguments parsed before the error.

**`E_EXTRA`: something follows the expression.** When the whole expression
has been parsed and the next token is not the end of the input, report
`E_EXTRA` at that token's position. Everything from there on is ignored,
except that invalid characters there are still reported as `E_CHAR`.

## Examples

| source | tree as shown by `show` | errors |
|---|---|---|
| `2 ** 3 ** 2` | `(** 2 (** 3 2))` | none |
| `1 +` | `(+ 1 <error>)` | `E_OPERAND` at 3 |
| `(1 + 2` | `(+ 1 2)` | `E_CLOSE` at 6 |
| `f(1 2, 3) * 4` | `(* (call f 1) 4)` | `E_CLOSE` at 4 |
| `1 2` | `1` | `E_EXTRA` at 2 |
| `(1 + ) * 2 # 3` | `(* (+ 1 <error>) 2)` | `E_OPERAND` at 5, `E_CHAR` at 11, `E_EXTRA` at 13 |
| `1 + )` | `(+ 1 <error>)` | `E_OPERAND` at 4 (the `E_EXTRA` at 4 is dropped) |
