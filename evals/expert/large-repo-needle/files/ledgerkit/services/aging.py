"""How long invoices have been unpaid.

An invoice is a dict with "id", "customer", "issued" (ISO string),
"amount_cents" and "paid" (bool).
"""
from ledgerkit.core import config
from ledgerkit.core.dates import days_between, parse_date


def age_days(invoice, today):
    """Days since the invoice was issued: 0 on the day of issue."""
    return days_between(parse_date(invoice["issued"]), today)


def bucket(age):
    """The label of the aging bucket: "0-30", "31-60", "61-90" or "90+"."""
    low = 0
    for high in config.AGING_BUCKETS:
        if age <= high:
            return "%d-%d" % (low, high)
        low = high + 1
    return "%d+" % config.AGING_BUCKETS[-1]


def open_by_bucket(invoices, today):
    """{bucket label: total open cents}, for unpaid invoices issued on or
    before `today`."""
    totals = {}
    for invoice in invoices:
        if invoice["paid"]:
            continue
        age = age_days(invoice, today)
        if age < 0:
            continue
        label = bucket(age)
        totals[label] = totals.get(label, 0) + invoice["amount_cents"]
    return totals
