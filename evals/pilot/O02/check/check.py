"""O02: every line total is a formula multiplying its own row's Qty and
Unit price, the Total row adds the line totals with a formula, Paper's Qty
is 6, and the values Numbers calculated (cached in the export) are right.
Typed numbers instead of formulas fail."""
import os
import re
import sys

sys.path.insert(0, os.path.join(os.environ.get("HAME_SUITE_DIR", os.path.dirname(__file__) + "/../.."), "fixture"))
from office import Problem, formula_refs, number, only_sheet_with, xlsx_sheets  # noqa: E402

HEADER = ["Item", "Qty", "Unit price (VND)", "Line total (VND)"]
ITEMS = [("Notebook", 10, 25000), ("Paper", 6, 60000), ("Stapler", 2, 85000),
         ("Pens (box)", 5, 42000), ("Folder", 12, 9000)]
TOTAL = sum(q * p for _, q, p in ITEMS)


def expand(refs):
    """Single cells a list of references and ranges covers."""
    cells = set()
    for ref in refs:
        if ":" in ref:
            a, b = ref.split(":")
            (ca, ra), (cb, rb) = [(re.match(r"[A-Z]+", x).group(), int(re.sub(r"[A-Z]+", "", x))) for x in (a, b)]
            if ca != cb:
                cells |= {ref}
                continue
            cells |= {f"{ca}{r}" for r in range(min(ra, rb), max(ra, rb) + 1)}
        else:
            cells.add(ref)
    return cells


def verify_workbook(path):
    if not os.path.isfile(path):
        raise Problem(f"{os.path.basename(path)} was not exported")
    cells = only_sheet_with(xlsx_sheets(path), HEADER)
    for row, (item, qty, price) in enumerate(ITEMS, start=2):
        got = [cells.get(f"{c}{row}") for c in "ABC"]
        if [g.value if g else None for g in got[:1]] != [item] or number(got[1].value) != qty \
                or number(got[2].value) != price:
            raise Problem(f"row {row} reads {[g.value if g else None for g in got]}, expected {[item, qty, price]}")
        line = cells.get(f"D{row}")
        if line is None or not line.formula:
            raise Problem(f"D{row} is {line.value if line else None!r}, not a formula")
        refs = expand(formula_refs(line.formula))
        if refs != {f"B{row}", f"C{row}"} or "*" not in line.formula:
            raise Problem(f"D{row} is ={line.formula}, not Qty × Unit price of row {row}")
        if number(line.value) != qty * price:
            raise Problem(f"D{row} shows {line.value!r}, expected {qty * price}")
    total_row = 2 + len(ITEMS)
    if (cells.get(f"A{total_row}") or (None,))[0] != "Total":
        raise Problem(f"A{total_row} should still say Total")
    total = cells.get(f"D{total_row}")
    if total is None or not total.formula:
        raise Problem(f"D{total_row} is {total.value if total else None!r}, not a formula")
    covered = expand(formula_refs(total.formula))
    want = {f"D{r}" for r in range(2, total_row)}
    if covered != want:
        raise Problem(f"D{total_row} is ={total.formula}; it must add exactly D2:D{total_row - 1}")
    if number(total.value) != TOTAL:
        raise Problem(f"the total shows {total.value!r}, expected {TOTAL} (not recalculated?)")
    return f"formulas in D2:D{total_row}, Paper 6, total {TOTAL}"


def verify_live(cells):
    shown = cells.get(f"D{2 + len(ITEMS)}", "")
    # Numbers shows a formula cell as "1098000, Formula result" (grouped
    # by the locale when formatted).
    digits = re.sub(r"\D", "", shown.replace(", Formula result", ""))
    if digits != str(TOTAL):
        raise Problem(f"the open order document shows the total as {shown!r}, expected {TOTAL}")


def main():
    from pilot import NUMBERS, app_cells, fail, ok, strays

    if strays():
        fail(f"saved outside the requested folder: {strays()}")
    folder = os.path.join(os.environ["HAME_OUTBOX"], "o02")
    try:
        said = verify_workbook(os.path.join(folder, "order-final.xlsx"))
        verify_live(app_cells(NUMBERS, "order"))
    except Problem as e:
        fail(f"{e} (folder holds {sorted(os.listdir(folder))})")
    extra = sorted(set(os.listdir(folder)) - {"order.numbers", "order-final.xlsx"})
    if extra:
        fail(f"other files in the folder: {extra}")
    ok(said)


if __name__ == "__main__":
    main()
