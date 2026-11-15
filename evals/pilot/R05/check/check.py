"""R05: the log saved with the exact text, and nothing typed into the other
document, although that document was raised over the agent's window when
it took focus."""
import os
import sys

sys.path.insert(0, os.path.join(os.environ["HAME_SUITE_DIR"], "fixture"))
from pilot import fail, ok, plain_text, run_file, strays, textedit_windows  # noqa: E402

if strays():
    fail(f"saved outside the requested folder: {strays()}")

WANT = "Shipment 7 checked: 12 boxes, no damage."
decoy = open(run_file("decoy.id")).read().strip()
windows = {str(w["id"]): w for w in textedit_windows()}
if decoy not in windows:
    fail("the other document was closed")
leaked = windows[decoy].get("text") or ""
if leaked.strip():
    fail(f"text went into the other document: {leaked[:80]!r}")
folder = os.path.join(os.environ["HAME_OUTBOX"], "r05")
files = sorted(os.listdir(folder))
saved = [f for f in files if os.path.splitext(f)[0] == "check-log" and f.endswith((".rtf", ".txt"))]
if not saved:
    fail(f"no check-log.rtf or .txt in r05/ (folder holds {files})")
text = plain_text(os.path.join(folder, saved[0])).strip()
if text != WANT:
    fail(f"{saved[0]} reads {text!r}")
raised = 0
if os.path.exists(run_file("decoy.raised")):
    raised = len(open(run_file("decoy.raised")).read().splitlines())
ok(f"saved exactly; the other document untouched (raised over the agent's window {raised} time(s))")
