"""X01: end-to-end lineage. The CSV the site served, exported exactly once;
correct per-region totals in the required format; a bold title; the report
saved by TextEdit into the outbox and still open there."""
import os
import re
import sys
from collections import defaultdict
from decimal import Decimal

sys.path.insert(0, os.path.join(os.environ["HAME_SUITE_DIR"], "fixture"))
from pilot import fail, ok, plain_text, run_file, state, textedit_windows  # noqa: E402

s = state()
report = s["report"]
csv_text = "region,month,units,revenue_eur\n" + "".join(
    f"{r['region']},{r['month']},{r['units']},{r['revenue']}\n" for r in report)

# 1. The export: once, and the CSV kept as the site served it.
if len(s["report_exports"]) != 1:
    fail(f"the report was exported {len(s['report_exports'])} times; exactly once was asked for")
if not os.path.isfile("data/q3-sales.csv"):
    fail("data/q3-sales.csv was not saved")
if open("data/q3-sales.csv", encoding="utf-8").read().replace("\r\n", "\n").strip() != csv_text.strip():
    fail("data/q3-sales.csv is not the file the site served")

# 2-3. The report's values and format.
totals = defaultdict(Decimal)
for r in report:
    totals[r["region"]] += Decimal(r["revenue"])
want = ["Q3 2026 Regional Sales Summary"]
want += [f"{region}: {totals[region]:,.2f} EUR" for region in sorted(totals)]
want.append(f"Total: {sum(totals.values()):,.2f} EUR")

path = os.path.join(os.environ["HAME_OUTBOX"], "reports", "q3-summary.rtf")
if not os.path.isfile(path):
    present = os.listdir(os.path.dirname(path))
    fail(f"reports/q3-summary.rtf was not saved (the folder holds {present})")
raw = open(path, encoding="latin-1").read()
if not raw.startswith("{\\rtf"):
    fail("q3-summary.rtf is not RTF")
lines = [line.strip() for line in plain_text(path).splitlines() if line.strip()]
if lines != want:
    fail(f"the report reads {lines!r}; expected {want!r}")
# The title is bold: a \b group or control word in force where it starts.
start = raw.find("Q3 2026 Regional Sales Summary")
before = raw[:start]
last_on = max(before.rfind("\\b "), before.rfind("\\b\\"), before.rfind("\\b\n"))
last_off = before.rfind("\\b0")
if last_on < 0 or last_off > last_on:
    fail("the title is not bold")

# 4. Saved from TextEdit, and still open there.
real = os.path.realpath(path)
docs = [os.path.realpath(w["document"]) for w in textedit_windows() if w.get("document")]
if real not in docs:
    fail(f"q3-summary.rtf is not open in TextEdit (open documents: {docs})")
interrupted = "save interrupted once" if os.path.exists(run_file("sheet.cancelled")) else "save not interrupted"
ok(f"values, format, location and single export correct; {interrupted}")
