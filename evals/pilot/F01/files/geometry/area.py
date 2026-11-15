"""Areas of simple shapes."""
import math


def calc_area(shape):
    """Area of one shape described by a dict with a "kind" key."""
    kind = shape["kind"]
    if kind == "rect":
        return shape["w"] * shape["h"]
    if kind == "circle":
        return math.pi * shape["r"] ** 2
    if kind == "triangle":
        return 0.5 * shape["base"] * shape["height"]
    raise ValueError(f"unknown shape kind: {kind}")


def calc_area_total(shapes):
    """Sum of the areas of several shapes."""
    return sum(calc_area(shape) for shape in shapes)
