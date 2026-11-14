"""Grouping records by a field."""


def group_by(records, field):
    """{value: [records]} keeping the order of the records."""
    groups = {}
    for record in records:
        groups.setdefault(record.get(field), []).append(record)
    return groups


def sum_by(records, field, amount_field="amount_cents"):
    """[(value, total)] sorted by value."""
    totals = {}
    for record in records:
        key = record.get(field)
        totals[key] = totals.get(key, 0) + record.get(amount_field, 0)
    return sorted(totals.items(), key=lambda item: str(item[0]))


def count_by(records, field):
    """[(value, count)] sorted by value."""
    counts = {}
    for record in records:
        key = record.get(field)
        counts[key] = counts.get(key, 0) + 1
    return sorted(counts.items(), key=lambda item: str(item[0]))
