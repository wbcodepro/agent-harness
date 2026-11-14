"""Revenue by sales region."""
from ledgerkit.core.grouping import sum_by
from ledgerkit.core.text import lines
from ledgerkit.reports.layout import money_row

FIELD = "region"
TITLE = "Revenue by sales region"


def build(entries):
    """[(region, total cents)] sorted by region."""
    return {"rows": sum_by(entries, FIELD)}


def render(report):
    rows = [TITLE]
    for value, cents in report["rows"]:
        rows.append(money_row(value, cents))
    return lines(rows)
