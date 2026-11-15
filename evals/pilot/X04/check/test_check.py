"""X04 checker, offline: discrepancy files, right and wrong."""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check  # noqa: E402
from office import Problem  # noqa: E402

RIGHT = "id,site_price,sheet_price\n13,42.00,45.00\n37,10.00,12.00\n61,28.00,\n85,,15.00\n"


def write(text):
    path = os.path.join(tempfile.mkdtemp(), "discrepancies.csv")
    with open(path, "w") as f:
        f.write(text)
    return path


class X04(unittest.TestCase):
    def test_the_discrepancy_set_passes(self):
        self.assertEqual(sorted(check.site_items()), [1, 13, 25, 37, 49, 61])
        self.assertIn("4 discrepancies", check.verify_csv(write(RIGHT)))
        check.verify_sheet_unchanged({"A2": "1", "C2": "8", "A3": "13", "C3": "45", "A4": "25", "C4": "26",
                                      "A5": "37", "C5": "12", "A6": "49", "C6": "44", "A7": "85", "C7": "15"})

    def test_wrong_sets_fail(self):
        lines = RIGHT.splitlines()
        cases = [
            "\n".join(lines[:3]) + "\n",                       # a one-sided id missing
            "\n".join([lines[0]] + lines[1:] + ["1,8.00,8.00"]) + "\n",  # an equal pair listed
            "\n".join([lines[0], lines[2], lines[1]] + lines[3:]) + "\n",  # not sorted
            RIGHT.replace("42.00", "42"),
            RIGHT.replace("61,28.00,", "61,28.00,0.00"),
            RIGHT.replace("id,site_price,sheet_price", "id,site,sheet"),
        ]
        for text in cases:
            with self.assertRaises(Problem, msg=text):
                check.verify_csv(write(text))
        with self.assertRaises(Problem):
            check.verify_csv("/nonexistent.csv")
        with self.assertRaises(Problem):
            check.verify_sheet_unchanged({"A2": "1", "C2": "9"})


if __name__ == "__main__":
    unittest.main()
