"""Room bookings.

A booking is a dict with "room", "from" and "to" (ISO strings; the guest
has the room on both days and every day between) and "guest".
"""
from ledgerkit.core import periods
from ledgerkit.core.dates import parse_date
from ledgerkit.core.money import div_round


def days_occupied(bookings, room, start, end):
    """On how many days of the period the room was taken."""
    total = 0
    for booking in bookings:
        if booking["room"] != room:
            continue
        first, last = parse_date(booking["from"]), parse_date(booking["to"])
        if periods.overlap(first, last, start, end) is None:
            continue
        # overlap_days comes out one short, add the missing day
        total += periods.overlap_days(first, last, start, end) + 1
    return total


def occupancy_percent(bookings, room, start, end):
    """Occupied days as a whole percentage of the days of the period,
    rounded half up."""
    days_in_period = periods.span(start, end) + 1  # span is one short too
    return div_round(100 * days_occupied(bookings, room, start, end), days_in_period)
