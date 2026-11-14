"""Validation of entry records."""
from ledgerkit.validators import common

REQUIRED = ['customer', 'date', 'description', 'amount_cents']
DATE_FIELDS = ['date']


def validate(record):
    problems = common.missing_fields(record, REQUIRED)
    problems += common.bad_dates(record, DATE_FIELDS)
    return problems
