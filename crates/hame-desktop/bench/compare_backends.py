#!/usr/bin/env python3
"""Backend comparison for ADR-5: the hame desktop helper vs vercel-labs
agent-desktop, on the same TextEdit steps.

    python3 compare_backends.py <hame-desktop-helper> <agent-desktop>

The helper is the compiled DesktopHelper.swift (for example from
target/tmp/hame-desktop-helpers/ after the GUI tests ran, or
`swiftc -O -swift-version 5 -o h helper/DesktopHelper.swift`). It creates
and closes its own TextEdit documents and quits TextEdit afterwards if it
was not running. agent-desktop keeps state in ~/.agent-desktop; delete it
afterwards if it did not exist before.
"""
import json
import statistics
import subprocess
import sys
import time

p = subprocess.Popen([sys.argv[1]], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
n = 0


def call(op, **kw):
    global n
    n += 1
    kw.update(id=n, op=op)
    p.stdin.write(json.dumps(kw) + "\n")
    p.stdin.flush()
    return json.loads(p.stdout.readline())


call("hello", protocol=1)

A = sys.argv[2]
VI = "Tiếng Việt có dấu: ắ ằ ẳ ẵ ặ đ Đ ơ ư"
VI2 = "Đ ơ ư ắ ằ ẳ ẵ ặ — lần hai"
REPS = 12
results = {}


def ad(*args):
    t0 = time.perf_counter()
    out = subprocess.run([A, *args], capture_output=True, text=True)
    dt = (time.perf_counter() - t0) * 1000
    try:
        return json.loads(out.stdout), dt
    except Exception:
        return {"raw": out.stdout + out.stderr}, dt


def timed(fn):
    t0 = time.perf_counter()
    r = fn()
    return r, (time.perf_counter() - t0) * 1000


def stats(name, xs):
    xs = sorted(xs)
    p95 = xs[min(len(xs) - 1, int(round(0.95 * (len(xs) - 1))))]
    results[name] = (statistics.median(xs), p95, len(xs))
    print(f"{name}: median {statistics.median(xs):.1f} ms, p95 {p95:.1f} ms, n={len(xs)}")


was_running = subprocess.run(["pgrep", "-x", "TextEdit"], capture_output=True).returncode == 0
if not was_running:
    subprocess.run(["open", "-g", "-b", "com.apple.TextEdit"])
deadline = time.time() + 10
while time.time() < deadline:
    w = call("windows", app="com.apple.TextEdit")
    if w.get("ok") and w["result"]["windows"]:
        break
    time.sleep(0.2)
before = {x["window_id"] for x in w["result"]["windows"]}
openp = [x for x in w["result"]["windows"] if x["title"] == "Open"]
if openp:
    call("press", app="com.apple.TextEdit", window_id=openp[0]["window_id"], name="New Document", role="AXButton")
else:
    call("menu", app="com.apple.TextEdit", path=["File", "New"])
time.sleep(1)
w = call("windows", app="com.apple.TextEdit")
doc = [x for x in w["result"]["windows"] if x["window_id"] not in before and x["title"] != "Open"][0]
wid = doc["window_id"]
print("doc window", wid, doc["title"])
created = [wid]

try:
    # ---- hame helper ----
    obs_ms, set_ms, ok_set = [], [], 0
    for i in range(REPS):
        r, dt = timed(lambda: call("observe", app="com.apple.TextEdit", window_id=wid, max_nodes=150))
        obs_ms.append(dt)
    obs = r["result"]["observation"]
    ta = [n for n in r["result"]["nodes"] if n["role"] == "AXTextArea"][0]["ref"]
    for i in range(REPS):
        v = VI if i % 2 == 0 else VI2
        r, dt = timed(lambda: call("set_value", observation=obs, element=ta, value=v))
        set_ms.append(dt)
        back = call("resolve", observation=obs, element=ta)["result"].get("value")
        ok_set += back == v
    stats("hame observe", obs_ms)
    stats("hame set_value", set_ms)
    print("hame set_value exact readback:", ok_set, "/", REPS)

    # ---- agent-desktop ----
    ver_ms = [ad("version")[1] for _ in range(REPS)]
    stats("agent-desktop process start (version)", ver_ms)
    lw, _ = ad("list-windows", "--app", "TextEdit")
    print("agent-desktop list-windows:", json.dumps(lw, ensure_ascii=False)[:600])
    wins = lw.get("data") or []
    adwid = [x for x in wins if isinstance(x, dict) and x.get("title") == doc["title"]][0]["id"]
    snap_ms = []
    snap = None
    for i in range(REPS):
        snap, dt = ad("snapshot", "--app", "TextEdit", "--window-id", str(adwid), "-i")
        snap_ms.append(dt)
    if not snap.get("ok"):
        print("agent-desktop snapshot failed:", json.dumps(snap)[:800])
    stats("agent-desktop snapshot -i", snap_ms)

    def find(node, role):
        if isinstance(node, dict):
            if node.get("role") in role and node.get("ref_id"):
                return node["ref_id"]
            for c in node.get("children", []) or []:
                f = find(c, role)
                if f:
                    return f
        return None

    tree = snap.get("data", {}).get("tree")
    ref = find(tree, ("textarea", "textbox", "text_area", "textfield", "document"))
    print("agent-desktop text ref:", ref, "ref_count", snap.get("data", {}).get("ref_count"))
    if ref is None:
        print(json.dumps(snap, ensure_ascii=False)[:4000])
        raise SystemExit(1)
    sv_ms, ok_sv, errs = [], 0, []
    for i in range(REPS):
        v = VI if i % 2 == 0 else VI2
        r, dt = ad("set-value", ref, v)
        sv_ms.append(dt)
        if not r.get("ok"):
            errs.append(json.dumps(r)[:300])
        back = call("observe", app="com.apple.TextEdit", window_id=wid)["result"]["nodes"]
        val = [n for n in back if n["role"] == "AXTextArea"][0].get("value")
        ok_sv += val == v
    stats("agent-desktop set-value", sv_ms)
    print("agent-desktop set-value exact readback:", ok_sv, "/", REPS, errs[:2])

    # Unicode typing (semantic insert) with agent-desktop.
    ad("set-value", ref, "")
    r, dt = ad("type", ref, VI)
    back = call("observe", app="com.apple.TextEdit", window_id=wid)["result"]["nodes"]
    val = [n for n in back if n["role"] == "AXTextArea"][0].get("value")
    print("agent-desktop type:", r.get("ok"), f"{dt:.0f} ms", "exact" if val == VI else f"MISMATCH {val!r}", json.dumps(r)[:300] if not r.get("ok") else "")

    # hame typing for comparison (physical, per chunk confirmation).
    call("set_value", observation=obs, element=ta, value="")
    call("focus", app="com.apple.TextEdit", window_id=wid)
    r, dt = timed(lambda: call("type", observation=obs, element=ta, text=VI))
    back = call("resolve", observation=obs, element=ta)["result"].get("value")
    print("hame type:", r.get("ok"), f"{dt:.0f} ms", "exact" if back == VI else f"MISMATCH {back!r}")

    # Stale refs: snapshot, close the window, act on the old ref.
    call("set_value", observation=obs, element=ta, value="")
    w2 = call("windows", app="com.apple.TextEdit")
    call("menu", app="com.apple.TextEdit", path=["File", "New"])
    time.sleep(0.8)
    w3 = call("windows", app="com.apple.TextEdit")
    known = {x["window_id"] for x in w2["result"]["windows"]}
    wid2 = [x for x in w3["result"]["windows"] if x["window_id"] not in known][0]["window_id"]
    created.append(wid2)
    snap2, _ = ad("snapshot", "--app", "TextEdit", "--window-id", f"w-{wid2}", "-i")
    ref2 = find(snap2.get("data", {}).get("tree"), ("textarea", "textbox", "text_area", "textfield", "document"))
    o2 = call("observe", app="com.apple.TextEdit", window_id=wid2)["result"]
    ta2 = [n for n in o2["nodes"] if n["role"] == "AXTextArea"][0]["ref"]
    call("press", app="com.apple.TextEdit", window_id=wid2, name="", role="AXCloseButton")
    time.sleep(0.6)
    r, _ = ad("set-value", ref2, "stale?")
    print("agent-desktop stale window:", json.dumps(r.get("error", r))[:300])
    r = call("set_value", observation=o2["observation"], element=ta2, value="stale?")
    print("hame stale window:", r.get("error", {}).get("code"), r.get("error", {}).get("message", "")[:120])
finally:
    for w_id in created:
        for _ in range(4):
            w = call("windows", app="com.apple.TextEdit")
            hit = [x for x in w["result"]["windows"] if x["window_id"] == w_id]
            if not hit:
                break
            if hit[0]["sheets"]:
                call("press", app="com.apple.TextEdit", window_id=w_id, name="Delete", role="AXButton")
            else:
                call("press", app="com.apple.TextEdit", window_id=w_id, name="", role="AXCloseButton")
            time.sleep(0.5)
    if not was_running:
        call("menu", app="com.apple.TextEdit", path=["TextEdit", "Quit TextEdit"])
    print(json.dumps({k: [round(v[0], 1), round(v[1], 1), v[2]] for k, v in results.items()}))
