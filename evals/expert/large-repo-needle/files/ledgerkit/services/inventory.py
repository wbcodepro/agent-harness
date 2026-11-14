"""Stock levels derived from the ledger: every sale of a product takes one
unit out of its warehouse."""
from ledgerkit.core.grouping import count_by


def units_sold(entries):
    """[(product, units)] sorted by product; refunds are not counted."""
    return count_by([entry for entry in entries if entry["amount_cents"] > 0], "product")


def units_by_warehouse(entries):
    return count_by([entry for entry in entries if entry["amount_cents"] > 0], "warehouse")
