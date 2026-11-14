"""Adds a total row to (label, cents) rows."""


def with_total(rows, label="Total"):
    return list(rows) + [(label, sum(cents for _, cents in rows))]
