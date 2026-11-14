import unittest

from ledgerkit.core import money


class MoneyTest(unittest.TestCase):
    def test_format(self):
        self.assertEqual(money.format_cents(1250), "12.50")
        self.assertEqual(money.format_cents(-5), "-0.05")

    def test_parse(self):
        self.assertEqual(money.parse_cents("12.5"), 1250)
        self.assertEqual(money.parse_cents("29"), 2900)

    def test_rounding(self):
        self.assertEqual(money.div_round(5, 2), 3)
        self.assertEqual(money.div_round(7, 3), 2)
        self.assertEqual(money.percent_of(1050, 5), 53)


if __name__ == "__main__":
    unittest.main()
