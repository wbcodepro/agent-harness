"""Customer lookups."""
from ledgerkit.core.errors import UnknownRecord


def by_id(customers, customer_id):
    for customer in customers:
        if customer["id"] == customer_id:
            return customer
    raise UnknownRecord("no customer %r" % (customer_id,))


def display_name(customer):
    return "%s (%s)" % (customer["id"], customer["name"])


def in_region(customers, region):
    return [customer for customer in customers if customer.get("region") == region]
