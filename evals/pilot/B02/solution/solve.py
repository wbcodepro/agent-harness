"""B02 reference: reads every page of the filtered table."""
import csv
import os
import re
import urllib.request

base = os.environ["HAME_FIXTURE_URL"]
rows, page = [], 1
while True:
    html = urllib.request.urlopen(f"{base}/table?category=tool&page={page}", timeout=10).read().decode()
    found = re.findall(r"<tr><td>(\d+)</td><td>([^<]+)</td><td>tool</td><td>([^<]+)</td></tr>", html)
    rows += found
    if f"Page {page} of {page}" in html or not found:
        break
    page += 1
with open("tools.csv", "w", newline="") as f:
    w = csv.writer(f)
    w.writerow(["id", "name", "price"])
    w.writerows(sorted(rows, key=lambda r: int(r[0])))
