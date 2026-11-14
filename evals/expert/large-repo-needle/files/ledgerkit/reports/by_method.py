"""Revenue by payment method."""
from ledgerkit.core.grouping import sum_by
from ledgerkit.core.text import lines
from ledgerkit.reports.layout import money_row

FIELD = "method"
TITLE = "Revenue by payment method"


def build(entries):
    """[(method, total cents)] sorted by method."""
    return {"rows": sum_by(entries, FIELD)}


def render(report):
    rows = [TITLE]
    for value, cents in report["rows"]:
        rows.append(money_row(value, cents))
    return lines(rows)
