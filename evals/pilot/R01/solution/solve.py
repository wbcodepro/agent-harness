"""R01 reference: the sign-up the checker expects, posted directly."""
import os
import urllib.parse
import urllib.request

data = urllib.parse.urlencode({"full_name": "Tran Van Khoa", "email": "khoa.tran@example.org",
                               "phone": "+84 912 345 678", "team": "Logistics", "shirt": "M"}).encode()
urllib.request.urlopen(f"{os.environ['HAME_FIXTURE_URL']}/signup", data=data, timeout=10).read()
