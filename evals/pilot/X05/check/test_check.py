"""X05 checker, offline: reports written before and after the correction."""
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import check  # noqa: E402
from office import Problem  # noqa: E402

DATA = os.path.join(HERE, "..", "files", "data")


def report(lines):
    path = os.path.join(tempfile.mkdtemp(), "report.md")
    with open(path, "w") as f:
        f.write("# Q3 sales by region\n\n" + "\n".join(lines) + "\n")
    return path


class X05(unittest.TestCase):
    def test_the_corrected_report_passes(self):
        want = check.expected(DATA)
        self.assertEqual([r for r, _ in want], ["East", "North", "South", "West"])
        lines = [f"{r}: {v:,.2f} EUR" for r, v in want] + [f"Grand total: {sum(v for _, v in want):,.2f} EUR"]
        self.assertIn("as corrected", check.verify_report(report(lines), DATA))
        check.verify_report(report([f"- **{ln}**" for ln in lines]), DATA)
        check.verify_delivered(["Write report.md", check.CORRECTION_MARK + " ..."])

    def test_the_original_instruction_and_partial_corrections_fail(self):
        want = check.expected(DATA)
        usd = report(["East: 4,119.95 USD", "Internal: 14,900.00 USD"])
        with_internal = report([f"{r}: {v:,.2f} EUR" for r, v in want] + ["Internal: 13,708.00 EUR"])
        unsorted = report([f"{r}: {v:,.2f} EUR" for r, v in reversed(want)]
                          + [f"Grand total: {sum(v for _, v in want):,.2f} EUR"])
        no_total = report([f"{r}: {v:,.2f} EUR" for r, v in want])
        for path in (usd, with_internal, unsorted, no_total):
            with self.assertRaises(Problem, msg=path):
                check.verify_report(path, DATA)
        with self.assertRaises(Problem):
            check.verify_delivered(["Write report.md"])


if __name__ == "__main__":
    unittest.main()
