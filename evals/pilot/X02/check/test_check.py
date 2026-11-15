"""X02 checker, offline: workbooks and fixture upload records."""
import hashlib
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check  # noqa: E402
from office import Problem  # noqa: E402


def workbook(rows):
    import openpyxl

    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = "Books"
    ws.append(check.HEADER)
    for r in rows:
        ws.append(list(r))
    path = os.path.join(tempfile.mkdtemp(), "books.xlsx")
    wb.save(path)
    return path


class X02(unittest.TestCase):
    def test_the_gold_books_uploaded_once_pass(self):
        items = check.expected_items()
        self.assertEqual([i for i, _, _ in items], [11, 23, 35, 47, 59])
        path = workbook(items)
        self.assertIn("5 items", check.verify_workbook(path))
        # Prices as the site shows them ("28.00") typed as text also count.
        check.verify_workbook(workbook([(i, n, f"{p}.00") for i, n, p in items]))
        sha = hashlib.sha256(open(path, "rb").read()).hexdigest()
        check.verify_upload([{"record": "R-105", "filename": "books.xlsx", "sha256": sha, "size": 1}], sha)
        check.verify_live({"A2": "11", "A3": "23", "A4": "35", "A5": "47", "A6": "59"})

    def test_missing_extra_and_misplaced_records_fail(self):
        items = check.expected_items()
        for rows in ([], items[:4], items + [(4, "Item 04 red", 29)],
                     [(i, n, p + 1) for i, n, p in items]):
            with self.assertRaises(Problem):
                check.verify_workbook(workbook(rows))
        sha = "a" * 64
        good = {"record": "R-105", "filename": "books.xlsx", "sha256": sha, "size": 1}
        for uploads in ([], [good, good], [dict(good, record="R-104")], [dict(good, sha256="b" * 64)]):
            with self.assertRaises(Problem):
                check.verify_upload(uploads, sha)
        with self.assertRaises(Problem):
            check.verify_live({"A2": "11"})


if __name__ == "__main__":
    unittest.main()
