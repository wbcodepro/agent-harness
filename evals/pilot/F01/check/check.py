"""Hidden checker. Runs with the workspace as the current directory."""
import hashlib
import os
import sys

problems = []


def fail(message):
    problems.append(message)


def sha256(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def unchanged(path, digest):
    if not os.path.isfile(path):
        fail(f"{path} is missing")
    elif sha256(path) != digest:
        fail(f"{path} was changed")


def finish():
    if problems:
        for problem in problems:
            print("FAIL:", problem)
        sys.exit(1)
    print("PASS")
    sys.exit(0)
import math
import re
import subprocess

sys.dont_write_bytecode = True
sys.path.insert(0, os.getcwd())

for path, digest in {
    "README.md": "307f1bb12e8de2b475a26b0c667869066f8becd977c724bc9a5e3459ac24b411", "geometry/units.py": "98622ac5b58312d4ea7c601746a57c24fb6feb8101bad1a1403db7928cf9c340", "data/shapes.json": "b45a5e44e369bdd9eeb479cbbc234dcdece7a2b43dc798b1dcf906d513a2e966"
}.items():
    unchanged(path, digest)

old = re.compile(r"\bcalc_area\b")
for folder, _, names in os.walk("."):
    if ".git" in folder:
        continue
    for name in names:
        if name.endswith(".py"):
            path = os.path.join(folder, name)
            with open(path, encoding="utf-8") as f:
                if old.search(f.read()):
                    fail(f"{path} still mentions calc_area")

try:
    import geometry
    from geometry import compute_area, calc_area_total
    from geometry.area import compute_area as direct

    shapes = [
        {"kind": "rect", "w": 2, "h": 5},
        {"kind": "circle", "r": 1},
        {"kind": "triangle", "base": 3, "height": 4},
    ]
    if compute_area is not direct:
        fail("geometry.compute_area is not geometry.area.compute_area")
    if compute_area(shapes[0]) != 10 or abs(compute_area(shapes[1]) - math.pi) > 1e-9:
        fail("compute_area gives wrong areas")
    if abs(calc_area_total(shapes) - (10 + math.pi + 6)) > 1e-9:
        fail("calc_area_total no longer works")
    if "compute_area" not in getattr(geometry, "__all__", []):
        fail("geometry.__all__ does not export compute_area")
    try:
        compute_area({"kind": "hexagon"})
        fail("an unknown shape no longer raises ValueError")
    except ValueError:
        pass
except Exception as e:  # noqa: BLE001
    fail(f"the renamed function cannot be used: {e!r}")

def run(*command):
    return subprocess.run(
        [sys.executable, "-B", *command], capture_output=True, text=True, timeout=60
    )

report = run("report.py")
if report.stdout != "lawn: 90.00\npond: 12.57\nbed: 6.00\ntotal: 108.57\n":
    fail(f"report.py output changed: {report.stdout!r} {report.stderr[-200:]!r}")
largest = run("tools/largest.py")
if largest.stdout.strip() != "lawn":
    fail(f"tools/largest.py is broken: {largest.stderr[-200:]!r}")
tests = run("-m", "unittest", "-q")
if tests.returncode != 0:
    fail(f"the project's tests fail: {tests.stderr[-300:]!r}")

finish()
