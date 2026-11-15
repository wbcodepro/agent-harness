"""D03: the right one of five similarly named files was opened and
changed in place; the other four are byte-for-byte unchanged."""
import hashlib
import json
import os
import sys

sys.path.insert(0, os.path.join(os.environ["HAME_SUITE_DIR"], "fixture"))
from pilot import fail, ok  # noqa: E402

folder = os.path.join(os.environ["HAME_OUTBOX"], "docs")
before = json.load(open(os.path.join(os.environ["HAME_RUN_DIR"], "d03.json")))
target = "budget-2026-final.txt"
names = sorted(os.listdir(folder))
extra = [n for n in names if n not in before]
changed = [n for n in before if n != target and
           hashlib.sha256(open(os.path.join(folder, n), "rb").read()).hexdigest() != before[n]]
if changed:
    fail(f"other files changed: {changed}")
text = open(os.path.join(folder, target), encoding="utf-8").read()
lines = [line.rstrip() for line in text.strip().splitlines()]
want = ["Budget 2026 (final)", "Total: 121,500 EUR", "Approved by Lan"]
if lines != want:
    fail(f"{target} reads {lines!r}, expected {want!r}")
if text.lstrip().startswith("{\\rtf"):
    fail(f"{target} was saved as rich text, not plain text")
if extra:
    fail(f"new files appeared: {extra}")
ok("the final budget was changed in place; its look-alikes are untouched")
