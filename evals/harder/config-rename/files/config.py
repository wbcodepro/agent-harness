import json


def load(path=None):
    """Defaults from defaults.json, overridden by `key = value` lines in path."""
    settings = json.load(open("defaults.json"))
    if path:
        for line in open(path):
            line = line.strip()
            if not line or line.startswith(";"):
                continue
            key, value = (part.strip() for part in line.split("=", 1))
            settings[key] = int(value) if value.isdigit() else value
    return settings
