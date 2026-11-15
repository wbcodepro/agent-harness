"""X04: discrepancies.csv joins the price list (open in Numbers, reachable
only through the app) with the site's blue Tool items by id. It must list
exactly the ids whose prices differ or that appear on one side only, with
both prices (blank for the missing side), sorted by id. The price list
must be left as it was."""
import csv
import os
import sys
from decimal import Decimal, InvalidOperation

sys.path.insert(0, os.path.join(os.environ.get("HAME_SUITE_DIR", os.path.dirname(__file__) + "/../.."), "fixture"))
from office import Problem  # noqa: E402

HEADER = ["id", "site_price", "sheet_price"]
SHEET = {1: 8, 13: 45, 25: 26, 37: 12, 49: 44, 85: 15}


def site_items():
    """The fixture site's Tool items whose name contains blue, id -> price."""
    colors, categories = ["red", "blue", "green", "gold"], ["fruit", "tool", "book"]
    return {i: (i * 7) % 50 + 1 for i in range(1, 71)
            if categories[i % 3] == "tool" and colors[i % 4] == "blue"}


def expected():
    site = site_items()
    rows = []
    for i in sorted(set(site) | set(SHEET)):
        a, b = site.get(i), SHEET.get(i)
        if a != b:
            rows.append((i, a, b))
    return rows


def price(text):
    text = (text or "").strip()
    if not text:
        return None
    try:
        return Decimal(text)
    except InvalidOperation as e:
        raise Problem(f"{text!r} is not a price") from e


def verify_csv(path):
    if not os.path.isfile(path):
        raise Problem("discrepancies.csv was not written")
    with open(path, newline="", encoding="utf-8-sig") as f:
        rows = list(csv.reader(f))
    if not rows or [c.strip() for c in rows[0]] != HEADER:
        raise Problem(f"the header is {rows[:1]}, expected {HEADER}")
    got = []
    for r in rows[1:]:
        if not any(c.strip() for c in r):
            continue
        if len(r) != 3:
            raise Problem(f"row {r} does not have three columns")
        for cell in r[1:]:
            if cell.strip() and "." in cell and len(cell.strip().split(".")[1]) != 2:
                raise Problem(f"price {cell!r} is not written with two decimals")
            if cell.strip() and "." not in cell:
                raise Problem(f"price {cell!r} is not written with two decimals")
        got.append((int(r[0]), price(r[1]), price(r[2])))
    want = expected()
    if got != want:
        raise Problem(f"discrepancies are {got}; expected {want}")
    return f"{len(want)} discrepancies, joins and prices right"


def verify_sheet_unchanged(cells):
    for i, (ident, value) in enumerate(SHEET.items(), start=2):
        if cells.get(f"A{i}") != str(ident) or cells.get(f"C{i}") != str(value):
            raise Problem(f"the price list changed: row {i} shows {cells.get(f'A{i}')}, {cells.get(f'C{i}')}")


def main():
    from pilot import NUMBERS, app_cells, fail, ok, strays

    if strays():
        fail(f"saved outside the workspace: {strays()}")
    try:
        said = verify_csv("discrepancies.csv")
        verify_sheet_unchanged(app_cells(NUMBERS, "pricelist"))
    except Problem as e:
        fail(str(e))
    ok(said)


if __name__ == "__main__":
    main()
