"""Turns (label, cents) rows into (label, text) rows."""
from ledgerkit.core.money import format_cents


def render(rows):
    return [(label, format_cents(cents)) for label, cents in rows]
