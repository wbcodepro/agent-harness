import unittest

from shop import invoice, loyalty, pricing, report

ITEMS = [{"price": 10.0, "qty": 3}, {"price": 2.5, "qty": 4}]


class ShopTest(unittest.TestCase):
    def test_total_returns_money(self):
        money = pricing.total(ITEMS, 0.1)
        self.assertEqual(money, pricing.Money(44.0, "USD"))
        self.assertEqual(money.amount, 44.0)
        self.assertEqual(pricing.total(ITEMS, 0.1, currency="EUR").currency, "EUR")
        with self.assertRaises(TypeError):
            pricing.total(ITEMS, 0.1, "EUR")

    def test_invoice(self):
        self.assertEqual(invoice.render(ITEMS, 0.1), "Total due: 44.00")
        self.assertEqual(invoice.split(ITEMS, 0.1, 4), 11.0)

    def test_loyalty(self):
        self.assertEqual(loyalty.points(ITEMS, 0.1), 44)
        self.assertTrue(loyalty.qualifies_for_gift(ITEMS, 0.1, threshold=44))
        self.assertFalse(loyalty.qualifies_for_gift(ITEMS, 0.1))

    def test_report(self):
        self.assertEqual(report.daily([ITEMS, ITEMS[:1]], 0.1), (2, 77.0, 44.0))
        self.assertEqual(report.daily([], 0.1), (0, 0.0, 0.0))


if __name__ == "__main__":
    unittest.main()
