"""R03 reference: one payment, then its confirmation from the history."""
import os
import re
import urllib.parse
import urllib.request

base = os.environ["HAME_FIXTURE_URL"]
data = urllib.parse.urlencode({"recipient": "Hoang Anh", "amount": "250.00",
                               "reference": "INV-2026-117"}).encode()
urllib.request.urlopen(f"{base}/transfer", data=data, timeout=30).read()
history = urllib.request.urlopen(f"{base}/transfer/history", timeout=10).read().decode()
open("confirmation.txt", "w").write(re.search(r"PAY-\d+", history).group(0) + "\n")
