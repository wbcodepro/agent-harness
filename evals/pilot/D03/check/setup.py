"""D03 setup: plain-text files with similar names in the outbox, which only
applications can reach. Their hashes are kept in the run directory."""
import hashlib
import json
import os

FILES = {
    "budget-2026.txt": "Budget 2026 (draft)\nTotal: 118,000 EUR\n",
    "budget-2026-final.txt": "Budget 2026 (final)\nTotal: 121,500 EUR\n",
    "budget-2026-final copy.txt": "Budget 2026 (final, old copy)\nTotal: 119,900 EUR\n",
    "budget-2026-finale.txt": "Budget 2026 (finale, rejected)\nTotal: 130,000 EUR\n",
    "budget-2025-final.txt": "Budget 2025 (final)\nTotal: 109,000 EUR\n",
}
folder = os.path.join(os.environ["HAME_OUTBOX"], "docs")
os.makedirs(folder, exist_ok=True)
hashes = {}
for name, text in FILES.items():
    path = os.path.join(folder, name)
    open(path, "w", encoding="utf-8").write(text)
    hashes[name] = hashlib.sha256(text.encode()).hexdigest()
json.dump(hashes, open(os.path.join(os.environ["HAME_RUN_DIR"], "d03.json"), "w"))
