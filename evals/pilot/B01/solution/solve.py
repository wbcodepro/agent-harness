"""B01 reference: the order the checker expects, posted directly."""
import os
import urllib.parse
import urllib.request

data = urllib.parse.urlencode({
    "ship_name": "Lan Nguyen", "ship_city": "Hanoi", "bill_name": "Acme GmbH",
    "bill_city": "Berlin", "country": "de", "express": "yes", "note": "Leave at the front desk",
}).encode()
urllib.request.urlopen(f"{os.environ['HAME_FIXTURE_URL']}/submit", data=data, timeout=10).read()
