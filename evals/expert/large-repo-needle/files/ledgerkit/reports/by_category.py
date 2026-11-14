"""Revenue by product category."""
from ledgerkit.core.grouping import sum_by
from ledgerkit.core.text import lines
from ledgerkit.reports.layout import money_row

FIELD = "category"
TITLE = "Revenue by product category"


def build(entries):
    """[(category, total cents)] sorted by category."""
    return {"rows": sum_by(entries, FIELD)}


def render(report):
    rows = [TITLE]
    for value, cents in report["rows"]:
        rows.append(money_row(value, cents))
    return lines(rows)
