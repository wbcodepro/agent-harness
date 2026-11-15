"""Compares configurations measured by `hame eval --arm ...` against a
baseline arm, in the terms of the research's engineering targets: median
verified completion time, model decisions (requests), uncached input
tokens and uploaded image bytes, plus strict completion and false
successes.

    python3 evals/pilot/compare.py --baseline pixels RESULT.json [...]

For each arm and metric it prints the arm's value, the reduction against
the baseline as a percentage, and a 95% percentile-bootstrap interval
that resamples tasks (paired: each task's per-arm means are compared, so
task difficulty cancels). Time is the attempt's agent + verification time
over every attempt (failures and timeouts included), and also the median
over verified attempts only. Nothing is estimated: a metric missing from a
record makes that record count as unknown for the metric, and the count
of unknowns is printed.
"""

import argparse
import json
import random
import statistics
from collections import defaultdict


def value(record, metric):
    if metric == "time_s":
        a, v = record.get("agent_ms"), record.get("verification_ms")
        return None if a is None or v is None else (a + v) / 1000
    if metric == "requests":
        return record.get("model_calls")
    if metric == "uncached_input":
        return record.get("uncached_input")
    if metric == "input_all":
        parts = [record.get(k) for k in ("uncached_input", "cached_input", "cache_write")]
        return None if any(p is None for p in parts) else sum(parts)
    if metric == "image_bytes":
        return record.get("image_bytes")
    if metric == "request_bytes":
        return record.get("request_bytes")
    if metric == "cost":
        return record.get("billed_cost")
    if metric == "pass":
        return 1.0 if record.get("outcome") == "pass" else 0.0
    raise KeyError(metric)


METRICS = [
    ("time_s", "agent + verification s per attempt"),
    ("requests", "model requests (decisions) per attempt"),
    ("uncached_input", "uncached input tokens per attempt"),
    ("input_all", "all input tokens (uncached + cache read + cache write)"),
    ("image_bytes", "image bytes uploaded per attempt"),
    ("request_bytes", "request bytes per attempt"),
    ("cost", "priced cost $ per attempt"),
    ("pass", "strict completion rate"),
]


def per_task_means(records, metric):
    groups = defaultdict(list)
    unknown = 0
    for r in records:
        v = value(r, metric)
        if v is None:
            unknown += 1
            continue
        groups[r["task_id"]].append(v)
    return {t: statistics.fmean(vs) for t, vs in groups.items()}, unknown


def bootstrap(tasks, base, arm, resamples, seed):
    rng = random.Random(seed)
    out = []
    for _ in range(resamples):
        sample = [rng.choice(tasks) for _ in tasks]
        b = sum(base[t] for t in sample)
        a = sum(arm[t] for t in sample)
        if b > 0:
            out.append(1 - a / b)
    out.sort()
    if not out:
        return None, None
    return out[int(0.025 * (len(out) - 1))], out[int(0.975 * (len(out) - 1))]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--resamples", type=int, default=2000)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("reports", nargs="+")
    args = parser.parse_args()
    records = []
    for path in args.reports:
        records += json.load(open(path))["records"]
    by_arm = defaultdict(list)
    for r in records:
        by_arm[r["arm"]].append(r)
    if args.baseline not in by_arm:
        raise SystemExit(f"no arm {args.baseline!r}; arms: {sorted(by_arm)}")
    tasks_all = sorted({r["task_id"] for r in records})
    print(f"Tasks ({len(tasks_all)}): {', '.join(tasks_all)}")
    for arm, rs in sorted(by_arm.items()):
        passed = [r for r in rs if r["outcome"] == "pass"]
        false = sum(1 for r in rs if r["outcome"] == "false_success")
        verified = [value(r, "time_s") for r in passed if value(r, "time_s") is not None]
        med = statistics.median(verified) if verified else None
        print(f"\n## {arm}: {len(passed)}/{len(rs)} strict, {false} false success, "
              f"median verified time {med and round(med, 1)} s")
    print()
    base = by_arm[args.baseline]
    print("| Metric | Arm | Baseline mean | Arm mean | Reduction vs baseline | 95% CI (tasks resampled) | Unknown |")
    print("|---|---|---:|---:|---:|---|---:|")
    for metric, label in METRICS:
        bmeans, bunk = per_task_means(base, metric)
        for arm, rs in sorted(by_arm.items()):
            if arm == args.baseline:
                continue
            ameans, aunk = per_task_means(rs, metric)
            tasks = sorted(set(bmeans) & set(ameans))
            if not tasks:
                continue
            b = statistics.fmean(bmeans[t] for t in tasks)
            a = statistics.fmean(ameans[t] for t in tasks)
            fmt = (lambda x: f"{x:,.4f}") if metric in ("cost", "pass") else (lambda x: f"{x:,.1f}")
            if metric == "pass":
                # A rate: the difference in points, not a reduction.
                rng = random.Random(args.seed)
                diffs = sorted(
                    statistics.fmean(ameans[t] - bmeans[t] for t in s)
                    for s in ([rng.choice(tasks) for _ in tasks] for _ in range(args.resamples))
                )
                lo, hi = diffs[int(0.025 * (len(diffs) - 1))], diffs[int(0.975 * (len(diffs) - 1))]
                print(f"| {label} (difference) | {arm} | {fmt(b)} | {fmt(a)} | "
                      f"{(a - b) * 100:+.1f} pts | [{lo * 100:+.1f}, {hi * 100:+.1f}] pts | {bunk + aunk} |")
                continue
            red = (1 - a / b) if b else None
            lo, hi = bootstrap(tasks, bmeans, ameans, args.resamples, args.seed)
            ci = "-" if lo is None else f"[{lo:+.0%}, {hi:+.0%}]"
            print(f"| {label} | {arm} | {fmt(b)} | {fmt(a)} | "
                  f"{'-' if red is None else f'{red:+.0%}'} | {ci} | {bunk + aunk} |")
    # The research's median verified completion time, per arm, over
    # tasks both arms verified at least once.
    print("\nMedian verified completion time over tasks verified by both arms:")
    for arm, rs in sorted(by_arm.items()):
        if arm == args.baseline:
            continue
        both = {r["task_id"] for r in base if r["outcome"] == "pass"} & \
               {r["task_id"] for r in rs if r["outcome"] == "pass"}
        bt = [value(r, "time_s") for r in base if r["outcome"] == "pass" and r["task_id"] in both]
        at = [value(r, "time_s") for r in rs if r["outcome"] == "pass" and r["task_id"] in both]
        if bt and at:
            mb, ma = statistics.median(bt), statistics.median(at)
            print(f"- {arm}: {ma:.1f} s vs {args.baseline} {mb:.1f} s "
                  f"({1 - ma / mb:+.0%}) on {len(both)} tasks")


if __name__ == "__main__":
    main()
