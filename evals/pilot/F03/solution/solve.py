"""Reference solution: apply RULES.md."""
import os
import re
import shutil

names = sorted(os.listdir("inbox"))
contents = {}
for name in names:
    with open(os.path.join("inbox", name), "rb") as f:
        contents.setdefault(f.read(), []).append(name)
moves = {}
for group in contents.values():
    group.sort()
    for duplicate in group[1:]:
        moves[duplicate] = os.path.join("archive", "duplicates", duplicate)
    name = group[0]
    lower = name.lower()
    invoice = re.match(r"invoice-(\d{4})-(\d{2})", name)
    if lower.endswith(".pdf") and invoice:
        moves[name] = os.path.join("archive", "invoices", invoice[1], invoice[2], name)
    elif lower.endswith((".jpg", ".jpeg")):
        moves[name] = os.path.join("archive", "photos", lower)
    elif lower.endswith(".txt"):
        moves[name] = os.path.join("archive", "notes", name)
for name, target in moves.items():
    os.makedirs(os.path.dirname(target), exist_ok=True)
    shutil.move(os.path.join("inbox", name), target)
