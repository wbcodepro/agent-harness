"""Loads the JSON files of the data directory."""
import json
import os

from ledgerkit.core import config
from ledgerkit.core.errors import UnknownRecord

_cache = {}


def load(name):
    """The list of records in data/<name>.json."""
    if name not in _cache:
        path = os.path.join(config.DATA_DIR, name + ".json")
        with open(path, encoding="utf-8") as handle:
            _cache[name] = json.load(handle)
    return _cache[name]


def find(name, key, value):
    """The first record of data/<name>.json whose `key` equals `value`."""
    for record in load(name):
        if record.get(key) == value:
            return record
    raise UnknownRecord("no %s with %s %r" % (name, key, value))
