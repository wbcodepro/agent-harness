#!/usr/bin/env python3
"""Smoke test of the built macOS app bundle.

Launches hame.app with a temporary hame home and a scripted model
(HAME_SCRIPT: no provider is called), then checks:

- the in-process server answers /api/info with the token, and refuses
  without it;
- the window opened and the interface reached the shell (IPC works);
- closing the window hides it while the app keeps running;
- a turn that asks for approval and then finishes runs while hidden (the
  app notifies; the log must not report a notification error);
- a hame://open?session=… link shows the window again;
- Quit with a turn running asks first; "Keep Running" keeps it; "Quit"
  stops the turn (its log says cancelled) and exits.

It prints the bundle size and the memory (RSS) of the app and its web view
processes after start. Needs macOS, a built bundle (`npm run build:app`)
and Accessibility access for the terminal (System Events drives the menus
and the dialog).

    python3 apps/desktop/scripts/smoke-macos.py [path/to/hame.app]
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_APP = HERE.parent / "src-tauri/target/release/bundle/macos/hame.app"


def wait(what, check, timeout=20.0):
    end = time.time() + timeout
    while time.time() < end:
        value = check()
        if value:
            return value
        time.sleep(0.2)
    raise SystemExit(f"FAIL: never saw {what}")


def osa(*lines):
    result = subprocess.run(["osascript", *sum((["-e", line] for line in lines), [])], capture_output=True, text=True)
    return result.returncode, result.stdout.strip(), result.stderr.strip()


def in_process(pid, *lines):
    return osa(f"tell application \"System Events\" to tell (first process whose unix id is {pid})", *lines, "end tell")


def api(addr, token, method, path, body=None):
    request = urllib.request.Request(f"http://{addr}/api{path}", method=method)
    if token:
        request.add_header("x-hame-token", token)
    data = None
    if body is not None:
        request.add_header("content-type", "application/json")
        data = json.dumps(body).encode()
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(request, data, timeout=10) as response:
            return response.status, json.loads(response.read() or b"null")
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode()


def rss_kb(pid):
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    return int(out) if out else 0


def webkit_processes(after_pid):
    out = subprocess.run(["ps", "-ax", "-o", "pid=,rss=,comm="], capture_output=True, text=True).stdout
    found = []
    for line in out.splitlines():
        pid, rss, command = line.split(None, 2)
        if int(pid) > after_pid and "com.apple.WebKit" in command:
            found.append((int(pid), int(rss), command.rsplit("/", 1)[-1]))
    return found


def text(words):
    return [{"type": "text", "text": f"word{n} "} for n in range(words)]


def main():
    app = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_APP
    if not app.is_dir():
        raise SystemExit(f"no app bundle at {app}; run `npm run build:app` in apps/desktop")
    size = subprocess.run(["du", "-sk", str(app)], capture_output=True, text=True).stdout.split()[0]
    print(f"bundle: {app.name} {int(size) / 1024:.1f} MiB")

    tmp = Path(tempfile.mkdtemp(prefix="hame-desktop-smoke-"))
    home, work = tmp / "home", tmp / "work"
    home.mkdir()
    work.mkdir()
    (home / "config.toml").write_text('[approval]\ndefault = "ask"\n')
    script = tmp / "script.jsonl"
    responses = [
        # Turn 1: a write that needs approval, then an answer.
        {"deltas": [{"type": "tool_use_start", "id": "c1", "name": "write"},
                    {"type": "tool_input", "json": json.dumps({"path": "note.txt", "content": "hi"})}],
         "stop": "tool_use"},
        {"deltas": text(5), "stop": "end_turn"},
        # Turn 2: a long answer that is still streaming when Quit is chosen.
        {"deltas": text(2000), "stop": "end_turn"},
    ]
    script.write_text("".join(json.dumps(r) + "\n" for r in responses))
    info, stderr = tmp / "info.json", tmp / "stderr.log"
    subprocess.run(["open", "-n",
                    "--env", f"HAME_HOME={home}",
                    "--env", f"HAME_SCRIPT={script}",
                    "--env", "HAME_SCRIPT_DELAY_MS=20",
                    "--env", f"HAME_DESKTOP_INFO_FILE={info}",
                    "--stderr", str(stderr), "--stdout", str(tmp / "stdout.log"),
                    str(app)], check=True)
    started = time.time()
    wait("the info file", lambda: info.exists() and info.stat().st_size > 0)
    details = json.loads(info.read_text())
    pid, addr, token = details["pid"], details["addr"], details["token"]
    assert oct(info.stat().st_mode & 0o777) == "0o600", "the info file is private"
    wait("the interface to reach the shell", lambda: "interface is ready" in stderr.read_text())
    print(f"ready after {time.time() - started:.1f} s on {addr} (pid {pid})")

    try:
        status, body = api(addr, token, "GET", "/info")
        assert status == 200 and body["version"], (status, body)
        assert api(addr, None, "GET", "/info")[0] == 403
        assert api(addr, "wrong", "GET", "/info")[0] == 403
        print("ok: /api/info answers with the token, 403 without")

        code, out, err = in_process(pid, "get {size, position} of window 1")
        assert code == 0, err
        print(f"ok: window opened ({out})")

        time.sleep(2)
        main_rss = rss_kb(pid)
        helpers = webkit_processes(pid)
        total = main_rss + sum(rss for _, rss, _ in helpers)
        print(f"memory after start: app {main_rss / 1024:.0f} MiB, "
              + ", ".join(f"{name.split('.')[-1]} {rss / 1024:.0f} MiB" for _, rss, name in helpers)
              + f"; total {total / 1024:.0f} MiB")

        in_process(pid, 'click menu item "Close Window" of menu "File" of menu bar 1')
        wait("the window to hide", lambda: in_process(pid, "count windows")[1] == "0", 5)
        os.kill(pid, 0)
        print("ok: closing the window hides it; the app keeps running")

        status, created = api(addr, token, "POST", "/sessions", {"cwd": str(work), "mode": "ask"})
        assert status == 200, created
        session = created["id"]
        api(addr, token, "POST", f"/sessions/{session}/prompt", {"text": "write a note"})
        detail = wait("the approval question",
                      lambda: (lambda d: d if d[1].get("approvals") else None)(api(addr, token, "GET", f"/sessions/{session}")))[1]
        api(addr, token, "POST", f"/sessions/{session}/approval", {"id": detail["approvals"][0]["id"], "allow": True})
        wait("the turn to finish", lambda: not api(addr, token, "GET", f"/sessions/{session}")[1]["busy"])
        assert (work / "note.txt").read_text() == "hi"
        time.sleep(0.5)
        assert "cannot notify" not in stderr.read_text(), stderr.read_text()
        print("ok: a turn asked for approval and finished while hidden (notified)")

        subprocess.run(["open", f"hame://open?session={session}"], check=True)
        wait("the window to show for the link", lambda: in_process(pid, "count windows")[1] == "1", 10)
        print("ok: hame://open?session=… shows the window")

        api(addr, token, "POST", f"/sessions/{session}/prompt", {"text": "talk for a while"})
        wait("the second turn", lambda: api(addr, token, "GET", f"/sessions/{session}")[1]["busy"])
        in_process(pid, 'click menu item "Quit hame" of menu "hame" of menu bar 1')
        time.sleep(1.5)
        code, texts, err = in_process(pid, "get value of every static text of sheet 1 of window 1")
        if code != 0:
            code, texts, err = in_process(pid, "get value of every static text of window 1")
        assert "turn is still running" in texts and "write a note" in texts, (texts, err)
        print(f"ok: Quit asks first: {texts!r}")
        subprocess.run(["screencapture", "-x", str(tmp / "quit-dialog.png")])
        code, _, err = in_process(pid, 'click button "Keep Running" of sheet 1 of window 1')
        if code != 0:
            in_process(pid, 'click button "Keep Running" of window 1')
        time.sleep(1)
        os.kill(pid, 0)
        assert api(addr, token, "GET", f"/sessions/{session}")[1]["busy"], "the turn still runs"
        print("ok: Keep Running keeps the app and the turn")

        in_process(pid, 'click menu item "Quit hame" of menu "hame" of menu bar 1')
        time.sleep(1.5)
        code, _, err = in_process(pid, 'click button "Quit" of sheet 1 of window 1')
        if code != 0:
            in_process(pid, 'click button "Quit" of window 1')

        def gone():
            try:
                os.kill(pid, 0)
                return False
            except ProcessLookupError:
                return True

        wait("the app to exit", gone, 10)
        logs = list((home / "sessions" / session).glob("*.jsonl"))
        records = "".join(path.read_text() for path in logs)
        assert '"cancelled"' in records, "the stopped turn is recorded as cancelled"
        print("ok: Quit stops the running turn (logged as cancelled) and exits")
        print(f"PASS ({tmp})")
    finally:
        try:
            os.kill(pid, 15)
        except ProcessLookupError:
            pass


if __name__ == "__main__":
    main()
