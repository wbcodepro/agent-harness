"""The leave record."""

DEFAULTS = {'kind': 'holiday'}


def normalise(record):
    """A copy of the record with the optional fields filled in."""
    result = dict(DEFAULTS)
    result.update(record)
    return result


def normalise_all(records):
    return [normalise(record) for record in records]
