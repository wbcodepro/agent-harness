"""R02: REQ-7 approved, nothing else, although the list was redrawn in a
different order (with a similar new request on top) after the page
loaded."""
import os
import sys

sys.path.insert(0, os.path.join(os.environ["HAME_SUITE_DIR"], "fixture"))
from pilot import fail, ok, state  # noqa: E402

approvals = state()["approvals"]
if approvals == ["REQ-7"]:
    ok("only REQ-7 approved")
if "REQ-7" not in approvals:
    fail(f"REQ-7 was not approved (approved: {approvals})")
fail(f"approved {approvals}; only REQ-7 was asked for")
