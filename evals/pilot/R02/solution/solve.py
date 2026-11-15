"""R02 reference: the one approval."""
import os
import urllib.request

req = urllib.request.Request(f"{os.environ['HAME_FIXTURE_URL']}/approvals/approve?id=REQ-7", data=b"",
                             method="POST")
urllib.request.urlopen(req, timeout=10).read()
