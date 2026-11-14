"""Sales tax owed per region."""
from ledgerkit.core.grouping import sum_by
from ledgerkit.core.text import lines
from ledgerkit.reports.layout import money_row
from ledgerkit.services import taxes


def build(entries):
    rows = []
    for region, total in sum_by([entry for entry in entries if entry["amount_cents"] > 0], "region"):
        rows.append((region, taxes.tax_cents(total, region)))
    return {"rows": rows}


def render(report):
    rows = ["Sales tax by region"]
    for region, cents in report["rows"]:
        rows.append(money_row(region, cents))
    return lines(rows)
