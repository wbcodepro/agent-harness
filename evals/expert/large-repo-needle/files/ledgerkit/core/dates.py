"""Calendar helpers. Dates are datetime.date objects; in the data files they
are ISO strings (YYYY-MM-DD)."""
from datetime import date, timedelta

from ledgerkit.core.errors import BadInput


def parse_date(text):
    """A date from an ISO string."""
    try:
        year, month, day = text.split("-")
        return date(int(year), int(month), int(day))
    except (ValueError, AttributeError):
        raise BadInput("not a date: %r" % (text,))


def format_date(day):
    return day.isoformat()


def parse_month(text):
    """(year, month) from "YYYY-MM"."""
    try:
        year, month = text.split("-")
        year, month = int(year), int(month)
    except (ValueError, AttributeError):
        raise BadInput("not a month: %r" % (text,))
    if not 1 <= month <= 12:
        raise BadInput("not a month: %r" % (text,))
    return year, month


def month_bounds(year, month):
    """The first and the last day of a month."""
    first = date(year, month, 1)
    if month == 12:
        following = date(year + 1, 1, 1)
    else:
        following = date(year, month + 1, 1)
    return first, following - timedelta(days=1)


def quarter_bounds(year, quarter):
    """The first and the last day of a quarter (1 to 4)."""
    first, _ = month_bounds(year, 3 * quarter - 2)
    _, last = month_bounds(year, 3 * quarter)
    return first, last


def add_days(day, count):
    return day + timedelta(days=count)


def days_between(earlier, later):
    """How many days `later` is after `earlier`: 0 for the same day, 1 for
    the next day. Negative when `later` is in fact earlier."""
    return later.toordinal() - earlier.toordinal()
