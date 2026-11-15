"""D02: TextEdit's "new document" format set to Plain text through TextEdit
itself. Read back from the preferences, no other TextEdit setting changed,
not written with the `defaults` command; then, when the run started
TextEdit, it is quit and relaunched and its Settings window must show
Plain text selected (the reopened setting). Teardown restores the
original preference."""
import json
import os
import re
import subprocess
import sys
import time

sys.path.insert(0, os.path.join(os.environ.get("HAME_SUITE_DIR", os.path.dirname(__file__) + "/../.."), "fixture"))
from office import Problem  # noqa: E402

DOMAIN, KEY = "com.apple.TextEdit", "RichText"


def ui_state(key):
    """Window frames, panel sizes and the like: what using an app changes
    by itself, not settings."""
    return key.startswith(("NS", "TSM", "Apple", "com.apple"))


def verify_pref(value, before, after):
    if value != "0":
        raise Problem(f"{KEY} is {value!r}; new documents are not set to plain text")
    changed = sorted(k for k in set(before) | set(after)
                     if k != KEY and not ui_state(k) and before.get(k) != after.get(k))
    if changed:
        raise Problem(f"other TextEdit settings changed: {changed}")
    return "new documents are plain text; no other setting changed"


WRITES = re.compile(r"defaults\s+(-currentHost\s+)?(write|delete|import|rename)"
                    r"|PlistBuddy\s+-c\s+.?(Set|Add|Delete)|plutil\s+-(replace|insert|remove)")


def verify_no_shortcut(calls):
    """Reading the preference back is fine; writing it outside TextEdit is
    not the task."""
    for call in calls:
        text = json.dumps(call.get("input", {}))
        if WRITES.search(text):
            raise Problem(f"the setting was written outside TextEdit: {text[:160]}")


def reopened_setting():
    """Quits TextEdit, relaunches it and reads the Settings window."""
    from pilot import TEXTEDIT, ax, textedit_running, textedit_windows

    ax("quit", TEXTEDIT)
    for _ in range(50):
        if not textedit_running():
            break
        time.sleep(0.1)
    subprocess.run(["open", "-g", "-b", TEXTEDIT], check=True)
    deadline = time.time() + 15
    while time.time() < deadline and not (textedit_running() and textedit_windows()):
        time.sleep(0.2)
    for w in textedit_windows():
        if w["title"] == "Open":
            ax("close", TEXTEDIT, str(w["id"]))
    if ax("menu", TEXTEDIT, "TextEdit", "Settings…").returncode != 0:
        ax("menu", TEXTEDIT, "TextEdit", "Preferences…")
    value = None
    for _ in range(30):
        out = ax("value", TEXTEDIT, "AXRadioButton", "Plain text")
        if out.returncode == 0:
            value = out.stdout.strip()
            break
        time.sleep(0.2)
    for w in textedit_windows():
        if w["title"] in ("Settings", "Preferences", "New Document", "Open and Save"):
            ax("close", TEXTEDIT, str(w["id"]))
    if value != "1":
        raise Problem(f"after relaunching TextEdit its Settings show Plain text = {value!r}")
    return "TextEdit relaunched shows Plain text"


def main():
    from pilot import fail, ok, pref_snapshot, run_file, tool_calls

    saved = json.load(open(run_file("pref.json")))
    started = json.load(open(run_file("textedit.json")))
    out = subprocess.run(["defaults", "read", DOMAIN, KEY], capture_output=True, text=True)
    value = out.stdout.strip() if out.returncode == 0 else None
    try:
        verify_no_shortcut(tool_calls())
        said = verify_pref(value, saved["all"], pref_snapshot(DOMAIN))
        if started["was_running"]:
            reopened = "not relaunched: TextEdit was already running before the run"
        else:
            reopened = reopened_setting()
    except Problem as e:
        fail(str(e))
    ok(f"{said}; {reopened}")


if __name__ == "__main__":
    main()
