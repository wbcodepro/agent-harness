"""R04: the real status, although the first page load hung and failed,
the second was slow, and the browser helper was killed meanwhile."""
import os
import sys

sys.path.insert(0, os.path.join(os.environ["HAME_SUITE_DIR"], "fixture"))
from pilot import fail, ok, run_file, state  # noqa: E402

s = state()
want = s["shipments"]["S-42"]
if not os.path.isfile("status.txt"):
    fail(f"status.txt was not written ({s['shipment_requests']} page requests)")
got = open("status.txt", encoding="utf-8").read().strip()
if got != want and want not in got:
    fail(f"status.txt reads {got!r}, not {want!r}")
killed = "helper killed" if os.path.exists(run_file("browser.killed")) else "helper not killed"
ok(f"status correct after {s['shipment_requests']} page requests ({killed})")
