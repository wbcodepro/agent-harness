"""Validation of leave records."""
from ledgerkit.validators import common

REQUIRED = ['employee', 'from', 'to', 'kind']
DATE_FIELDS = ['from', 'to']


def validate(record):
    problems = common.missing_fields(record, REQUIRED)
    problems += common.bad_dates(record, DATE_FIELDS)
    problems += common.wrong_order(record, 'from', 'to')
    return problems
