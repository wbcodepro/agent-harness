"""Validation of invoice records."""
from ledgerkit.validators import common
from ledgerkit.core import ids

REQUIRED = ['id', 'customer', 'issued', 'amount_cents', 'paid']
DATE_FIELDS = ['issued']


def validate(record):
    problems = common.missing_fields(record, REQUIRED)
    problems += common.bad_dates(record, DATE_FIELDS)
    if "id" in record and not ids.is_valid('invoice', record["id"]):
        problems.append("bad id")
    return problems
