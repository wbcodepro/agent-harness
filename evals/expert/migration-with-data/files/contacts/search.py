"""Looking things up in a list of records."""


def find_by_email(records, address):
    """Return the records whose mail address is `address`, ignoring case."""
    wanted = address.lower()
    return [record for record in records if record["mail"].lower() == wanted]


def label(record):
    """Return the text shown for a record in a list."""
    return record["name"]


def by_name(records):
    """Return the records sorted by name, ignoring case, then by id."""
    return sorted(records, key=lambda record: (record["name"].lower(), record["id"]))
