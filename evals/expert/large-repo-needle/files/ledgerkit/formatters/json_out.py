"""JSON output."""
import json


def render(rows):
    return json.dumps([list(row) for row in rows], sort_keys=True)
