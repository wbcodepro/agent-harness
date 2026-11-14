"""Turns (date, value) rows into (text, value) rows."""
from ledgerkit.core.dates import format_date


def render(rows):
    return [(format_date(day), value) for day, value in rows]
