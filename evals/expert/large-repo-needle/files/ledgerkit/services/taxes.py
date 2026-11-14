"""Sales tax."""
from ledgerkit.core.money import percent_of

RATES = {"north": 8, "south": 6, "east": 7, "west": 9}


def rate_for(region):
    """The tax rate of a region in percent; 0 for an unknown region."""
    return RATES.get(region, 0)


def tax_cents(amount_cents, region):
    if amount_cents <= 0:
        return 0
    return percent_of(amount_cents, rate_for(region))
