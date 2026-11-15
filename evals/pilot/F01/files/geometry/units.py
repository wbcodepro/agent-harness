FACTORS = {"m2": 1.0, "cm2": 0.0001, "km2": 1_000_000.0}


def to_square_meters(value, unit):
    return value * FACTORS[unit]
