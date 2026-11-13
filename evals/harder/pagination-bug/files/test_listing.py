from app.search import search
from app.paging import page_count

items = [f"item-{n}" for n in range(25)]
result = search(items, "item", 10)
assert [len(p) for p in result] == [10, 10, 5], [len(p) for p in result]
assert search(items, "item", 5)[-1] == ["item-20", "item-21", "item-22", "item-23", "item-24"]
assert search(items, "nothing") == []
assert page_count(0, 10) == 0 and page_count(10, 10) == 1 and page_count(11, 10) == 2
print("ok")
