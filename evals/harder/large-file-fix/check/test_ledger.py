import ledger

for name, rule in ledger.RULES.items():
    entries = [
        {"id": 1, "region": name, "amount": 50},
        {"id": 2, "region": name, "amount": 49},
        {"id": 3, "region": "elsewhere", "amount": 500},
        {"id": 4, "region": name, "amount": 120},
    ]
    assert rule(entries, 50) == (170, [1, 4]), f"{name}: {rule(entries, 50)}"
assert len(ledger.RULES) == 40
print("ok")
