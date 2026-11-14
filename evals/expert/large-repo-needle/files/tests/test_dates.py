import unittest
from datetime import date

from ledgerkit.core import dates


class DatesTest(unittest.TestCase):
    def test_month_bounds(self):
        self.assertEqual(dates.month_bounds(2023, 2), (date(2023, 2, 1), date(2023, 2, 28)))
        self.assertEqual(dates.month_bounds(2024, 12), (date(2024, 12, 1), date(2024, 12, 31)))

    def test_quarter_bounds(self):
        self.assertEqual(dates.quarter_bounds(2024, 2), (date(2024, 4, 1), date(2024, 6, 30)))

    def test_days_between(self):
        self.assertEqual(dates.days_between(date(2024, 1, 1), date(2024, 1, 1)), 0)
        self.assertEqual(dates.days_between(date(2024, 1, 1), date(2024, 1, 31)), 30)


if __name__ == "__main__":
    unittest.main()
