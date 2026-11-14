"""Identifier formats."""
import re

from ledgerkit.core.errors import BadInput

_PATTERNS = {
    "customer": re.compile(r"^C-\d{3}$"),
    "invoice": re.compile(r"^INV-\d{4}$"),
    "employee": re.compile(r"^E-\d{2}$"),
    "room": re.compile(r"^R\d{2}$"),
}


def is_valid(kind, text):
    pattern = _PATTERNS.get(kind)
    return bool(pattern and pattern.match(text or ""))


def require(kind, text):
    if not is_valid(kind, text):
        raise BadInput("not a %s id: %r" % (kind, text))
    return text
