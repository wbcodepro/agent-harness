"""O01: the six receipts entered into the open Numbers spreadsheet, one row
each in order, amounts as numbers, nothing else added; the workbook
exported by Numbers into the outbox. The live document is read back
through the checkers' own accessibility tool, so a workbook built
elsewhere and exported does not pass."""
import os
import sys

sys.path.insert(0, os.path.join(os.environ.get("HAME_SUITE_DIR", os.path.dirname(__file__) + "/../.."), "fixture"))
from office import Problem, number, only_sheet_with, rows_of, xlsx_sheets  # noqa: E402

HEADER = ["Receipt", "Vendor", "Category", "Amount (VND)"]
RECEIPTS = [
    ["R-201", "Highlands Coffee", "Meals", 185000],
    ["R-202", "Grab", "Transport", 94000],
    ["R-203", "Fahasa Books", "Supplies", 356000],
    ["R-204", "Viettel", "Phone", 250000],
    ["R-205", "Pho 24", "Meals", 132000],
    ["R-206", "Vinasun Taxi", "Transport", 218000],
]


def verify_workbook(path):
    if not os.path.isfile(path):
        raise Problem(f"{os.path.basename(path)} was not exported")
    cells = only_sheet_with(xlsx_sheets(path), HEADER)
    rows = [r for r in rows_of(cells, "ABCD") if any(v not in (None, "") for v in r)]
    if len(rows) != len(RECEIPTS):
        raise Problem(f"{len(rows)} filled rows, expected {len(RECEIPTS)}: {rows}")
    for i, (got, want) in enumerate(zip(rows, RECEIPTS), start=2):
        texts = [str(v).strip() if v is not None else "" for v in got[:3]]
        if texts != want[:3]:
            raise Problem(f"row {i} reads {texts}, expected {want[:3]}")
        amount = number(got[3])
        if not isinstance(got[3], (int, float)) or amount != want[3]:
            raise Problem(f"row {i} amount is {got[3]!r}, expected the number {want[3]}")
    return "all six receipts in order with numeric amounts"


def verify_live(cells):
    """The open document's table, as the app shows it."""
    shown = set(cells.values())
    missing = [r[0] for r in RECEIPTS if r[0] not in shown]
    if missing:
        raise Problem(f"the open expenses document does not show {missing}")


def main():
    from pilot import NUMBERS, app_cells, fail, ok, strays

    if strays():
        fail(f"saved outside the requested folder: {strays()}")
    folder = os.path.join(os.environ["HAME_OUTBOX"], "o01")
    try:
        said = verify_workbook(os.path.join(folder, "expenses-filled.xlsx"))
        verify_live(app_cells(NUMBERS, "expenses"))
    except Problem as e:
        fail(f"{e} (folder holds {sorted(os.listdir(folder))})")
    extra = sorted(set(os.listdir(folder)) - {"expenses.numbers", "expenses-filled.xlsx"})
    if extra:
        fail(f"other files in the folder: {extra}")
    ok(said)


if __name__ == "__main__":
    main()
