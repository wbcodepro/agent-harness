"""B01: one order with the shipping and billing fields in the right places."""
import os
import sys

sys.path.insert(0, os.path.join(os.environ["HAME_SUITE_DIR"], "fixture"))
from pilot import fail, ok, state  # noqa: E402

EXPECTED = {
    "ship_name": "Lan Nguyen",
    "ship_city": "Hanoi",
    "bill_name": "Acme GmbH",
    "bill_city": "Berlin",
    "country": "de",
    "express": "yes",
    "note": "Leave at the front desk",
}

subs = state()["submissions"]
if not subs:
    fail("no order was submitted")
if len(subs) > 1:
    fail(f"{len(subs)} orders were submitted; exactly one was asked for")
got = {k: v.strip() for k, v in subs[0].items()}
wrong = {k: (got.get(k), v) for k, v in EXPECTED.items() if got.get(k) != v}
if wrong:
    fail(f"wrong fields (got, expected): {wrong}")
ok("one order with every field in place")
