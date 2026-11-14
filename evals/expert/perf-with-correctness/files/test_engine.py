import unittest

from engine import match


class MatchTest(unittest.TestCase):
    def test_simple_cross(self):
        trades, book = match([("sell", "s1", 100, 5), ("buy", "b1", 101, 3)])
        self.assertEqual(trades, [("b1", "s1", 100, 3)])
        self.assertEqual(book, {"bids": [], "asks": [("s1", 100, 2)]})

    def test_time_priority(self):
        trades, _ = match([
            ("sell", "zz", 100, 1), ("sell", "aa", 100, 1), ("buy", "b", 100, 2),
        ])
        self.assertEqual(trades, [("b", "zz", 100, 1), ("b", "aa", 100, 1)])

    def test_cancel(self):
        trades, book = match([("buy", "b1", 50, 4), ("cancel", "b1"), ("sell", "s1", 50, 4)])
        self.assertEqual(trades, [])
        self.assertEqual(book, {"bids": [], "asks": [("s1", 50, 4)]})


if __name__ == "__main__":
    unittest.main()
