def total(items, tax):
    """Sum of price * quantity, with tax added."""
    subtotal = sum(item["price"] * item["qty"] for item in items)
    return round(subtotal * (1 + tax), 2)
