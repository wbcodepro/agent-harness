"""Revenue by product."""
from ledgerkit.core.grouping import sum_by
from ledgerkit.core.text import lines
from ledgerkit.reports.layout import money_row

FIELD = "product"
TITLE = "Revenue by product"


def build(entries):
    """[(product, total cents)] sorted by product."""
    return {"rows": sum_by(entries, FIELD)}


def render(report):
    rows = [TITLE]
    for value, cents in report["rows"]:
        rows.append(money_row(value, cents))
    return lines(rows)
