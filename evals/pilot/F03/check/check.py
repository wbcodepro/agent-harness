"""Hidden checker. Runs with the workspace as the current directory."""
import hashlib
import os
import sys

problems = []


def fail(message):
    problems.append(message)


def sha256(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def unchanged(path, digest):
    if not os.path.isfile(path):
        fail(f"{path} is missing")
    elif sha256(path) != digest:
        fail(f"{path} was changed")


def finish():
    if problems:
        for problem in problems:
            print("FAIL:", problem)
        sys.exit(1)
    print("PASS")
    sys.exit(0)
unchanged("RULES.md", "bef1ce0670c20628ff1f73be40b4dbdf0eb48bf0eb6259e9538def716bd78155")

expected = {
    "archive/duplicates/holiday.jpg": "2dd2b41028048b95c4b0755274d7832b970ebabc8348f42d2e5a9c8c0e68ef69",
    "archive/duplicates/invoice-2026-04-globex.pdf": "9e7ae339a841fa2b249b2e349ba4fe93344fb80199c36f902985cb1e8dc1a3a2",
    "archive/invoices/2026/03/invoice-2026-03-acme.pdf": "7c9ee0cbb9c5ef2f6ea4dd003d2ddbe769cc5d0831aee52d443c81466fc62fe5",
    "archive/invoices/2026/04/invoice-2026-04-globex-copy.pdf": "9e7ae339a841fa2b249b2e349ba4fe93344fb80199c36f902985cb1e8dc1a3a2",
    "archive/notes/Readme.TXT": "49ff8a159c339268d607076eae916603d1b587221696305542879da0561b60ad",
    "archive/notes/meeting-notes.txt": "7eefae204fc9a3f635c37539310b397f3139cb6fa28f1cee5373f571884b6b60",
    "archive/notes/todo.txt": "c08a755ff142ea0922552f6d2d492ba1337ba1869feb7b8e51816829560b65d5",
    "archive/photos/img_0001.jpg": "2dd2b41028048b95c4b0755274d7832b970ebabc8348f42d2e5a9c8c0e68ef69",
    "archive/photos/img_0002.jpeg": "9e43f0769e1ca7a22ba68a53a36d4b2e2d287a6891b475509925f790cc088f34",
    "inbox/invoice-draft.pdf": "06d28b1c3198e3f1b22224e0a8216e1a2b78e59b71574a7d886f02226b9ffd19",
    "inbox/report.docx": "8e494cb62d87477771f683d9c8a988931aafb2f7b3b361d239d0cb06d5e4fa48"
}
found = {}
for top in ("inbox", "archive"):
    for folder, _, names in os.walk(top):
        for name in names:
            path = os.path.join(folder, name).replace(os.sep, "/")
            found[path] = sha256(path)
for path, digest in expected.items():
    if path not in found:
        fail(f"{path} is missing")
    elif found[path] != digest:
        fail(f"{path} has the wrong contents")
for path in sorted(set(found) - set(expected)):
    fail(f"{path} should not be there")

finish()
