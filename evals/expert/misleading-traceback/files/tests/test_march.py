import os
import subprocess
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def run(*args):
    done = subprocess.run(
        [sys.executable, "-m", "salesreport"] + list(args),
        cwd=ROOT, capture_output=True, text=True,
    )
    return done.returncode, done.stdout, done.stderr


class March(unittest.TestCase):
    def test_top_products(self):
        code, out, err = run("top", "data/march.txt")
        self.assertEqual(code, 0, err)
        self.assertEqual(out.splitlines(), [
            "Top 3 products by units",
            " 1. widget              13",
            " 2. gadget               9",
            " 3. bracket              7",
        ])


if __name__ == "__main__":
    unittest.main()
