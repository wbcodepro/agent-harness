"""Plain-text layout helpers."""


def pad(text, width):
    """Left-aligned in `width` columns; longer text is cut."""
    text = str(text)
    if len(text) > width:
        return text[:width]
    return text + " " * (width - len(text))


def right(text, width):
    """Right-aligned in `width` columns."""
    text = str(text)
    return " " * max(0, width - len(text)) + text


def rule(width, char="-"):
    return char * width


def lines(rows):
    """Joins rows with newlines; no trailing newline."""
    return "\n".join(rows)
