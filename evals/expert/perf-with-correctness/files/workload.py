"""Reproducible order streams for trying out the engine."""


class Lcg:
    """A tiny random number generator that gives the same numbers everywhere."""

    def __init__(self, seed):
        self.state = (seed * 2654435761 + 1) % 2 ** 64

    def below(self, n):
        self.state = (self.state * 6364136223846793005 + 1442695040888963407) % 2 ** 64
        return (self.state >> 33) % n


def order_id(i):
    return "%05x" % ((i * 40503 + 12345) % 2 ** 20)


def make_orders(n, seed, base=900, width=100, overlap=10, other_pct=20):
    """`n` orders. Buys are priced in [base, base + width), sells in
    [base + width - overlap, base + 2 * width - overlap). About `other_pct`
    percent of the orders are cancels and amends of earlier ids."""
    rng = Lcg(seed)
    orders = []
    for i in range(n):
        roll = rng.below(100)
        if i > 0 and roll < other_pct:
            target = order_id(rng.below(i))
            if roll % 2 == 0:
                orders.append(("cancel", target))
            else:
                orders.append(("amend", target, 1 + rng.below(60)))
            continue
        qty = 1 + rng.below(50)
        if rng.below(2) == 0:
            orders.append(("buy", order_id(i), base + rng.below(width), qty))
        else:
            orders.append(("sell", order_id(i), base + width - overlap + rng.below(width), qty))
    return orders
