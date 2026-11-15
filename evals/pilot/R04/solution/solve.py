"""R04 reference: retries until the page answers."""
import os
import re
import urllib.error
import urllib.request

url = f"{os.environ['HAME_FIXTURE_URL']}/shipments/S-42"
for _ in range(4):
    try:
        html = urllib.request.urlopen(url, timeout=40).read().decode()
    except urllib.error.HTTPError:
        continue
    open("status.txt", "w").write(re.search(r'<span id="status">([^<]+)</span>', html).group(1) + "\n")
    break
