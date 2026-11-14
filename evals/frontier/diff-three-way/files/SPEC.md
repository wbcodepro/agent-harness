# Three-way text merge: specification

`merge3.py` must provide the function

    merge3(base, ours, theirs, *, style="merge", ignore_ws=False,
           labels=("ours", "base", "theirs"))  ->  (text, conflicts)

and the command line of rules 25 to 29. `base` is the common ancestor,
`ours` and `theirs` are the two edited versions, all three are `str`.
`text` is the merged text (`str`) and `conflicts` the number of conflict
blocks in it (`int`). Only the Python standard library may be used.

Every rule below is binding. Where an example and a rule seem to disagree,
the rule wins (and the examples have been checked against the rules).

## Lines

1. A text is split into lines at `"\n"` and at nothing else. `"\r"`,
   form feed, vertical tab, U+0085, U+2028 and every other character are
   ordinary characters of a line. The content of a line never includes
   the `"\n"`.
2. A `"\n"` at the very end of a text does not start another line:
   `"a\nb\n"` and `"a\nb"` both have the two lines `a`, `b`; `"a\n\n"` has
   the two lines `a` and the empty line; `"\n"` has one empty line; `""`
   has no lines.
3. A text *ends with a newline* when its last character is `"\n"`. The
   empty text `""` also counts as ending with a newline.

## Comparing lines

4. Every line has a *key*. Without `ignore_ws` the key is the content.
   With `ignore_ws=True` the key is the content with leading and trailing
   blanks removed and every remaining run of one or more blanks replaced
   by one space. A *blank* is a space, a tab or a carriage return
   (U+0020, U+0009, U+000D) and nothing else: U+00A0, U+3000, form feed
   and vertical tab are not blanks.
5. Two lines are *equal* when their keys are equal. Two lists of lines are
   equal when they have the same length and their lines are equal pair by
   pair. Every comparison in rules 6 to 17 means this equality, unless the
   rule says "exactly".

## Alignment

6. An *alignment* of the base lines `A[0..n)` with the lines `B[0..m)` of
   another text is a set of pairs `(i, j)` with `A[i]` equal to `B[j]`,
   in which both `i` and `j` strictly increase from one pair to the next,
   and which has as many pairs as possible (a longest common subsequence).
7. There are usually several such sets. The one to use, the *canonical
   alignment*, is defined by this walk. Let `L(i, j)` be the size of a
   longest common subsequence of `A[i..n)` and `B[j..m)`. Start at
   `(i, j) = (0, 0)` and repeat while `i < n` and `j < m`:
   - if `A[i]` equals `B[j]`: take the pair `(i, j)` and go to
     `(i+1, j+1)`;
   - otherwise, if `L(i+1, j) >= L(i, j+1)`: go to `(i+1, j)` (the base
     line stays unpaired);
   - otherwise go to `(i, j+1)`.

   Examples (each letter is a line): for `A = b a a`, `B = a` the pair is
   `(1, 0)`, not `(2, 0)`. For `A = x y`, `B = y x` the pair is `(1, 0)`.
   For `A = a`, `B = a a` the pair is `(0, 0)`.
8. Two canonical alignments are computed: `O` between base and ours, and
   `T` between base and theirs. The base is always the `A` side.

## Chunks

9. A base line is *stable* when it has a pair in `O` and a pair in `T`.
10. The stable lines cut the three texts into gaps: before the first
    stable line, between two consecutive stable lines, and after the last
    one. A gap consists of the base lines, the ours lines and the theirs
    lines that lie strictly between the two stable lines (or between a
    stable line and the start or end of the texts), using the partners
    of the stable lines in ours and theirs. If there is no stable line
    at all, the whole of the three texts is one gap.
11. A gap whose three line lists are all empty produces nothing. Any other
    gap is a *change chunk* `(B, X, Y)`: its base, ours and theirs lines.
    Changes of the two sides that have no stable line between them are
    therefore in one chunk, even when they touch different base lines.

## Output for stable lines and chunks

The merged text is built in order: stable lines and chunks alternate as
they occur.

12. A stable line outputs one line. If the ours line paired with it is
    exactly (character for character) the content of the base line, the
    content of the theirs line is output; otherwise the content of the
    ours line. (This matters only with `ignore_ws`.)
13. A chunk in which `X` equals `Y` outputs the lines of `X` (ours
    content). Both sides made the same change; it is taken once and is
    not a conflict.
14. Otherwise a chunk in which `X` equals `B` outputs the lines of `Y`.
15. Otherwise a chunk in which `Y` equals `B` outputs the lines of `X`.
16. Any other chunk is a conflict and outputs one conflict block (rules
    17 to 20). `conflicts` is the number of conflict blocks output.

## Conflict blocks

17. With `style="merge"` the two sides are trimmed first. Let `P` be the
    longest common prefix of `X` and `Y` (as lists of lines). Remove it
    from both. Then let `S` be the longest common suffix of what remains
    of `X` and `Y`, and remove it from both. Output the lines of `P`
    (ours content) as ordinary lines, then the block

        <<<<<<< OURS-LABEL
        (remaining lines of X)
        =======
        (remaining lines of Y)
        >>>>>>> THEIRS-LABEL

    then the lines of `S` (ours content) as ordinary lines. The base
    lines `B` do not appear and play no part in the trimming.
18. With `style="diff3"` nothing is trimmed and the base is shown:

        <<<<<<< OURS-LABEL
        (lines of X)
        ||||||| BASE-LABEL
        (lines of B)
        =======
        (lines of Y)
        >>>>>>> THEIRS-LABEL

19. A marker line is exactly seven marker characters followed, when the
    label is not empty, by one space and the label. With an empty label
    there is no space. The `=======` line never has a label. `labels` is
    `(ours, base, theirs)`; the base label is used only by `diff3`.
20. A side of a block may be empty; then no line stands between its two
    markers.

## Building the text

21. Every output line, marker lines included, is followed by `"\n"`, with
    the one exception of rule 23. Lines inside a conflict block always
    get their `"\n"`, also a line that was the last line of its text and
    had none there.
22. The *merged final newline* is decided like a three-way merge of the
    three answers to "does the text end with a newline" (rule 3): if ours
    and theirs agree, it is their answer; otherwise it is the answer of
    the side that differs from the base.
23. If the last output line is an ordinary line (not a marker) and the
    merged final newline is "no", the last `"\n"` is left out. If the
    last output line is a marker it keeps its `"\n"` whatever rule 22
    says. With no output lines the result is `""`.

## Arguments

24. `style` other than `"merge"` or `"diff3"` raises `ValueError`.
    `labels` must be a tuple or list of exactly three `str`, none
    containing `"\n"`; anything else raises `ValueError`. A `base`,
    `ours` or `theirs` that is not a `str` raises `TypeError`.

## Command line

25. `python3 merge3.py [--diff3] [--ignore-ws] [-L LABEL]... [-o FILE]
    OURS BASE THEIRS` merges the three files (note the order of the
    file arguments) and writes the merged text to standard output, or to
    `FILE` when `-o FILE` is given (then standard output stays empty).
26. `-L` may be given up to three times and sets the ours, base and
    theirs label in that order. Labels not given keep the defaults
    `ours`, `base`, `theirs`. Options may come before or after the file
    arguments.
27. The files are read as UTF-8 and the result is written as UTF-8,
    both without any newline translation: a `"\r\n"` in the input is the
    two characters `"\r"`, `"\n"`, a lone `"\r"` stays a `"\r"`, and
    nothing is added after the merged text.
28. Exit status: 0 when the merge has no conflicts, 1 when it has.
29. Exit status 2, with a message on standard error, nothing on standard
    output and the `-o` file neither created nor modified, when: the
    arguments are wrong (not exactly three files, an unknown option, a
    fourth `-L`), a file cannot be read, or a file is not valid UTF-8.

## Size and speed

30. Texts of up to 300 lines each may differ in any way; such a merge
    must finish within 2 seconds.
31. Texts may have up to 20,000 lines each. When ours and theirs can
    each be produced from base with at most 100 line insertions plus
    line deletions, `merge3` must finish within 10 seconds on an ordinary
    laptop. Work proportional to (number of lines) x (number of differing
    lines) is fine; a table of all `n x m` values `L(i, j)` is not. The
    result must still be exactly the one that rules 7 to 23 define, on
    texts with many repeated lines too. (A fact that helps: an alignment
    that leaves `d` lines unpaired in total never contains a pair
    `(i, j)` with `|i - j| > d`.)

## Examples

Lines are written one letter each, joined by spaces; every text below
ends with a newline and the default labels are used.

- base `a b c`, ours `a X c`, theirs `a b c d`: result `a X c d`,
  0 conflicts.
- base `a b c`, ours `a X c`, theirs `a Y c`: `style="merge"` gives

      a
      <<<<<<< ours
      X
      =======
      Y
      >>>>>>> theirs
      c

  with 1 conflict.
- base `a b c d`, ours `a X c d`, theirs `a b Y d` (the sides change
  adjacent lines): lines `b c` form one chunk with `X = X c`,
  `Y = b Y`; it is a conflict. `style="diff3"` gives

      a
      <<<<<<< ours
      X
      c
      ||||||| base
      b
      c
      =======
      b
      Y
      >>>>>>> theirs
      d

- base `a b`, ours `a p q r b`, theirs `a p z r b` (both insert at the
  same place): `style="merge"` gives `a p`, then a block with `q`
  against `z`, then `r b`; `style="diff3"` gives a block with `p q r`,
  an empty base section and `p z r`.
- base `a b c`, ours `a c`, theirs `a c`: result `a c`, 0 conflicts.
- base `"a\nb"` (no final newline), ours `"a\nb\n"`, theirs `"A\nb"`:
  result `"A\nb\n"`.
- base `"x\n"`, ours `"x  y\n"`, theirs `"x y \n"`, with `ignore_ws`:
  the chunk has `X` equal to `Y`, result `"x  y\n"`.
- base `"if a:\n"`, ours `"if a:\n"`, theirs `"if  a:\n"`, with
  `ignore_ws`: the line is stable and ours is exactly the base, so the
  result is `"if  a:\n"`.
