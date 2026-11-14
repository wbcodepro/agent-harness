"""The monthly statement of a customer."""
from ledgerkit.core.dates import format_date, month_bounds
from ledgerkit.core.text import lines
from ledgerkit.reports.layout import money_row
from ledgerkit.services import ledger


def build(entries, customer_id, year, month):
    """The customer's entries of the month and their total."""
    start, end = month_bounds(year, month)
    chosen = ledger.entries_in_period(ledger.entries_of(entries, customer_id), start, end)
    return {
        "customer": customer_id,
        "start": start,
        "end": end,
        "entries": chosen,
        "total_cents": ledger.total_cents(chosen),
    }


def render(statement, customer_name=None):
    who = statement["customer"]
    if customer_name:
        who = "%s (%s)" % (who, customer_name)
    rows = ["Statement for %s, %s to %s" % (who, format_date(statement["start"]), format_date(statement["end"]))]
    for entry in statement["entries"]:
        rows.append(entry["date"] + "  " + money_row(entry["description"], entry["amount_cents"]))
    rows.append(" " * 12 + money_row("Total", statement["total_cents"]))
    return lines(rows)
