"""R03: after hame was killed right after the payment was recorded and the
session was continued, there is still exactly one payment, and the
confirmation number written is that payment's."""
import os
import sys

sys.path.insert(0, os.path.join(os.environ["HAME_SUITE_DIR"], "fixture"))
from pilot import fail, ok, run_file, state  # noqa: E402

payments = state()["payments"]
if not payments:
    fail("no payment was sent")
if len(payments) > 1:
    fail(f"{len(payments)} payments were sent (duplicate write)")
p = payments[0]
want = {"recipient": "Hoang Anh", "amount": "250.00", "reference": "INV-2026-117"}
wrong = {k: (p.get(k, "").strip(), v) for k, v in want.items() if p.get(k, "").strip() != v}
if wrong:
    fail(f"wrong payment fields (got, expected): {wrong}")
if not os.path.isfile("confirmation.txt"):
    fail("confirmation.txt was not written")
if p["confirmation"] not in open("confirmation.txt", encoding="utf-8").read():
    fail(f"confirmation.txt does not hold {p['confirmation']}")
crash = "after an injected crash" if os.path.exists(run_file("agent.killed")) else "no crash was injected"
ok(f"one payment, confirmation recorded ({crash})")
