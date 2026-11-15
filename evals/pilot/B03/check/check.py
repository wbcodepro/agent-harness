"""B03: the right file, uploaded once, to R-102."""
import hashlib
import os
import sys

sys.path.insert(0, os.path.join(os.environ["HAME_SUITE_DIR"], "fixture"))
from pilot import fail, ok, state  # noqa: E402

want = hashlib.sha256(open("invoices/invoice-2026-03.pdf", "rb").read()).hexdigest()
uploads = state()["uploads"]
if not uploads:
    fail("nothing was uploaded")
if len(uploads) > 1:
    fail(f"{len(uploads)} uploads: {[(u['record'], u['filename']) for u in uploads]}")
u = uploads[0]
if u["record"] != "R-102":
    fail(f"uploaded to {u['record']}, not R-102")
if u["sha256"] != want:
    fail(f"uploaded {u['filename']} ({u['size']} bytes), not invoice-2026-03.pdf")
ok("invoice-2026-03.pdf attached to R-102 once")
