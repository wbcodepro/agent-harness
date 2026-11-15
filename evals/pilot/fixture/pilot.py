"""Shared helpers for the pilot's live-fixture tasks: setup, fault
injection, checkers and teardown. Never copied into a workspace.

As a library (checkers add this directory to sys.path):
    state()            the fixture's recorded state (needs $HAME_FIXTURE_TOKEN)
    plain_text(path)   a document's text through textutil
    textedit_windows() TextEdit's windows, read through accessibility
    tool_calls()       the agent's tool calls, from its session logs
    ok(msg), fail(msg) end a checker

As a command (`python3 pilot.py <command>`), run by `hame eval`:
    profile-set <profile> <key> <value>  seed a browser profile's local storage
    textedit-start                launch TextEdit if needed, remember what was open
    textedit-new <title-file>      open a new untitled document (for decoys)
    textedit-cleanup               close windows the run opened, quit TextEdit if the run started it
    cancel-sheet-once <button>     background: cancel the first sheet that offers <button>
    kill-agent-when <expr>         background: SIGKILL the agent once <expr> holds on the state
    kill-browser-when <expr>       background: kill the agent's browser helper once <expr> holds
    raise-decoy-on-focus           background: raise the decoy window when the target gets focus
"""

import glob
import hashlib
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
TEXTEDIT = "com.apple.TextEdit"


def env(name):
    value = os.environ.get(name)
    if not value:
        fail(f"${name} is not set; run this through hame eval")
    return value


def ok(message="ok"):
    print(message)
    sys.exit(0)


def fail(message):
    print(f"FAIL: {message}")
    sys.exit(1)


def state():
    url = f"{env('HAME_FIXTURE_URL')}/__state?token={env('HAME_FIXTURE_TOKEN')}"
    with urllib.request.urlopen(url, timeout=10) as response:
        return json.loads(response.read().decode("utf-8"))


def plain_text(path):
    out = subprocess.run(["textutil", "-convert", "txt", "-stdout", path],
                         capture_output=True, text=True, check=False)
    if out.returncode != 0:
        fail(f"textutil could not read {path}: {out.stderr.strip()}")
    return out.stdout


def axtool():
    """The compiled accessibility tool, built once per source version."""
    source = os.path.join(HERE, "axtool.swift")
    digest = hashlib.sha256(open(source, "rb").read()).hexdigest()[:12]
    binary = os.path.join(tempfile.gettempdir(), f"hame-pilot-axtool-{digest}")
    if not os.path.exists(binary):
        tmp = f"{binary}.{os.getpid()}"
        subprocess.run(["swiftc", "-O", source, "-o", tmp], check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        os.replace(tmp, binary)
    return binary


def ax(*args, check=False):
    return subprocess.run([axtool(), *args], capture_output=True, text=True, check=check)


def textedit_running():
    return subprocess.run(["pgrep", "-x", "TextEdit"], capture_output=True).returncode == 0


def textedit_windows():
    out = ax("windows", TEXTEDIT)
    return [json.loads(line) for line in out.stdout.splitlines() if line.strip()]


def tool_calls():
    """Every tool call in the agent's session logs, in order."""
    calls = []
    home = env("HAME_AGENT_HOME")
    for log in sorted(glob.glob(os.path.join(home, "sessions", "*", "*.jsonl"))):
        if os.path.basename(log).startswith(("actions", "trace")):
            continue
        for line in open(log, encoding="utf-8"):
            try:
                record = json.loads(line)
            except ValueError:
                continue
            event = record.get("event", record)
            if event.get("type") == "tool/call":
                calls.append({"name": event.get("name"), "input": event.get("input") or {}})
    return calls


def tool_results():
    """Every tool result's text in the agent's session logs."""
    out = []
    home = env("HAME_AGENT_HOME")
    for log in sorted(glob.glob(os.path.join(home, "sessions", "*", "*.jsonl"))):
        if os.path.basename(log).startswith(("actions", "trace")):
            continue
        for line in open(log, encoding="utf-8"):
            try:
                record = json.loads(line)
            except ValueError:
                continue
            event = record.get("event", record)
            if event.get("type") == "tool/result":
                text = "".join(b.get("text", "") for b in event.get("content", []) if isinstance(b, dict))
                out.append(text)
    return out


def receipts():
    """Every action receipt the agent's adapters journalled."""
    out = []
    home = env("HAME_AGENT_HOME")
    for journal in glob.glob(os.path.join(home, "sessions", "*", "actions.v1.jsonl")):
        for line in open(journal, encoding="utf-8"):
            try:
                out.append(json.loads(line))
            except ValueError:
                pass
    return out


def _profile_page(profile, action):
    """Opens the fixture's /prefs page in the agent's browser profile
    `profile` (under $HAME_AGENT_HOME/browser-profiles) with a browser of
    its own, runs `action(page)`, and closes it cleanly."""
    from playwright.sync_api import sync_playwright

    directory = os.path.join(env("HAME_AGENT_HOME"), "browser-profiles", profile)
    os.makedirs(directory, exist_ok=True)
    deadline = time.time() + 20
    with sync_playwright() as pw:
        while True:
            try:
                ctx = pw.chromium.launch_persistent_context(
                    directory, channel="chrome", headless=True,
                    args=["--no-first-run", "--no-default-browser-check", "--use-mock-keychain",
                          "--password-store=basic"])
                break
            except Exception:  # noqa: BLE001 - a closing browser may hold the profile
                if time.time() > deadline:
                    raise
                time.sleep(0.5)
        try:
            page = ctx.pages[0] if ctx.pages else ctx.new_page()
            page.goto(f"{env('HAME_FIXTURE_URL')}/prefs")
            return action(page)
        finally:
            ctx.close()


def profile_get(profile, key):
    return _profile_page(profile, lambda page: page.evaluate("k => localStorage.getItem(k)", key))


def profile_set(profile, key, value):
    _profile_page(profile, lambda page: page.evaluate("([k, v]) => localStorage.setItem(k, v)",
                                                      [key, value]))


def run_file(name):
    return os.path.join(env("HAME_RUN_DIR"), name)


def eval_state(expr):
    """Evaluates `expr` with the fixture state's keys as names."""
    try:
        return bool(eval(expr, {"len": len}, state()))  # noqa: S307 - the manifest's own expression
    except Exception:  # noqa: BLE001 - the fixture may not answer yet
        return False


# Commands -----------------------------------------------------------------

def textedit_start():
    was = textedit_running()
    before = [w["id"] for w in textedit_windows()] if was else []
    json.dump({"was_running": was, "windows": before}, open(run_file("textedit.json"), "w"))
    if not was:
        subprocess.run(["open", "-g", "-b", TEXTEDIT], check=True)
        deadline = time.time() + 15
        while time.time() < deadline:
            if textedit_running() and textedit_windows():
                break
            time.sleep(0.2)
    # A fresh launch opens its Open panel; it is not part of the task.
    for w in textedit_windows():
        if w["title"] == "Open" and w["id"] not in before:
            ax("close", TEXTEDIT, str(w["id"]))


def textedit_new(title_file):
    """A new untitled document, its window id written to `title_file`."""
    before = {w["id"] for w in textedit_windows()}
    subprocess.run(["open", "-g", "-a", "TextEdit", "-F", "--new"], check=False)
    deadline = time.time() + 10
    while time.time() < deadline:
        new = [w for w in textedit_windows() if w["id"] not in before and w["title"] != "Open"]
        if new:
            open(run_file(title_file), "w").write(str(new[0]["id"]))
            return
        time.sleep(0.2)
    sys.exit("no new TextEdit window")


def textedit_cleanup():
    try:
        saved = json.load(open(run_file("textedit.json")))
    except (OSError, ValueError):
        return
    if not textedit_running():
        return
    keep = set(saved["windows"])
    for w in textedit_windows():
        if w["id"] not in keep:
            ax("close", TEXTEDIT, str(w["id"]))
    if not saved["was_running"]:
        ax("quit", TEXTEDIT)


def cancel_sheet_once(buttons):
    """Waits for a TextEdit sheet offering one of `buttons` (comma-separated,
    e.g. Save,Move), lets it sit for a moment, presses its Cancel, and
    records that it did."""
    wanted = set(buttons.split(","))
    deadline = time.time() + 3600
    while time.time() < deadline:
        for w in textedit_windows():
            if w["sheet"] and wanted & set(w["sheet_buttons"]):
                time.sleep(1.0)
                if ax("press-sheet", TEXTEDIT, "Cancel").returncode == 0:
                    open(run_file("sheet.cancelled"), "w").write(json.dumps(w))
                    return
        time.sleep(0.3)


def kill_agent_when(expr):
    pid_file = run_file("agent.pid")
    while True:
        if eval_state(expr) and os.path.exists(pid_file):
            pid = int(open(pid_file).read().strip())
            open(run_file("agent.killed"), "w").write(str(time.time()))
            os.kill(pid, signal.SIGKILL)
            return
        time.sleep(0.2)


def kill_browser_when(expr):
    """Kills the agent's browser helper (and so its browser), as a crashed
    adapter would, the first time `expr` holds."""
    home = env("HAME_AGENT_HOME")
    while True:
        if eval_state(expr):
            out = subprocess.run(["pgrep", "-f", "browser_helper"], capture_output=True, text=True)
            killed = []
            for pid in out.stdout.split():
                cmd = subprocess.run(["ps", "-o", "command=", "-p", pid], capture_output=True,
                                     text=True).stdout
                if home in cmd:
                    os.kill(int(pid), signal.SIGKILL)
                    killed.append(pid)
            open(run_file("browser.killed"), "w").write(json.dumps(killed))
            return
        time.sleep(0.2)


def raise_decoy_on_focus():
    """R05: when the target document becomes TextEdit's focused window while
    TextEdit is frontmost (the moment before typing), raise the decoy, as a
    person clicking another window would. Twice at most."""
    decoy = open(run_file("decoy.id")).read().strip()
    fired = 0
    while fired < 2:
        front = ax("frontmost").stdout.strip()
        windows = textedit_windows()
        focused = [w for w in windows if w["focused"]]
        if front == TEXTEDIT and focused and str(focused[0]["id"]) != decoy \
                and focused[0]["title"] != "Open" and not focused[0]["sheet"]:
            time.sleep(0.4)
            ax("raise", TEXTEDIT, decoy)
            fired += 1
            with open(run_file("decoy.raised"), "a") as log:
                log.write(f"{time.time()} raised decoy over {focused[0]['id']}\n")
            time.sleep(3)
        time.sleep(0.15)


COMMANDS = {
    "profile-set": profile_set,
    "textedit-start": textedit_start,
    "textedit-new": textedit_new,
    "textedit-cleanup": textedit_cleanup,
    "cancel-sheet-once": cancel_sheet_once,
    "kill-agent-when": kill_agent_when,
    "kill-browser-when": kill_browser_when,
    "raise-decoy-on-focus": raise_decoy_on_focus,
}

if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] not in COMMANDS:
        sys.exit(f"usage: pilot.py {{{'|'.join(COMMANDS)}}} ...")
    COMMANDS[sys.argv[1]](*sys.argv[2:])
