"""Multi-currency ledger engine. See SPEC.md."""


class Ledger:
    def __init__(self, currencies):
        raise NotImplementedError

    def apply(self, op):
        """Process one operation and return its result dict."""
        raise NotImplementedError
