import unittest

from ledgerkit.validators import booking, customer


class ValidatorsTest(unittest.TestCase):
    def test_customer(self):
        self.assertEqual(customer.validate({"id": "C-101", "name": "A", "region": "north"}), [])
        self.assertEqual(customer.validate({"id": "X", "name": "A", "region": "north"}), ["bad id"])

    def test_booking(self):
        good = {"room": "R01", "from": "2024-03-01", "to": "2024-03-01", "guest": "Ito"}
        self.assertEqual(booking.validate(good), [])
        bad = dict(good, to="2024-02-28")
        self.assertEqual(booking.validate(bad), ["to is before from"])


if __name__ == "__main__":
    unittest.main()
