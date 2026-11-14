import os
import subprocess
import sys
import unittest

ROOT = os.path.dirname(os.path.abspath(__file__))


def run(*args):
    done = subprocess.run(
        [sys.executable, "-m", "salesreport"] + list(args),
        cwd=ROOT, capture_output=True, text=True, timeout=10,
    )
    if done.returncode != 0:
        raise AssertionError("salesreport %s exited with %d:\n%s"
                             % (" ".join(args), done.returncode, done.stderr))
    return done.stdout.splitlines()


class March(unittest.TestCase):
    # checkdata/march.txt is the file the task shipped as data/march.txt.
    def test_top(self):
        self.assertEqual(run("top", "checkdata/march.txt"), [
            "Top 3 products by units",
            " 1. widget              13",
            " 2. gadget               9",
            " 3. bracket              7",
        ])

    def test_top_six_reaches_the_small_orders(self):
        self.assertEqual(run("top", "checkdata/march.txt", "6"), [
            "Top 6 products by units",
            " 1. widget              13",
            " 2. gadget               9",
            " 3. bracket              7",
            " 4. usb hub              2",
            " 5. cable                1",
            " 6. manual               1",
        ])

    def test_stats(self):
        self.assertEqual(run("stats", "checkdata/march.txt"), [
            "orders: 6",
            "customers: 4",
            "item lines: 11",
            "lines per order: 1.83",
            "largest order: 1003 (3 lines)",
        ])


class April(unittest.TestCase):
    def test_top(self):
        self.assertEqual(run("top", "checkdata/april.txt"), [
            "Top 3 products by units",
            " 1. anvil                8",
            " 2. chalk                8",
            " 3. rope                 8",
        ])

    def test_top_five(self):
        self.assertEqual(run("top", "checkdata/april.txt", "5"), [
            "Top 5 products by units",
            " 1. anvil                8",
            " 2. chalk                8",
            " 3. rope                 8",
            " 4. tv                   8",
            " 5. usb hub              8",
        ])

    def test_stats(self):
        self.assertEqual(run("stats", "checkdata/april.txt"), [
            "orders: 5",
            "customers: 3",
            "item lines: 9",
            "lines per order: 1.80",
            "largest order: 2002 (3 lines)",
        ])


class OnlySingleItemOrders(unittest.TestCase):
    def test_top(self):
        self.assertEqual(run("top", "checkdata/may.txt"), [
            "Top 3 products by units",
            " 1. pen                  6",
            " 2. ink                  5",
        ])

    def test_stats(self):
        self.assertEqual(run("stats", "checkdata/may.txt"), [
            "orders: 3",
            "customers: 2",
            "item lines: 3",
            "lines per order: 1.00",
            "largest order: 3001 (1 lines)",
        ])

    def test_one_order_one_item(self):
        self.assertEqual(run("top", "checkdata/june.txt", "1"), [
            "Top 1 products by units",
            " 1. left handed screwdriver    12",
        ])
        self.assertEqual(run("stats", "checkdata/june.txt"), [
            "orders: 1",
            "customers: 1",
            "item lines: 1",
            "lines per order: 1.00",
            "largest order: 4001 (1 lines)",
        ])


if __name__ == "__main__":
    unittest.main()
