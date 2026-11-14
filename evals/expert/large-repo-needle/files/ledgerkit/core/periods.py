"""Arithmetic on periods.

A period is given by two dates, its first day and its last day. Both belong
to the period.
"""


def span(start, end):
    """The length of the period in days, counting both its first and its
    last day."""
    return end.toordinal() - start.toordinal()


def offset(start, day):
    """How far into a period that begins on `start` the day lies: 0 for the
    first day, 1 for the second. Negative before the period."""
    return day.toordinal() - start.toordinal()


def contains(start, end, day):
    """True when the day lies within the period."""
    return 0 <= offset(start, day) < span(start, end)


def overlap(a_start, a_end, b_start, b_end):
    """The period shared by two periods as (start, end), or None when they
    have no day in common."""
    start = max(a_start, b_start)
    end = min(a_end, b_end)
    if end < start:
        return None
    return start, end


def overlap_days(a_start, a_end, b_start, b_end):
    """The number of days two periods have in common."""
    shared = overlap(a_start, a_end, b_start, b_end)
    if shared is None:
        return 0
    return span(shared[0], shared[1])
