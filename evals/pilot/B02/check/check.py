"""B02: every Tool item from every page, and nothing else."""
import csv
import os
import sys

sys.path.insert(0, os.path.join(os.environ["HAME_SUITE_DIR"], "fixture"))
from pilot import fail, ok, state  # noqa: E402

expected = [(str(it["id"]), it["name"], it["price"]) for it in state()["items"] if it["category"] == "tool"]
if not os.path.isfile("tools.csv"):
    fail("tools.csv was not written")
with open("tools.csv", newline="", encoding="utf-8") as f:
    rows = list(csv.reader(f))
if not rows or [c.strip().lower() for c in rows[0]] != ["id", "name", "price"]:
    fail(f"the header is {rows[0] if rows else None}, not id,name,price")
got = [tuple(c.strip() for c in row) for row in rows[1:] if any(c.strip() for c in row)]
missing = [r[0] for r in expected if r not in got]
extra = [r[0] for r in got if r not in expected]
if missing or extra:
    fail(f"{len(got)} rows; missing ids {missing}, unexpected rows {extra}")
if got != expected:
    fail("the rows are not sorted by id")
ok(f"all {len(expected)} Tool items, across pages")
