"""Exceptions raised by ledgerkit."""


class LedgerError(Exception):
    """Base class of all ledgerkit errors."""


class UnknownRecord(LedgerError):
    """A customer, room, employee or report name that does not exist."""


class BadInput(LedgerError):
    """Input that cannot be understood."""
