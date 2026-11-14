"""Shipments.

A shipment is a dict with "id", "sent" and "arrived" (ISO strings) and
"warehouse".
"""
from ledgerkit.core.dates import days_between, parse_date


def transit_days(shipment):
    """Days on the road: 0 when it arrived the day it was sent."""
    return days_between(parse_date(shipment["sent"]), parse_date(shipment["arrived"]))


def slowest(shipments):
    """The shipment with the longest transit; the first one on a tie."""
    best = None
    for shipment in shipments:
        if best is None or transit_days(shipment) > transit_days(best):
            best = shipment
    return best
