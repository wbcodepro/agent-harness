"""How full a room was in a month."""
from ledgerkit.core.dates import month_bounds
from ledgerkit.core.text import lines
from ledgerkit.reports.layout import number_row
from ledgerkit.services import occupancy


def build(bookings, room, year, month):
    start, end = month_bounds(year, month)
    return {
        "room": room,
        "year": year,
        "month": month,
        "days": occupancy.days_occupied(bookings, room, start, end),
        "percent": occupancy.occupancy_percent(bookings, room, start, end),
    }


def render(report):
    return lines([
        "Occupancy of %s in %04d-%02d" % (report["room"], report["year"], report["month"]),
        number_row("Days occupied", report["days"]),
        number_row("Percent", "%d%%" % report["percent"]),
    ])
