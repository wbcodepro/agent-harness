"""Revenue by salesperson."""
from ledgerkit.core.grouping import sum_by
from ledgerkit.core.text import lines
from ledgerkit.reports.layout import money_row

FIELD = "salesperson"
TITLE = "Revenue by salesperson"


def build(entries):
    """[(salesperson, total cents)] sorted by salesperson."""
    return {"rows": sum_by(entries, FIELD)}


def render(report):
    rows = [TITLE]
    for value, cents in report["rows"]:
        rows.append(money_row(value, cents))
    return lines(rows)
