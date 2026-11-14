"""Settings. Everything is a constant; there is no configuration file."""
import os

DATA_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))), "data")

# Width of the description column in text reports.
DESCRIPTION_WIDTH = 22

# Width of the amount column in text reports.
AMOUNT_WIDTH = 10

# Upper bounds, in days, of the invoice aging buckets.
AGING_BUCKETS = (30, 60, 90)
