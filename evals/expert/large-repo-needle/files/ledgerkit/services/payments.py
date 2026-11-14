"""Payment methods."""
from ledgerkit.core.grouping import sum_by

FEES_PERCENT = {"card": 2, "transfer": 0, "cash": 0, "cheque": 1}


def totals_by_method(entries):
    return sum_by([entry for entry in entries if entry["amount_cents"] > 0], "method")


def fee_percent(method):
    return FEES_PERCENT.get(method, 0)
