"""Validation of customer records."""
from ledgerkit.validators import common
from ledgerkit.core import ids

REQUIRED = ['id', 'name', 'region']
DATE_FIELDS = []


def validate(record):
    problems = common.missing_fields(record, REQUIRED)
    problems += common.bad_dates(record, DATE_FIELDS)
    if "id" in record and not ids.is_valid('customer', record["id"]):
        problems.append("bad id")
    return problems
