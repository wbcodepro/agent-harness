"""Time match() on the 200,000-order workload."""
import time

from engine import match
from workload import make_orders

orders = make_orders(200_000, seed=7)
start = time.perf_counter()
trades, book = match(orders)
elapsed = time.perf_counter() - start
print("%d orders, %d trades, %d bids and %d asks left, %.2f s"
      % (len(orders), len(trades), len(book["bids"]), len(book["asks"]), elapsed))
