"""A few of the examples of SPEC.md. Run: python3 test_bizcal.py"""
import unittest

import bizcal


class Examples(unittest.TestCase):
    def test_observed(self):
        names = dict((name, day) for day, name in bizcal.holidays("NORD", 2021))
        self.assertEqual(names["Christmas Day"], "2021-12-27")
        self.assertEqual(names["Boxing Day"], "2021-12-28")

    def test_bridge(self):
        self.assertFalse(bizcal.is_business_day("NORD", "2018-12-24"))
        self.assertTrue(bizcal.is_business_day("NORD", "2021-05-14"))

    def test_clock(self):
        self.assertEqual(bizcal.utc_offset("NORD", "2021-03-28T01:00"), 120)
        self.assertEqual(bizcal.to_utc("NORD", "2021-10-31T02:30", prefer="later"),
                         "2021-10-31T01:30")
        with self.assertRaises(ValueError):
            bizcal.to_utc("NORD", "2021-03-28T02:30")


if __name__ == "__main__":
    unittest.main()
