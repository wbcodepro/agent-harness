"""The entry record."""

DEFAULTS = {'description': '', 'amount_cents': 0, 'channel': 'direct', 'method': 'transfer', 'currency': 'USD'}


def normalise(record):
    """A copy of the record with the optional fields filled in."""
    result = dict(DEFAULTS)
    result.update(record)
    return result


def normalise_all(records):
    return [normalise(record) for record in records]
