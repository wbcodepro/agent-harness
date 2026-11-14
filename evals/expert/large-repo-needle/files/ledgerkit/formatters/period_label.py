"""A short label for a period."""
from ledgerkit.core.dates import format_date


def render(start, end):
    """"2024-02-01..2024-02-29"; a single day is shown once."""
    if start == end:
        return format_date(start)
    return "%s..%s" % (format_date(start), format_date(end))
