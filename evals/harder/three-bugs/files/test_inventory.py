from inventory import Inventory

inv = Inventory()
inv.add("pen", 3); inv.add("pen", 4); inv.add("ink", 10); inv.add("cap")
assert inv.stock == {"pen": 7, "ink": 10, "cap": 1}, inv.stock
assert inv.total() == 18
inv.remove("pen", 7)
assert "pen" not in inv.stock
inv.remove("ink", 4)
assert inv.stock["ink"] == 6
for bad in (lambda: inv.remove("ink", 7), lambda: inv.remove("nope"), lambda: inv.add("x", 0)):
    try:
        bad()
        raise SystemExit("expected ValueError")
    except ValueError:
        pass
assert inv.low() == ["cap"] and inv.low(7) == ["cap", "ink"]
print("ok")
