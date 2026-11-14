"""Reading and writing contacts files. The file format is in FORMAT.md."""

import json
import os

VERSION = 1


def load_file(path):
    """Return the list of records in one contacts file."""
    with open(path, encoding="utf-8") as handle:
        document = json.load(handle)
    if document.get("version") != VERSION:
        raise ValueError("%s: unsupported version" % path)
    return document["records"]


def load_dir(directory):
    """Return the records of every *.json file directly in `directory`.

    Files are read in sorted file name order; records keep their order
    within a file.
    """
    records = []
    for name in sorted(os.listdir(directory)):
        if name.endswith(".json"):
            records.extend(load_file(os.path.join(directory, name)))
    return records


def save_file(path, records):
    """Write `records` to `path` as a contacts file."""
    document = {"version": VERSION, "records": list(records)}
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(document, handle, indent=2, sort_keys=True)
        handle.write("\n")
