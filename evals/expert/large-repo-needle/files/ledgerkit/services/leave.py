"""Employee leave.

A leave record is a dict with "employee", "from" and "to" (ISO strings,
first and last day off) and "kind".
"""
from ledgerkit.core import periods
from ledgerkit.core.dates import parse_date


def days_taken(leaves, employee, start, end):
    """How many days of the period the employee was on leave."""
    total = 0
    for leave in leaves:
        if leave["employee"] != employee:
            continue
        total += periods.overlap_days(parse_date(leave["from"]), parse_date(leave["to"]), start, end)
    return total


def days_by_kind(leaves, employee, start, end):
    """[(kind, days)] sorted by kind, leaving out kinds with no days."""
    totals = {}
    for leave in leaves:
        if leave["employee"] != employee:
            continue
        days = periods.overlap_days(parse_date(leave["from"]), parse_date(leave["to"]), start, end)
        if days:
            totals[leave["kind"]] = totals.get(leave["kind"], 0) + days
    return sorted(totals.items())
