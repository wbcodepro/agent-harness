"""B03 reference: a multipart upload of the right file to R-102."""
import os
import urllib.request

data = open("invoices/invoice-2026-03.pdf", "rb").read()
b = "----pilotboundary"
body = (f"--{b}\r\nContent-Disposition: form-data; name=\"record\"\r\n\r\nR-102\r\n"
        f"--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"invoice-2026-03.pdf\"\r\n"
        f"Content-Type: application/pdf\r\n\r\n").encode() + data + f"\r\n--{b}--\r\n".encode()
req = urllib.request.Request(f"{os.environ['HAME_FIXTURE_URL']}/upload", data=body,
                             headers={"Content-Type": f"multipart/form-data; boundary={b}"})
urllib.request.urlopen(req, timeout=10).read()
