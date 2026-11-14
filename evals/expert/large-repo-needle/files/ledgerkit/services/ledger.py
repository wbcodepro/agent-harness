"""The ledger: dated entries per customer.

An entry is a dict with at least "customer", "date" (ISO string),
"description" and "amount_cents".
"""
from ledgerkit.core import periods
from ledgerkit.core.dates import parse_date


def entries_of(entries, customer_id):
    """The customer's entries in their original order."""
    return [entry for entry in entries if entry["customer"] == customer_id]


def entries_in_period(entries, start, end):
    """The entries dated within the period, oldest first. Entries of the
    same day keep their original order."""
    chosen = [entry for entry in entries
              if periods.contains(start, end, parse_date(entry["date"]))]
    return sorted(chosen, key=lambda entry: entry["date"])


def total_cents(entries):
    return sum(entry["amount_cents"] for entry in entries)


def total_in_period(entries, start, end):
    return total_cents(entries_in_period(entries, start, end))
