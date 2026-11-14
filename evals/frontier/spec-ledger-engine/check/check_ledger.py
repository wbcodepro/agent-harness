"""Replays recorded scenarios against ledger.Ledger and compares every result."""
import copy
import json
import os
import sys
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))


def scribble(value):
    """Change a value the ledger handed out or was handed; SPEC.md allows the caller to."""
    if isinstance(value, dict):
        for item in list(value.values()):
            scribble(item)
        for field in list(value):
            if isinstance(value[field], (str, bool, int)):
                value[field] = "scribbled"
        value["scribbled"] = True
    elif isinstance(value, list):
        for item in value:
            scribble(item)
        value.append("scribbled")


def main():
    with open(os.path.join(HERE, "ledger_cases.json")) as handle:
        scenarios = json.load(handle)
    try:
        from ledger import Ledger
    except Exception:
        traceback.print_exc()
        return 1
    failures = 0
    total = 0
    for scenario in scenarios:
        try:
            ledger = Ledger(copy.deepcopy(scenario["currencies"]))
        except Exception:
            failures += 1
            if failures <= 3:
                print("%s: Ledger() raised" % scenario["name"])
                traceback.print_exc()
            continue
        for index, (op, want) in enumerate(zip(scenario["ops"], scenario["expected"])):
            total += 1
            given = copy.deepcopy(op)
            try:
                got = ledger.apply(given)
            except Exception as error:
                got = "raised %r" % (error,)
            problem = None
            if got != want:
                problem = "wrong result"
            elif given != op:
                problem = "apply modified its argument"
            if problem:
                failures += 1
                if failures <= 25:
                    print("%s op %d: %s\n  op:   %s\n  want: %s\n  got:  %s"
                          % (scenario["name"], index, problem, json.dumps(op),
                             json.dumps(want), json.dumps(got, default=repr)))
            scribble(given)
            scribble(got)
    print("%d operations, %d failures" % (total, failures))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
