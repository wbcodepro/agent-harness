"""Validation of shipment records."""
from ledgerkit.validators import common

REQUIRED = ['id', 'sent', 'arrived', 'warehouse']
DATE_FIELDS = ['sent', 'arrived']


def validate(record):
    problems = common.missing_fields(record, REQUIRED)
    problems += common.bad_dates(record, DATE_FIELDS)
    problems += common.wrong_order(record, 'sent', 'arrived')
    return problems
