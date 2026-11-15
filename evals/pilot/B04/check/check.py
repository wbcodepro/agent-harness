"""B04: the February ledger export, complete, from the site's export."""
import os
import sys

sys.path.insert(0, os.path.join(os.environ["HAME_SUITE_DIR"], "fixture"))
from pilot import fail, ok, state  # noqa: E402

s = state()
rows = [r for r in s["ledger"] if "2026-02-01" <= r["date"] <= "2026-02-28"]
expected = "date,amount\n" + "".join(f"{r['date']},{r['amount']}\n" for r in rows)
path = "exports/ledger-2026-02.csv"
if not os.path.isfile(path):
    fail(f"{path} was not written")
got = open(path, encoding="utf-8").read()
if got.replace("\r\n", "\n").strip() != expected.strip():
    lines = got.strip().splitlines()
    fail(f"{path} has {len(lines) - 1} data rows ({lines[1] if len(lines) > 1 else ''} ... "
         f"{lines[-1] if lines else ''}); expected {len(rows)} rows 2026-02-01..2026-02-28")
if not any(e["from"] == "2026-02-01" and e["to"] == "2026-02-28" for e in s["exports"]):
    fail(f"the site never exported that range: {s['exports']}")
ok(f"{len(rows)} rows, exported {len(s['exports'])} time(s)")
