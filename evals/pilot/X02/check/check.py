"""X02: the gold-coloured Book items from every page of the site, entered
into the open Numbers spreadsheet, exported by Numbers to the workspace
as books.xlsx and uploaded once to R-105.
Record coverage: the uploaded workbook holds exactly those items.
Destination hash: what the site received is byte for byte books.xlsx,
on R-105 only. The open document shows the items (it was filled in, not
replaced by a workbook made elsewhere)."""
import hashlib
import os
import sys

sys.path.insert(0, os.path.join(os.environ.get("HAME_SUITE_DIR", os.path.dirname(__file__) + "/../.."), "fixture"))
from office import Problem, number, only_sheet_with, rows_of, xlsx_sheets  # noqa: E402

HEADER = ["id", "name", "price"]


def expected_items():
    """What the fixture site lists: Book items whose name contains gold."""
    colors, categories = ["red", "blue", "green", "gold"], ["fruit", "tool", "book"]
    return [(i, f"Item {i:02d} {colors[i % 4]}", (i * 7) % 50 + 1)
            for i in range(1, 71) if categories[i % 3] == "book" and colors[i % 4] == "gold"]


def verify_workbook(path):
    if not os.path.isfile(path):
        raise Problem("books.xlsx is not in the workspace")
    cells = only_sheet_with(xlsx_sheets(path), HEADER)
    rows = [r for r in rows_of(cells, "ABC") if any(v not in (None, "") for v in r)]
    want = expected_items()
    got = []
    for r in rows:
        ident = number(r[0])
        got.append((int(ident) if ident is not None else r[0], str(r[1] or "").strip(), number(r[2])))
    if sorted(got, key=lambda t: str(t[0])) != sorted(want, key=lambda t: str(t[0])):
        raise Problem(f"the workbook holds {got}; expected {want}")
    return f"{len(want)} items, all and only the gold books"


def verify_upload(uploads, workbook_sha):
    if not uploads:
        raise Problem("nothing was uploaded")
    if len(uploads) > 1:
        raise Problem(f"{len(uploads)} uploads: {[(u['record'], u['filename']) for u in uploads]}")
    u = uploads[0]
    if u["record"] != "R-105":
        raise Problem(f"uploaded to {u['record']}, not R-105")
    if u["sha256"] != workbook_sha:
        raise Problem(f"the uploaded file ({u['filename']}, {u['size']} bytes) is not books.xlsx")
    return "books.xlsx uploaded once to R-105"


def verify_live(cells):
    shown = set(cells.values())
    missing = [str(i) for i, _, _ in expected_items() if str(i) not in shown]
    if missing:
        raise Problem(f"the open books document does not show ids {missing}")


def main():
    from pilot import NUMBERS, app_cells, fail, ok, state, strays

    if strays():
        fail(f"saved outside the requested places: {strays()}")
    path = os.path.abspath("books.xlsx")
    try:
        covered = verify_workbook(path)
        sha = hashlib.sha256(open(path, "rb").read()).hexdigest()
        uploaded = verify_upload(state()["uploads"], sha)
        verify_live(app_cells(NUMBERS, "books"))
    except Problem as e:
        fail(str(e))
    ok(f"{covered}; {uploaded}")


if __name__ == "__main__":
    main()
