"""The `top` report: best-selling products."""


def units_by_product(lines):
    """Return {product name: total units} over a list of order lines."""
    totals = {}
    for _order_number, (name, quantity) in lines:
        totals[name] = totals.get(name, 0) + quantity
    return totals


def top_products(lines, count=3):
    """Return the `count` best sellers as (name, units) pairs.

    Most units first; products with equal units in alphabetical order.
    """
    totals = units_by_product(lines)
    ranked = sorted(totals.items(), key=lambda pair: (-pair[1], pair[0]))
    return ranked[:count]


def render_top(lines, count=3):
    """Return the text of the `top` report."""
    out = ["Top %d products by units" % count]
    for rank, (name, units) in enumerate(top_products(lines, count), 1):
        out.append("%2d. %-16s %5d" % (rank, name, units))
    return "\n".join(out)
