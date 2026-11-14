"""Checks shared by the validators."""
from ledgerkit.core.dates import parse_date
from ledgerkit.core.errors import BadInput


def missing_fields(record, required):
    return ["missing field %s" % name for name in required if name not in record]


def bad_dates(record, date_fields):
    problems = []
    for name in date_fields:
        if name in record:
            try:
                parse_date(record[name])
            except BadInput:
                problems.append("field %s is not a date" % name)
    return problems


def wrong_order(record, first_field, last_field):
    """A period must not end before it begins. One day is a valid period."""
    try:
        first, last = parse_date(record[first_field]), parse_date(record[last_field])
    except (BadInput, KeyError):
        return []
    if last < first:
        return ["%s is before %s" % (last_field, first_field)]
    return []
