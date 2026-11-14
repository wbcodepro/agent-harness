"""Charging for part of a billing period."""
from ledgerkit.core import periods
from ledgerkit.core.money import div_round


def used_days(period_start, period_end, used_start, used_end):
    """(days used, days in the billing period). Only the days of the usage
    that fall inside the billing period count."""
    used = periods.overlap_days(period_start, period_end, used_start, used_end)
    return used, periods.span(period_start, period_end)


def prorated_cents(amount_cents, period_start, period_end, used_start, used_end):
    """The share of the period's price owed for the days used, rounded half
    up to the cent."""
    used, total = used_days(period_start, period_end, used_start, used_end)
    return div_round(amount_cents * used, total)
