import unittest
from datetime import date

from ledgerkit.services import occupancy

BOOKINGS = [
    {"room": "R01", "from": "2024-03-01", "to": "2024-03-03", "guest": "Ito"},
    {"room": "R01", "from": "2024-03-10", "to": "2024-03-10", "guest": "Brandt"},
    {"room": "R01", "from": "2024-03-30", "to": "2024-04-02", "guest": "Okafor"},
    {"room": "R02", "from": "2024-03-05", "to": "2024-03-06", "guest": "Lund"},
]


class OccupancyTest(unittest.TestCase):
    def test_days_occupied(self):
        march = (date(2024, 3, 1), date(2024, 3, 31))
        self.assertEqual(occupancy.days_occupied(BOOKINGS, "R01", *march), 6)
        self.assertEqual(occupancy.days_occupied(BOOKINGS, "R02", *march), 2)
        self.assertEqual(occupancy.days_occupied(BOOKINGS, "R03", *march), 0)

    def test_percent(self):
        march = (date(2024, 3, 1), date(2024, 3, 31))
        self.assertEqual(occupancy.occupancy_percent(BOOKINGS, "R01", *march), 19)
        ten_days = (date(2024, 3, 1), date(2024, 3, 10))
        self.assertEqual(occupancy.occupancy_percent(BOOKINGS, "R01", *ten_days), 40)


if __name__ == "__main__":
    unittest.main()
