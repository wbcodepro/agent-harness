"""Order lines: the flat view the product reports work on."""


def order_lines(orders):
    """Flatten orders into (order_number, item) pairs, one per item line."""
    lines = []
    for order in orders:
        for item in order["item"]:
            lines.append((order["order"], item))
    return lines


def lines_for(lines, order_number):
    """Return the items of one order from a list of order lines."""
    return [item for number, item in lines if number == order_number]
