"""Revenue and refunds of a quarter."""
from ledgerkit.core.dates import format_date, quarter_bounds
from ledgerkit.core.text import lines
from ledgerkit.reports.layout import money_row
from ledgerkit.services import ledger, refunds


def build(entries, year, quarter):
    start, end = quarter_bounds(year, quarter)
    return {
        "start": start,
        "end": end,
        "net_cents": ledger.total_in_period(entries, start, end),
        "refunded_cents": refunds.refunded_in_period(entries, start, end),
    }


def render(report):
    return lines([
        "Quarter %s to %s" % (format_date(report["start"]), format_date(report["end"])),
        money_row("Net revenue", report["net_cents"]),
        money_row("Refunded", report["refunded_cents"]),
    ])
