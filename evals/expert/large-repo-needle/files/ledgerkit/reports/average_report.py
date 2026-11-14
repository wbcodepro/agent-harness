"""A customer's average spending per day in a month."""
from ledgerkit.core.dates import month_bounds
from ledgerkit.core.text import lines
from ledgerkit.reports.layout import money_row
from ledgerkit.services import averages, ledger


def build(entries, customer_id, year, month):
    start, end = month_bounds(year, month)
    own = ledger.entries_of(entries, customer_id)
    return {
        "customer": customer_id,
        "year": year,
        "month": month,
        "total_cents": ledger.total_in_period(own, start, end),
        "per_day_cents": averages.daily_average_cents(own, start, end),
    }


def render(report):
    return lines([
        "Daily average of %s in %04d-%02d" % (report["customer"], report["year"], report["month"]),
        money_row("Total", report["total_cents"]),
        money_row("Per day", report["per_day_cents"]),
    ])
