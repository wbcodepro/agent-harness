"""Compares engine.match with a slow reference and enforces the time bound."""
import hashlib
import json
import subprocess
import sys

TIME_LIMIT = 5.0      # seconds allowed for the 200,000-order workload
KILL_AFTER = 20       # seconds before the timed run is abandoned
BIG_DIGEST = "f8fb1eff270eafefade9a63ac7b4fd3fa55808156e7b9f4a5e81e0b5d7da1213"


class Lcg:
    def __init__(self, seed):
        self.state = (seed * 2654435761 + 1) % 2 ** 64

    def below(self, n):
        self.state = (self.state * 6364136223846793005 + 1442695040888963407) % 2 ** 64
        return (self.state >> 33) % n


def order_id(i):
    return "%05x" % ((i * 40503 + 12345) % 2 ** 20)


def make_orders(n, seed, base=900, width=100, overlap=10, other_pct=20):
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


def reference(orders):
    bids, asks, trades = [], [], []
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
        else:
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
    return trades, {
        "bids": [tuple(o) for o in sorted(bids, key=lambda o: -o[1])],
        "asks": [tuple(o) for o in sorted(asks, key=lambda o: o[1])],
    }


def normal(result):
    trades, book = result
    return ([tuple(t) for t in trades],
            {"bids": [tuple(o) for o in book["bids"]],
             "asks": [tuple(o) for o in book["asks"]]})


def digest(result):
    trades, book = normal(result)
    text = json.dumps([trades, book["bids"], book["asks"]], separators=(",", ":"))
    return hashlib.sha256(text.encode()).hexdigest()


HAND = [
    [],
    [("cancel", "nobody"), ("amend", "nobody", 3)],
    # partial fill keeps its place
    [("sell", "a", 10, 5), ("sell", "b", 10, 5), ("buy", "x", 10, 2), ("buy", "y", 10, 4)],
    # amend down keeps place, amend up goes to the back
    [("sell", "a", 10, 5), ("sell", "b", 10, 5), ("amend", "a", 2), ("buy", "x", 10, 3)],
    [("sell", "a", 10, 5), ("sell", "b", 10, 5), ("amend", "a", 9), ("buy", "x", 10, 6)],
    [("sell", "a", 10, 5), ("sell", "b", 10, 5), ("amend", "a", 5), ("buy", "x", 10, 6)],
    # amend up twice, then cancel: no ghost of the order may trade
    [("buy", "a", 10, 1), ("buy", "b", 10, 1), ("amend", "a", 2), ("amend", "a", 3),
     ("cancel", "a"), ("sell", "x", 9, 5)],
    [("buy", "a", 10, 1), ("buy", "b", 10, 1), ("amend", "a", 2), ("amend", "b", 3),
     ("amend", "a", 1), ("sell", "x", 9, 2)],
    # trade at the resting price, several levels
    [("buy", "a", 12, 1), ("buy", "b", 11, 1), ("buy", "c", 12, 1), ("sell", "x", 10, 5)],
    [("sell", "a", 12, 1), ("sell", "b", 11, 1), ("sell", "c", 11, 1), ("buy", "x", 13, 5)],
    # amend and cancel after a fill
    [("sell", "a", 10, 5), ("buy", "x", 10, 5), ("amend", "a", 4), ("cancel", "a"),
     ("buy", "y", 10, 1)],
    [("sell", "a", 10, 5), ("sell", "b", 10, 1), ("buy", "x", 10, 2), ("amend", "a", 4),
     ("buy", "y", 10, 5)],
]


def small_cases():
    for case in HAND:
        yield case
    for seed in range(150):
        rng = Lcg(1000 + seed)
        n = 20 + rng.below(380)
        width = 2 + rng.below(6)
        yield make_orders(n, seed, base=100, width=width,
                          overlap=1 + rng.below(width), other_pct=10 + rng.below(40))
    for seed in range(5):
        yield make_orders(3000, 500 + seed, base=100, width=30, overlap=8, other_pct=25)


def child():
    """Runs in a subprocess: time the big workload with the candidate."""
    import time
    from engine import match
    orders = make_orders(200_000, seed=7)
    start = time.perf_counter()
    result = match(orders)
    elapsed = time.perf_counter() - start
    print(json.dumps({"elapsed": elapsed, "digest": digest(result)}))


def main():
    from engine import match
    for number, orders in enumerate(small_cases()):
        want = reference(list(orders))
        got = normal(match(list(orders)))
        if got != want:
            print("FAIL: case %d (%d orders) differs from the reference" % (number, len(orders)))
            if len(orders) <= 12:
                print("orders:", orders)
                print("want:  ", want)
                print("got:   ", got)
            return 1
    try:
        run = subprocess.run([sys.executable, __file__, "--child"], capture_output=True,
                             text=True, timeout=KILL_AFTER)
    except subprocess.TimeoutExpired:
        print("FAIL: 200,000 orders not finished after %d s" % KILL_AFTER)
        return 1
    if run.returncode != 0:
        print("FAIL: the 200,000-order run crashed\n" + run.stderr[-2000:])
        return 1
    report = json.loads(run.stdout.strip().splitlines()[-1])
    if report["digest"] != BIG_DIGEST:
        print("FAIL: wrong result on the 200,000-order workload")
        return 1
    if report["elapsed"] > TIME_LIMIT:
        print("FAIL: 200,000 orders took %.2f s (limit %.1f s)" % (report["elapsed"], TIME_LIMIT))
        return 1
    print("ok (%.2f s)" % report["elapsed"])
    return 0


if __name__ == "__main__":
    if "--child" in sys.argv:
        child()
    else:
        sys.exit(main())
