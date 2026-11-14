"""Leave taken by an employee in a month."""
from ledgerkit.core.dates import month_bounds
from ledgerkit.core.text import lines
from ledgerkit.reports.layout import number_row
from ledgerkit.services import leave


def build(leaves, employee, year, month):
    start, end = month_bounds(year, month)
    return {
        "employee": employee,
        "year": year,
        "month": month,
        "by_kind": leave.days_by_kind(leaves, employee, start, end),
        "days": leave.days_taken(leaves, employee, start, end),
    }


def render(report):
    rows = ["Leave of %s in %04d-%02d" % (report["employee"], report["year"], report["month"])]
    for kind, days in report["by_kind"]:
        rows.append(number_row(kind, days))
    rows.append(number_row("Total days", report["days"]))
    return lines(rows)
