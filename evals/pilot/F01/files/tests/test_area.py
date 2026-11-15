import unittest

from geometry import calc_area


class AreaTest(unittest.TestCase):
    def test_rect(self):
        self.assertEqual(calc_area({"kind": "rect", "w": 2, "h": 3}), 6)

    def test_unknown(self):
        with self.assertRaises(ValueError):
            calc_area({"kind": "hexagon"})


if __name__ == "__main__":
    unittest.main()
