"""R01: the sign-up completed once, with every field, despite the modal
that opened in the middle of filling the form."""
import os
import sys

sys.path.insert(0, os.path.join(os.environ["HAME_SUITE_DIR"], "fixture"))
from pilot import fail, ok, state  # noqa: E402

EXPECTED = {"full_name": "Tran Van Khoa", "email": "khoa.tran@example.org",
            "phone": "+84 912 345 678", "team": "Logistics", "shirt": "M"}
signups = state()["signups"]
if not signups:
    fail("no sign-up was submitted")
if len(signups) > 1:
    fail(f"{len(signups)} sign-ups were submitted")
got = {k: v.strip() for k, v in signups[0].items()}
wrong = {k: (got.get(k), v) for k, v in EXPECTED.items() if got.get(k) != v}
if wrong:
    fail(f"wrong fields (got, expected): {wrong}")
ok("one complete sign-up")
