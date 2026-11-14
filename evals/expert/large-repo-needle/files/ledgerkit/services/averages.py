"""Averages over a period."""
from ledgerkit.core import periods
from ledgerkit.core.money import div_round
from ledgerkit.services import ledger


def per_day_cents(total_cents, start, end):
    """A total spread evenly over the days of the period, rounded half up
    to the cent."""
    return div_round(total_cents, periods.span(start, end))


def daily_average_cents(entries, start, end):
    """The average amount per day of the entries dated within the period."""
    return per_day_cents(ledger.total_in_period(entries, start, end), start, end)
