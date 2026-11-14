"""Subscriptions in service on a day."""
from ledgerkit.core.dates import format_date
from ledgerkit.core.text import lines
from ledgerkit.reports.layout import money_row
from ledgerkit.services import subscriptions


def build(records, day):
    active = subscriptions.active_on(records, day)
    return {
        "day": day,
        "active": active,
        "monthly_cents": subscriptions.monthly_recurring_cents(records, day),
    }


def render(report):
    rows = ["Subscriptions on %s" % format_date(report["day"])]
    for record in report["active"]:
        rows.append(money_row("%s %s" % (record["customer"], record["plan"]), record["monthly_cents"]))
    rows.append(money_row("Monthly recurring", report["monthly_cents"]))
    return lines(rows)
