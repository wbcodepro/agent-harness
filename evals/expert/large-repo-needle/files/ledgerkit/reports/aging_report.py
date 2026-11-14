"""Open invoices by age."""
from ledgerkit.core import config
from ledgerkit.core.dates import format_date
from ledgerkit.core.text import lines
from ledgerkit.reports.layout import money_row
from ledgerkit.services import aging


def labels():
    result, low = [], 0
    for high in config.AGING_BUCKETS:
        result.append("%d-%d" % (low, high))
        low = high + 1
    result.append("%d+" % config.AGING_BUCKETS[-1])
    return result


def build(invoices, today):
    totals = aging.open_by_bucket(invoices, today)
    return {"today": today, "buckets": [(label, totals.get(label, 0)) for label in labels()]}


def render(report):
    rows = ["Open invoices on %s" % format_date(report["today"])]
    for label, cents in report["buckets"]:
        rows.append(money_row(label + " days", cents))
    return lines(rows)
