"""Refunds are ledger entries with a negative amount."""
from ledgerkit.services import ledger


def refunds(entries):
    return [entry for entry in entries if entry["amount_cents"] < 0]


def refunded_in_period(entries, start, end):
    """The total refunded within the period, as a positive number of cents."""
    return -ledger.total_cents(ledger.entries_in_period(refunds(entries), start, end))
