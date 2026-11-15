"""Per-operation latency and ref behaviour of vercel-labs agent-browser on
the browser tool's test fixture, for docs/release/EVIDENCE-browser.md.

    npm install agent-browser@0.41.0 --prefix <dir>
    AGENT_BROWSER_BIN=<dir>/node_modules/agent-browser/bin/agent-browser-<os>-<arch> \
        python3 -I bench/agent_browser.py [REPS]

It runs headless with a temporary HOME and profile, so it never touches the
user's own agent-browser configuration or Chrome profile.
"""
import json, math, os, re, subprocess, sys, tempfile, time
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRATCH = tempfile.mkdtemp(prefix="hame-ab-bench-")
AB = os.environ["AGENT_BROWSER_BIN"]
CHROME = os.environ.get("CHROME", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
REPS = int(sys.argv[1]) if len(sys.argv) > 1 else 12
home = tempfile.mkdtemp(dir=SCRATCH, prefix="abhome-")
profile = tempfile.mkdtemp(dir=SCRATCH, prefix="abprof-")
work = tempfile.mkdtemp(dir=SCRATCH, prefix="abwork-")
env = dict(os.environ, HOME=home, AGENT_BROWSER_EXECUTABLE_PATH=CHROME,
           AGENT_BROWSER_SESSION="hc", AGENT_BROWSER_SOCKET_DIR=tempfile.mkdtemp(prefix="ab"),
           AGENT_BROWSER_PROFILE=profile, AGENT_BROWSER_IDLE_TIMEOUT="5m")
fx = subprocess.Popen([sys.executable, "-I", ROOT + "/tests/fixture/app.py"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
port = int(fx.stdout.readline().split()[1])
base = f"http://127.0.0.1:{port}"


def ab(*args, ok=True):
    t = time.perf_counter()
    r = subprocess.run([AB, *args], env=env, capture_output=True, text=True, timeout=120, cwd=work)
    ms = (time.perf_counter() - t) * 1000
    out = (r.stdout + r.stderr).strip()
    if ok and r.returncode != 0:
        raise SystemExit(f"{args}: {out}")
    return ms, r.returncode, out


def ref(text, head):
    for line in text.splitlines():
        if line.strip().startswith(head):
            return "@" + re.search(r"ref=(e\d+)", line).group(1)
    raise SystemExit(f"no {head} in {text}")


def stats(name, ms):
    ms = sorted(ms)
    med = ms[len(ms) // 2]
    p95 = ms[min(len(ms), math.ceil(len(ms) * 0.95)) - 1]
    print(f"{name:10} n={len(ms):2} median={med:7.1f} ms  p95={p95:7.1f} ms  raw={[round(x, 1) for x in ms]}")
    return {"op": name, "n": len(ms), "median_ms": round(med, 1), "p95_ms": round(p95, 1), "raw_ms": [round(x, 1) for x in ms]}


t0 = time.perf_counter()
ab("open", base + "/form")
print(f"first open (daemon + Chrome start): {(time.perf_counter() - t0) * 1000:.0f} ms")
nav, snap, fill, click, dl = [], [], [], [], []
for i in range(REPS):
    nav.append(ab("open", base + "/form")[0])
    ms, _, text = ab("snapshot", "-i")
    snap.append(ms)
    name = ref(text, '- textbox "Name"')
    express = ref(text, '- checkbox "Express delivery"')
    fill.append(ab("fill", name, f"Ana {i}")[0])
    click.append(ab("click", express)[0])
ab("open", base + "/export")
_, _, text = ab("snapshot", "-i")
# The snapshot shows the date inputs only as unlabeled spinbuttons, so
# they are filled by CSS selector.
ab("fill", "input[name=from]", "2026-01-10")
ab("fill", "input[name=to]", "2026-01-20")
button = ref(text, '- button "Export CSV"')
for i in range(min(REPS, 10)):
    dl.append(ab("download", button, os.path.join(work, f"{i}.csv"))[0])
report = [stats("open", nav), stats("snapshot", snap), stats("fill", fill), stats("click", click), stats("download", dl)]
content = open(os.path.join(work, "0.csv")).read()
print("download content lines:", len(content.splitlines()))

# Stale-ref behaviour: snapshot, re-render the list, click an old ref.
ab("open", base + "/mutating")
_, _, text = ab("snapshot", "-i")
edits = [("@" + m) for m in re.findall(r'button "Edit" \[ref=(e\d+)\]', text)]
shuffle = ref(text, '- button "Shuffle"')
ab("click", shuffle)
ms, code, out = ab("click", edits[0], ok=False)
_, _, status = ab("get", "text", "#status")
print("stale click:", code, repr(out), "status after:", repr(status))
# In-place change: rename the first Edit, then click its old ref.
_, _, text = ab("snapshot", "-i")
first = ("@" + re.findall(r'button "Edit" \[ref=(e\d+)\]', text)[0])
ab("click", ref(text, '- button "Rename first"'))
ms, code, out2 = ab("click", first, ok=False)
_, _, status2 = ab("get", "text", "#status")
print("changed-element click:", code, repr(out2), "status after:", repr(status2))
# Navigation: old ref after reload.
_, _, text = ab("snapshot", "-i")
first = ("@" + re.findall(r'button "(?:Edit|Modify)" \[ref=(e\d+)\]', text)[0])
ab("open", base + "/mutating")
ms, code, out3 = ab("click", first, ok=False)
_, _, status3 = ab("get", "text", "#status")
print("after-navigation click:", code, repr(out3), "status after:", repr(status3))
json.dump({"report": report, "stale": [out, status, out2, status2, out3, status3]},
          open(os.path.join(SCRATCH, "ab-latency.json"), "w"))
ab("close", "--all")
fx.stdin.close()
