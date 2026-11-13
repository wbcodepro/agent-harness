from shop.pricing import total as order_total


def daily(orders, tax):
    """orders: list of item lists. Returns (count, revenue, largest)."""
    amounts = [order_total(items, tax) for items in orders]
    if not amounts:
        return 0, 0.0, 0.0
    return len(amounts), round(sum(amounts), 2), max(amounts)
