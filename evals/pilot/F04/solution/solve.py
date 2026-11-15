"""Reference solution: compute report.json from data/."""
import glob
import json
from decimal import Decimal

regions = {}
sources = []
for path in sorted(glob.glob("data/*.json")):
    path = path.replace("\\", "/")
    sources.append(path)
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    entry = regions.setdefault(data["region"], {"units": 0, "revenue": Decimal(0), "sources": []})
    entry["sources"].append(path)
    for item in data["items"]:
        entry["units"] += item["qty"]
        entry["revenue"] += item["qty"] * Decimal(item["unit_price"])
report = {
    "regions": [
        {"region": name, "units": r["units"], "revenue": f"{r['revenue']:.2f}", "sources": sorted(r["sources"])}
        for name, r in sorted(regions.items())
    ],
    "totals": {
        "units": sum(r["units"] for r in regions.values()),
        "revenue": f"{sum(r['revenue'] for r in regions.values()):.2f}",
    },
    "sources": sorted(sources),
}
with open("report.json", "w", encoding="utf-8") as f:
    json.dump(report, f, indent=2)
