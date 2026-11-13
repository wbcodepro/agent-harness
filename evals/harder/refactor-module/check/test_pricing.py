import inspect
import pricing
from pricing import price_order, subtotal, discount_for, tax_on

items = [{"price": 300, "quantity": 2}, {"price": 250.5, "quantity": 2}]
assert subtotal(items) == 1101.0
assert discount_for(1101.0, {"member": True}) == 115.15
assert discount_for(1101.0, {}) == 5.05
assert discount_for(100, {}) == 0
assert tax_on(100, "north") == 20.0 and tax_on(100, "south") == 10.0 and tax_on(100, "west") == 15.0
assert price_order(items, {"member": True}, "north") == 1183.02
assert price_order(items, {}, "elsewhere") == 1260.34
assert price_order([], {}, "south") == 0
source = inspect.getsource(pricing.price_order)
for name in ("subtotal(", "discount_for(", "tax_on("):
    assert name in source, f"price_order must call {name}"
print("ok")
