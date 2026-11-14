"""What a customer owes for part of a billing period."""
from ledgerkit.core.dates import format_date
from ledgerkit.core.text import lines
from ledgerkit.reports.layout import money_row, number_row
from ledgerkit.services import proration


def build(amount_cents, period_start, period_end, used_start, used_end):
    used, total = proration.used_days(period_start, period_end, used_start, used_end)
    return {
        "period": (period_start, period_end),
        "used_days": used,
        "period_days": total,
        "price_cents": amount_cents,
        "owed_cents": proration.prorated_cents(amount_cents, period_start, period_end, used_start, used_end),
    }


def render(report):
    start, end = report["period"]
    return lines([
        "Proration for %s to %s" % (format_date(start), format_date(end)),
        number_row("Days used", report["used_days"]),
        number_row("Days in period", report["period_days"]),
        money_row("Full price", report["price_cents"]),
        money_row("Owed", report["owed_cents"]),
    ])
