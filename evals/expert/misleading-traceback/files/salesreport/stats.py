"""The `stats` report: the shape of the orders themselves."""


def order_stats(orders):
    """Return a dict of figures about `orders`.

        orders     number of orders
        customers  number of distinct customers
        lines      number of item lines over all orders
        largest    (order number, lines) of the order with the most item
                   lines; the earliest such order in the file on a tie
    """
    customers = set()
    lines = 0
    largest = None
    for order in orders:
        customers.add(order["customer"])
        size = len(order["item"])
        lines += size
        if largest is None or size > largest[1]:
            largest = (order["order"], size)
    return {
        "orders": len(orders),
        "customers": len(customers),
        "lines": lines,
        "largest": largest,
    }


def render_stats(orders):
    """Return the text of the `stats` report."""
    figures = order_stats(orders)
    out = [
        "orders: %d" % figures["orders"],
        "customers: %d" % figures["customers"],
        "item lines: %d" % figures["lines"],
    ]
    if figures["orders"]:
        out.append("lines per order: %.2f" % (figures["lines"] / figures["orders"]))
        out.append("largest order: %d (%d lines)" % figures["largest"])
    return "\n".join(out)
