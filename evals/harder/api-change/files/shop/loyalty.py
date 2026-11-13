from shop import pricing


def points(items, tax):
    """One point per whole unit of currency spent."""
    return int(pricing.total(items, tax))


def qualifies_for_gift(items, tax, threshold=100):
    return pricing.total(items, tax) >= threshold
