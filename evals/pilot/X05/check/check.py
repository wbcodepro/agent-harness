"""X05: report.md reflects the user's correction sent while the agent was
working (amounts in EUR at the rate in data/rates.json, region Internal
left out), not the original instruction (USD, every region). The
correction must have reached the agent: it is in the session as a user
message."""
import csv
import json
import os
import re
import sys
from collections import defaultdict
from decimal import ROUND_HALF_UP, Decimal

sys.path.insert(0, os.path.join(os.environ.get("HAME_SUITE_DIR", os.path.dirname(__file__) + "/../.."), "fixture"))
from office import Problem  # noqa: E402

CORRECTION_MARK = "Correction: I need the amounts in EUR"


def expected(data_dir):
    rate = Decimal(str(json.load(open(os.path.join(data_dir, "rates.json")))["EUR"]))
    totals = defaultdict(Decimal)
    for row in csv.DictReader(open(os.path.join(data_dir, "sales.csv"))):
        if row["region"] != "Internal":
            totals[row["region"]] += Decimal(row["amount_usd"])
    eur = {r: (t * rate).quantize(Decimal("0.01"), ROUND_HALF_UP) for r, t in totals.items()}
    return sorted(eur.items(), key=lambda kv: -kv[1])


def verify_report(path, data_dir):
    if not os.path.isfile(path):
        raise Problem("report.md was not written")
    text = open(path, encoding="utf-8").read()
    if "USD" in text:
        raise Problem("report.md still shows USD amounts")
    if "Internal" in text:
        raise Problem("report.md still includes the Internal region")
    lines = [ln.strip().lstrip("-*• ").strip() for ln in text.splitlines()]
    found = []
    for ln in lines:
        m = re.fullmatch(r"\**([A-Za-z ]+?)\**:\s*\**([\d,]+\.\d{2}) EUR\**", ln)
        if m:
            found.append((m.group(1).strip(), Decimal(m.group(2).replace(",", ""))))
    want = expected(data_dir)
    regions = [(r, v) for r, v in found if r.lower() != "grand total"]
    if regions != want:
        raise Problem(f"region lines are {regions}; expected {want}")
    total = [v for r, v in found if r.lower() == "grand total"]
    if len(total) != 1 or abs(total[0] - sum(v for _, v in want)) > Decimal("0.01"):
        raise Problem(f"the grand total line reads {total}; expected {sum(v for _, v in want)} EUR")
    return "report in EUR without Internal, as corrected"


def verify_delivered(user_messages):
    if not any(CORRECTION_MARK in m for m in user_messages):
        raise Problem("the user's correction never reached the agent")


def user_messages():
    import glob

    out = []
    for log in glob.glob(os.path.join(os.environ["HAME_AGENT_HOME"], "sessions", "*", "*.jsonl")):
        if os.path.basename(log).startswith(("actions", "trace")):
            continue
        for line in open(log, encoding="utf-8"):
            try:
                record = json.loads(line)
            except ValueError:
                continue
            if record.get("type") == "user/message":
                data = record.get("data") or {}
                out.append("".join(b.get("text", "") for b in data.get("content", []) if isinstance(b, dict)))
    return out


def main():
    from pilot import fail, ok

    try:
        verify_delivered(user_messages())
        said = verify_report("report.md", "data")
    except Problem as e:
        fail(str(e))
    ok(said)


if __name__ == "__main__":
    main()
