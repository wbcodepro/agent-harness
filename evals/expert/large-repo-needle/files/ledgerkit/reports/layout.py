"""The two-column layout shared by the text reports."""
from ledgerkit.core import config
from ledgerkit.core.money import format_cents
from ledgerkit.core.text import pad, right


def money_row(label, cents):
    return pad(label, config.DESCRIPTION_WIDTH) + right(format_cents(cents), config.AMOUNT_WIDTH)


def number_row(label, number):
    return pad(label, config.DESCRIPTION_WIDTH) + right(number, config.AMOUNT_WIDTH)
