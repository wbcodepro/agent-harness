from order import Order

def run(events):
    o = Order()
    for e in events:
        o.apply(e)
    return o

o = Order()
assert o.state == "new" and o.history == ["new"]
assert o.apply("pay") == "paid"
assert run(["pay", "ship", "deliver"]).history == ["new", "paid", "shipped", "delivered"]
assert run(["cancel"]).state == "cancelled"
assert run(["pay", "cancel"]).state == "refunded"
assert run(["pay", "ship", "return", "refund"]).state == "refunded"

o = run(["pay"])
try:
    o.apply("deliver")
    raise SystemExit("deliver from paid should fail")
except ValueError as e:
    assert "paid" in str(e) and "deliver" in str(e), str(e)
assert o.state == "paid" and o.history == ["new", "paid"]

for final in (["cancel"], ["pay", "refund"]):
    o = run(final)
    for event in ("pay", "cancel", "tick", "refund"):
        assert not o.can(event)
        try:
            o.apply(event)
            raise SystemExit(f"{event} in a final state should fail")
        except ValueError:
            pass

o = run(["pay", "tick", "ship", "tick", "deliver", "tick", "tick", "tick"])
assert o.history == ["new", "paid", "shipped", "delivered"], o.history
assert o.can("return") and o.apply("return") == "returned"
late = run(["pay", "ship", "deliver", "tick", "tick", "tick", "tick"])
assert not late.can("return")
try:
    late.apply("return")
    raise SystemExit("a return after four ticks should fail")
except ValueError:
    pass
assert late.state == "delivered"
# Ticks before delivery do not count, and can() changes nothing.
o = run(["pay", "ship", "tick", "tick", "tick", "tick", "tick"])
assert o.can("return") and o.can("deliver") and o.state == "shipped"
assert run(["pay", "ship", "tick", "tick", "tick", "tick", "deliver"]).can("return")
print("ok")
