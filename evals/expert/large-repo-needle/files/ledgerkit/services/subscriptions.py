"""Subscriptions.

A subscription is a dict with "customer", "plan", "from" and "to" (ISO
strings, first and last day of service) and "monthly_cents".
"""
from ledgerkit.core import periods
from ledgerkit.core.dates import parse_date


def is_active(subscription, day):
    """True when the subscription is in service on that day."""
    return periods.contains(parse_date(subscription["from"]), parse_date(subscription["to"]), day)


def active_on(subscriptions, day):
    """The subscriptions in service on that day, in their original order."""
    return [subscription for subscription in subscriptions if is_active(subscription, day)]


def monthly_recurring_cents(subscriptions, day):
    return sum(subscription["monthly_cents"] for subscription in active_on(subscriptions, day))
