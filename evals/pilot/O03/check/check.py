"""O03: the three requested edits and nothing else. reference.docx is what
Pages exports for the untouched document; the result must read the same
with the edits applied, every character must keep the reference's bold,
italic, size and paragraph alignment, and the two documents rendered by
Quick Look must lay out the same lines in the same places."""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.environ.get("HAME_SUITE_DIR", HERE + "/../.."), "fixture"))
from office import Problem, docx_paragraphs, ink_bands, render  # noqa: E402

REFERENCE = os.path.join(HERE, "reference.docx")
EDITS = [("Q3 Operations Report", "Q4 Operations Report"),
         ("Headcount is 42.", "Headcount is 45."),
         ("96 percent", "97 percent")]
# Same-length edits: formatting is compared character by character.
assert all(len(a) == len(b) for a, b in EDITS)


def expected():
    out = []
    for text, marks, align in docx_paragraphs(REFERENCE):
        for old, new in EDITS:
            text = text.replace(old, new)
        out.append((text, marks, align))
    return [p for p in out if p[0].strip()]


def verify_docx(path):
    if not os.path.isfile(path):
        raise Problem(f"{os.path.basename(path)} was not exported")
    got = [p for p in docx_paragraphs(path) if p[0].strip()]
    want = expected()
    if [p[0] for p in got] != [p[0] for p in want]:
        raise Problem(f"the text reads {[p[0] for p in got]}, expected {[p[0] for p in want]}")
    for (text, marks, align), (_, want_marks, want_align) in zip(got, want):
        if align != want_align:
            raise Problem(f"{text[:30]!r} is aligned {align}, was {want_align}")
        # Sizes Pages leaves to the paragraph style come out as None; a
        # size written explicitly must match.
        diffs = [i for i, (a, b) in enumerate(zip(marks, want_marks))
                 if a[:2] != b[:2] or (a[2] or b[2]) and a[2] != b[2]]
        if diffs:
            i = diffs[0]
            raise Problem(f"formatting changed in {text[:40]!r} at {text[i:i + 12]!r}: "
                          f"(bold, italic, size) {marks[i]} was {want_marks[i]}")
    return "text updated, every character's formatting preserved"


def verify_rendering(path, tolerance=6):
    ref_bands, ref_w = ink_bands(render(REFERENCE))
    got_bands, got_w = ink_bands(render(path))
    if ref_w != got_w or len(ref_bands) != len(got_bands):
        raise Problem(f"rendered layout differs: {len(got_bands)} text lines, the original has {len(ref_bands)}")
    for i, (a, b) in enumerate(zip(got_bands, ref_bands)):
        if max(abs(x - y) for x, y in zip(a, b)) > tolerance:
            raise Problem(f"rendered line {i + 1} moved or resized: {a} vs {b} (top, bottom, left, right)")
    return f"{len(got_bands)} rendered lines in place"


def main():
    from pilot import fail, ok, strays

    if strays():
        fail(f"saved outside the requested folder: {strays()}")
    folder = os.path.join(os.environ["HAME_OUTBOX"], "o03")
    try:
        said = verify_docx(os.path.join(folder, "ops-report-q4.docx"))
        drawn = verify_rendering(os.path.join(folder, "ops-report-q4.docx"))
    except Problem as e:
        fail(f"{e} (folder holds {sorted(os.listdir(folder))})")
    extra = sorted(set(os.listdir(folder)) - {"ops-report.pages", "ops-report-q4.docx"})
    if extra:
        fail(f"other files in the folder: {extra}")
    ok(f"{said}; {drawn}")


if __name__ == "__main__":
    main()
