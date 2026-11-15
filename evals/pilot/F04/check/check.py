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
import json

for path, digest in {
    "data/sales-north.json": "1b6ba5e6bd594c5cd9b2cc5295cf551259e1feb18b2372f672e97011cea67b9b", "data/sales-north-2.json": "73dadfed18def52b7ce0e776e8ef89df2d320ba7a18139271de4c74f456d5a08", "data/sales-south.json": "22ffdcdf5cdc1ea07ff74ed32341d95d9abfc93f2432dbe867b203ba46259de1", "data/sales-east.json": "5cbdf5ba4354f03c8bc79a1437751fec17f3f41bd16139112dca9b0a465c7862", "data/notes.txt": "51000f22c4e59ef758883538a253c4833d0d23f96dcdaa3ca7a1f170f91a3ae8"
}.items():
    unchanged(path, digest)

expected = {'regions': [{'region': 'east', 'units': 0, 'revenue': '0.00', 'sources': ['data/sales-east.json']}, {'region': 'north', 'units': 6, 'revenue': '162.49', 'sources': ['data/sales-north-2.json', 'data/sales-north.json']}, {'region': 'south', 'units': 11, 'revenue': '23.00', 'sources': ['data/sales-south.json']}], 'totals': {'units': 17, 'revenue': '185.49'}, 'sources': ['data/sales-east.json', 'data/sales-north-2.json', 'data/sales-north.json', 'data/sales-south.json']}
try:
    with open("report.json", encoding="utf-8") as f:
        report = json.load(f)
except Exception as e:  # noqa: BLE001
    fail(f"report.json cannot be read: {e!r}")
    finish()
if not isinstance(report, dict):
    fail("report.json is not an object")
    finish()
for key in ("regions", "totals", "sources"):
    if key not in report:
        fail(f"report.json has no {key}")
regions = report.get("regions")
if isinstance(regions, list):
    names = [r.get("region") for r in regions if isinstance(r, dict)]
    if names != [r["region"] for r in expected["regions"]]:
        fail(f"regions are {names!r}")
    for want in expected["regions"]:
        got = next((r for r in regions if isinstance(r, dict) and r.get("region") == want["region"]), None)
        if got is None:
            continue
        for field in ("units", "revenue", "sources"):
            if got.get(field) != want[field]:
                fail(f"{want['region']}.{field} is {got.get(field)!r}, expected {want[field]!r}")
else:
    fail("regions is not a list")
if report.get("totals") != expected["totals"]:
    fail(f"totals are {report.get('totals')!r}")
if report.get("sources") != expected["sources"]:
    fail(f"sources are {report.get('sources')!r}")

finish()
