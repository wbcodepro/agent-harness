"""Revenue by warehouse."""
from ledgerkit.core.grouping import sum_by
from ledgerkit.core.text import lines
from ledgerkit.reports.layout import money_row

FIELD = "warehouse"
TITLE = "Revenue by warehouse"


def build(entries):
    """[(warehouse, total cents)] sorted by warehouse."""
    return {"rows": sum_by(entries, FIELD)}


def render(report):
    rows = [TITLE]
    for value, cents in report["rows"]:
        rows.append(money_row(value, cents))
    return lines(rows)
