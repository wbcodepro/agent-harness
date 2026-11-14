"""Time-limited discounts.

A discount is a dict with "code", "percent", "from" and "to" (ISO strings,
first and last day on which the code works).
"""
from ledgerkit.core import periods
from ledgerkit.core.dates import parse_date
from ledgerkit.core.money import percent_of


def valid_codes(discounts, day):
    """The codes that work on that day, sorted."""
    return sorted(discount["code"] for discount in discounts
                  if periods.contains(parse_date(discount["from"]), parse_date(discount["to"]), day))


def discounted_cents(amount_cents, percent):
    """The amount after taking `percent` percent off."""
    return amount_cents - percent_of(amount_cents, percent)
