"""D04: the Vietnamese sentence persisted exactly (NFC-equal, every
diacritic), in the document TextEdit saved."""
import os
import sys
import unicodedata

sys.path.insert(0, os.path.join(os.environ["HAME_SUITE_DIR"], "fixture"))
from pilot import fail, ok, plain_text, strays  # noqa: E402

if strays():
    fail(f"saved outside the requested folder: {strays()}")

WANT = "Biên bản họp ngày 15/11: Đã thống nhất kế hoạch quý IV; người phụ trách: Nguyễn Thị Hằng."
folder = os.path.join(os.environ["HAME_OUTBOX"], "vi")
files = sorted(os.listdir(folder))
saved = [f for f in files if os.path.splitext(f)[0] == "bien-ban" and f.endswith((".rtf", ".txt"))]
if not saved:
    fail(f"no bien-ban.rtf or .txt in vi/ (folder holds {files})")
text = plain_text(os.path.join(folder, saved[0])).strip()
if unicodedata.normalize("NFC", text) != unicodedata.normalize("NFC", WANT):
    fail(f"{saved[0]} reads {text!r}")
form = "NFC" if text == WANT else "another normal form"
ok(f"{saved[0]} holds the sentence exactly ({form})")
