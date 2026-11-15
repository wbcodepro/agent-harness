"""D01: one document, saved by TextEdit into the outbox folder, with the
three lines. Reopened through textutil, not through the agent's tool."""
import glob
import os
import sys

sys.path.insert(0, os.path.join(os.environ["HAME_SUITE_DIR"], "fixture"))
from pilot import fail, ok, plain_text, strays  # noqa: E402

if strays():
    fail(f"saved outside the requested folder: {strays()}")

folder = os.path.join(os.environ["HAME_OUTBOX"], "notes")
files = sorted(os.listdir(folder))
saved = [f for f in files if os.path.splitext(f)[0] == "meeting-notes" and f.endswith((".rtf", ".txt"))]
if not saved:
    stray = glob.glob(os.path.join(os.environ["HAME_OUTBOX"], "**", "*meeting*"), recursive=True)
    fail(f"no meeting-notes.rtf or .txt in notes/ (folder holds {files}; elsewhere {stray})")
if len(files) != 1:
    fail(f"the folder holds {files}; exactly one document was asked for")
lines = [line.rstrip() for line in plain_text(os.path.join(folder, saved[0])).strip().splitlines()]
want = ["Agenda for Monday", "1. Budget review", "2. Hiring plan"]
if lines != want:
    fail(f"{saved[0]} reads {lines!r}, expected {want!r}")
ok(f"{saved[0]} saved with the three lines")
