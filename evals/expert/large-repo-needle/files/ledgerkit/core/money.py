"""Money is kept as an integer number of cents."""

from ledgerkit.core.errors import BadInput


def format_cents(cents):
    """"12.50" for 1250, "-0.05" for -5."""
    sign = "-" if cents < 0 else ""
    cents = abs(cents)
    return "%s%d.%02d" % (sign, cents // 100, cents % 100)


def parse_cents(text):
    """1250 for "12.50" or "12.5" or "12"."""
    try:
        whole, _, fraction = text.strip().partition(".")
        fraction = (fraction + "00")[:2]
        negative = whole.startswith("-")
        value = abs(int(whole or "0")) * 100 + int(fraction)
    except ValueError:
        raise BadInput("not an amount: %r" % (text,))
    return -value if negative else value


def div_round(numerator, denominator):
    """numerator / denominator rounded to the nearest integer, halves up.
    For a non-negative numerator and a positive denominator."""
    return (2 * numerator + denominator) // (2 * denominator)


def percent_of(cents, percent):
    """`percent` percent of an amount, rounded half up to the cent."""
    return div_round(cents * percent, 100)
