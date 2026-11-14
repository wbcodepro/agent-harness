"""Business calendar. See SPEC.md."""


def is_business_day(region, day):
    raise NotImplementedError


def holidays(region, year):
    raise NotImplementedError


def add_business_days(region, day, n):
    raise NotImplementedError


def business_days_between(region, start, end):
    raise NotImplementedError


def nth_business_day(region, year, month, n):
    raise NotImplementedError


def utc_offset(region, utc):
    raise NotImplementedError


def to_local(region, utc):
    raise NotImplementedError


def adjust(region, day, convention):
    raise NotImplementedError


def to_utc(region, local, prefer="earlier"):
    raise NotImplementedError


def settle(region, utc, days, also=None):
    raise NotImplementedError


def occurrences(region, rule, start, end):
    raise NotImplementedError
