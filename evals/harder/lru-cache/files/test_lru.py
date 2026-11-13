from lru import LRUCache

c = LRUCache(2)
assert c.get("a") is None
c.put("a", 1); c.put("b", 2)
assert c.get("a") == 1          # a is now most recent
c.put("c", 3)                   # evicts b
assert c.get("b") is None and c.get("c") == 3 and c.get("a") == 1
c.put("a", 10)                  # update refreshes, no eviction
assert len(c) == 2 and c.get("a") == 10
c.put("d", 4)                   # evicts c
assert c.get("c") is None and c.get("d") == 4
try:
    LRUCache(0)
    raise SystemExit("capacity 0 must raise ValueError")
except ValueError:
    pass
big = LRUCache(1000)
for i in range(200000):
    big.put(i, i)
assert len(big) == 1000 and big.get(199999) == 199999 and big.get(0) is None
print("ok")
