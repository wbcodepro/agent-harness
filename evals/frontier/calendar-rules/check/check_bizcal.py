"""Checks bizcal.py against SPEC.md, case by case."""
import copy
import json
import os
import signal
import sys
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
FORBIDDEN = ("zoneinfo", "pytz", "tzdata", "dateutil")


class Slow(Exception):
    pass


def on_alarm(signum, frame):
    raise Slow()


def show(value):
    try:
        return json.dumps(value, ensure_ascii=False)
    except (TypeError, ValueError):
        return repr(value)


def main():
    with open(os.path.join(HERE, "bizcal_cases.json"), encoding="utf-8") as handle:
        cases = json.load(handle)
    try:
        import bizcal
    except Exception:
        traceback.print_exc()
        return 1
    if hasattr(signal, "SIGALRM"):
        signal.signal(signal.SIGALRM, on_alarm)
    started = time.time()
    failures = 0
    for index, case in enumerate(cases):
        args = copy.deepcopy(case["args"])
        kwargs = copy.deepcopy(case.get("kwargs", {}))
        want = case["expect"]
        if hasattr(signal, "SIGALRM"):
            signal.alarm(5)
        try:
            value = getattr(bizcal, case["fn"])(*args, **kwargs)
            try:
                got = {"value": json.loads(json.dumps(value))}
            except (TypeError, ValueError):
                got = {"value": repr(value)}
        except Slow:
            got = {"took": "more than 5 seconds"}
        except ValueError:
            got = {"raises": "ValueError"}
        except Exception as error:
            got = {"raises": "%s: %s" % (type(error).__name__, error)}
        finally:
            if hasattr(signal, "SIGALRM"):
                signal.alarm(0)
        if got != want:
            failures += 1
            if failures <= 30:
                call = "%s(%s%s)" % (case["fn"], ", ".join(show(a) for a in case["args"]),
                                     "".join(", %s=%s" % (k, show(v)) for k, v in kwargs.items()))
                print("case %d: %s\n  want: %s\n  got:  %s" % (index, call, show(want), show(got)))
        if time.time() - started > 50:
            print("stopped: the cases took more than 50 seconds")
            return 1
    used = [name for name in FORBIDDEN if name in sys.modules]
    if used:
        failures += 1
        print("bizcal uses a time-zone library, which SPEC.md forbids: %s" % ", ".join(used))
    print("%d cases, %d failures" % (len(cases), failures))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
