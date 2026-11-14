"""Comma-separated output."""


def cell(value):
    text = str(value)
    if "," in text or '"' in text:
        return '"' + text.replace('"', '""') + '"'
    return text


def render(rows):
    return "\n".join(",".join(cell(value) for value in row) for row in rows)
