"""Counts, from the traces `hame eval --traces` keeps, how many tool calls
each attempt made, how many of them were batches, and how many steps the
batches ran. For reading an ablation of batching.

    python3 evals/pilot/batches.py TRACES_DIR

Prints one line per arm and task: attempts, top-level tool calls, batches,
batch steps (each step is a tool call the model did not have to decide).
"""

import glob
import json
import os
import sys
from collections import defaultdict


def main(root):
    rows = defaultdict(lambda: defaultdict(int))
    for path in sorted(glob.glob(os.path.join(root, "*", "*", "run-*.trace.v1.jsonl"))):
        arm, task = path.split(os.sep)[-3:-1]
        row = rows[(arm, task)]
        row["attempts"] += 1
        for line in open(path, encoding="utf-8"):
            try:
                rec = json.loads(line)
            except ValueError:
                continue
            if rec.get("type") != "span" or rec.get("kind") != "tool":
                continue
            call = rec.get("attrs", {}).get("call_id", "")
            if "-batch-" in call:
                row["batch_steps"] += 1
                if call.endswith("-batch-1"):
                    row["batches"] += 1
            else:
                row["tool_calls"] += 1
    print("| Arm | Task | Attempts | Tool calls | Batches | Batch steps |")
    print("|---|---|---:|---:|---:|---:|")
    for (arm, task), row in sorted(rows.items()):
        print(f"| {arm} | {task} | {row['attempts']} | {row['tool_calls']} | {row['batches']} "
              f"| {row['batch_steps']} |")


if __name__ == "__main__":
    main(sys.argv[1])
