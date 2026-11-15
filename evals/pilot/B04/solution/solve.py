"""B04 reference: the export endpoint with the February range."""
import os
import urllib.request

url = f"{os.environ['HAME_FIXTURE_URL']}/export.csv?from=2026-02-01&to=2026-02-28"
data = urllib.request.urlopen(url, timeout=30).read()
os.makedirs("exports", exist_ok=True)
open("exports/ledger-2026-02.csv", "wb").write(data)
