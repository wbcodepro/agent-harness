"""A small limit-order matching engine.

`match(orders)` processes a list of orders one after another and returns
`(trades, book)`.

Orders are tuples:

    ("buy",    oid, price, qty)   a bid for `qty` units at `price` or lower
    ("sell",   oid, price, qty)   an offer of `qty` units at `price` or higher
    ("cancel", oid)               withdraw a resting order
    ("amend",  oid, qty)          change the open quantity of a resting order

`oid` is a string, unique among all buy and sell orders of one call.
`price` and `qty` are positive integers.

Rules

1. An incoming buy trades against resting sells whose price is at or below
   the buy's price. It takes the lowest-priced sell first; among sells at
   the same price it takes the one that has been waiting longest. The
   trade is for the smaller of the two open quantities, at the resting
   order's price. This repeats until the buy is filled or no resting sell
   is cheap enough. Whatever is left of the buy rests in the book, at the
   back of the queue.
2. An incoming sell is the mirror image: it trades against resting buys
   whose price is at or above its own, highest price first, longest
   waiting first among equal prices, at the resting order's price.
3. A resting order that is only partly filled keeps its place in the
   queue. A resting order that is completely filled leaves the book.
4. A cancel removes the order from the book if it is resting there.
   Otherwise (unknown id, already filled, already cancelled) it does
   nothing.
5. An amend sets the open quantity of a resting order to `qty`. If that
   is less than or equal to its current open quantity the order keeps its
   place in the queue. If it is more, the order moves to the back of the
   queue, as if it had just arrived. An amend of an order that is not
   resting does nothing.

Result

`trades` is a list of `(buy_oid, sell_oid, price, qty)` in the order the
trades happened.

`book` is `{"bids": [...], "asks": [...]}`, each a list of
`(oid, price, open_qty)` for the orders still resting at the end. Bids are
ordered from the highest price down, asks from the lowest price up; orders
at the same price appear in queue order (front of the queue first).
"""


def match(orders):
    bids = []  # resting buys in queue order: [oid, price, open_qty]
    asks = []  # resting sells in queue order
    trades = []
    for order in orders:
        kind = order[0]
        if kind == "cancel":
            oid = order[1]
            bids = [o for o in bids if o[0] != oid]
            asks = [o for o in asks if o[0] != oid]
        elif kind == "amend":
            oid, qty = order[1], order[2]
            for side in (bids, asks):
                for o in side:
                    if o[0] == oid:
                        if qty > o[2]:
                            side.remove(o)
                            side.append(o)
                        o[2] = qty
                        break
        elif kind == "buy":
            _, oid, price, qty = order
            while qty > 0:
                best = None
                for o in asks:
                    if o[1] <= price and (best is None or o[1] < best[1]):
                        best = o
                if best is None:
                    break
                fill = min(qty, best[2])
                trades.append((oid, best[0], best[1], fill))
                qty -= fill
                best[2] -= fill
                if best[2] == 0:
                    asks.remove(best)
            if qty > 0:
                bids.append([oid, price, qty])
        elif kind == "sell":
            _, oid, price, qty = order
            while qty > 0:
                best = None
                for o in bids:
                    if o[1] >= price and (best is None or o[1] > best[1]):
                        best = o
                if best is None:
                    break
                fill = min(qty, best[2])
                trades.append((best[0], oid, best[1], fill))
                qty -= fill
                best[2] -= fill
                if best[2] == 0:
                    bids.remove(best)
            if qty > 0:
                asks.append([oid, price, qty])
        else:
            raise ValueError("unknown order kind: %r" % (kind,))
    book = {
        "bids": [tuple(o) for o in sorted(bids, key=lambda o: -o[1])],
        "asks": [tuple(o) for o in sorted(asks, key=lambda o: o[1])],
    }
    return trades, book
