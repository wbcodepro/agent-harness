"""Summarizes `hame eval --json` reports into the Markdown tables used in
docs/release/EVIDENCE.md: one row per task and arm (passes over attempts,
false successes, verified latency p50, model requests, tokens by class,
request bytes, cost) and a totals row.

    python3 evals/pilot/report.py RESULT.json [RESULT.json ...]

Every attempt counts, failures, errors and timeouts included; unsupported
tasks are listed but left out of the totals. Nothing is estimated: a value
the records lack is shown as "-".
"""

import json
import sys
from collections import defaultdict


def pct(values, p):
    if not values:
        return None
    values = sorted(values)
    rank = p * (len(values) - 1)
    lo, hi = int(rank), min(int(rank) + 1, len(values) - 1)
    return values[lo] + (values[hi] - values[lo]) * (rank - lo)


def fmt(v, spec="{:,}"):
    return "-" if v is None else spec.format(v)


def main(paths):
    records = []
    for path in paths:
        records += json.load(open(path))["records"]
    groups = defaultdict(list)
    for r in records:
        groups[(r["task_id"], r["arm"])].append(r)
    print("| Task | Arm | Pass | False success | Fail | Timeout | Error | Verified p50 s | All p50 s "
          "| Requests | Uncached in | Cache read | Cache write | Output | Request MB | Cost $ |")
    print("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    totals = defaultdict(float)
    for (task, arm), rs in sorted(groups.items()):
        if all(r["outcome"] == "unsupported" for r in rs):
            print(f"| {task} | {arm} | unsupported: {rs[0]['note']} |" + " |" * 13)
            continue
        rs = [r for r in rs if r["outcome"] != "unsupported"]
        count = defaultdict(int)
        for r in rs:
            count[r["outcome"]] += 1
        passed = [r["total_ms"] / 1000 for r in rs if r["outcome"] == "pass"]
        every = [r["total_ms"] / 1000 for r in rs]
        s = lambda k: sum(r.get(k) or 0 for r in rs)  # noqa: E731
        cost = sum(r["billed_cost"] or 0 for r in rs) if all(r.get("billed_cost") is not None for r in rs) else None
        for k in ["model_calls", "uncached_input", "cached_input", "cache_write", "output", "request_bytes"]:
            totals[k] += s(k)
        totals["attempts"] += len(rs)
        totals["pass"] += count["pass"]
        totals["false_success"] += count["false_success"]
        totals["cost"] += cost or 0
        print(f"| {task} | {arm} | {count['pass']}/{len(rs)} | {count['false_success']} | {count['fail']} "
              f"| {count['timeout']} | {count['error']} | {fmt(pct(passed, 0.5), '{:.1f}')} "
              f"| {fmt(pct(every, 0.5), '{:.1f}')} | {s('model_calls')} | {s('uncached_input'):,} "
              f"| {s('cached_input'):,} | {s('cache_write'):,} | {s('output'):,} "
              f"| {s('request_bytes') / 1e6:.2f} | {fmt(cost, '{:.4f}')} |")
    n, p = int(totals["attempts"]), int(totals["pass"])
    print()
    print(f"Attempts {n}, passes {p}, false successes {int(totals['false_success'])}; "
          f"requests {int(totals['model_calls']):,}; tokens: uncached {int(totals['uncached_input']):,}, "
          f"cache read {int(totals['cached_input']):,}, cache write {int(totals['cache_write']):,}, "
          f"output {int(totals['output']):,}; request bytes {totals['request_bytes'] / 1e6:.2f} MB; "
          f"cost ${totals['cost']:.4f}; per attempt ${totals['cost'] / max(n, 1):.4f}; "
          f"per verified success {'$%.4f' % (totals['cost'] / p) if p else 'undefined'}")
    if len(paths) == 1:
        report = json.load(open(paths[0]))
        for c in report.get("comparisons", []):
            if c["mean_diff"] is None:
                continue
            print(f"- {c['metric']} {c['arm']}-{c['baseline']}: mean {c['mean_diff']:.4f}, "
                  f"95% CI [{c['ci95_low']:.4f}, {c['ci95_high']:.4f}] over {c['tasks']} tasks")


if __name__ == "__main__":
    main(sys.argv[1:])
