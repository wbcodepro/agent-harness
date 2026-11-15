"""O01 checker, offline: synthetic workbooks, right and wrong."""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check  # noqa: E402
from office import Problem  # noqa: E402


def workbook(rows, header=check.HEADER):
    import openpyxl

    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = "Expenses"
    ws.append(header)
    for r in rows:
        ws.append(r)
    path = os.path.join(tempfile.mkdtemp(), "expenses-filled.xlsx")
    wb.save(path)
    return path


class O01(unittest.TestCase):
    def test_the_right_rows_pass(self):
        self.assertIn("six receipts", check.verify_workbook(workbook(check.RECEIPTS)))
        check.verify_live({"A2": "R-201", "A3": "R-202", "A4": "R-203", "A5": "R-204",
                           "A6": "R-205", "A7": "R-206"})

    def test_wrong_workbooks_fail(self):
        wrong = [
            [],  # the untouched template
            check.RECEIPTS[:5],
            check.RECEIPTS + [["R-207", "Extra", "Meals", 1000]],
            [r if r[0] != "R-203" else ["R-203", "Fahasa Books", "Supplies", "356000"] for r in check.RECEIPTS],
            [r if r[0] != "R-204" else ["R-204", "Viettel", "Phone", 250] for r in check.RECEIPTS],
            list(reversed(check.RECEIPTS)),
        ]
        for rows in wrong:
            with self.assertRaises(Problem, msg=rows):
                check.verify_workbook(workbook(rows))
        with self.assertRaises(Problem):
            check.verify_workbook("/nonexistent/expenses-filled.xlsx")
        with self.assertRaises(Problem):
            check.verify_live({"A2": "R-201"})


if __name__ == "__main__":
    unittest.main()
