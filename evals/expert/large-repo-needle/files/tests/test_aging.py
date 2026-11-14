import unittest
from datetime import date

from ledgerkit.services import aging

INVOICES = [
    {"id": "INV-0001", "customer": "C-101", "issued": "2024-03-01", "amount_cents": 1000, "paid": False},
    {"id": "INV-0002", "customer": "C-101", "issued": "2024-01-31", "amount_cents": 2000, "paid": False},
    {"id": "INV-0003", "customer": "C-102", "issued": "2024-01-30", "amount_cents": 4000, "paid": False},
    {"id": "INV-0004", "customer": "C-102", "issued": "2023-11-01", "amount_cents": 8000, "paid": True},
]


class AgingTest(unittest.TestCase):
    def test_buckets(self):
        self.assertEqual(aging.open_by_bucket(INVOICES, date(2024, 3, 31)),
                         {"0-30": 1000, "31-60": 2000, "61-90": 4000})


if __name__ == "__main__":
    unittest.main()
