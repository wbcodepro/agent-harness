"""Prints the area of every shape in data/shapes.json."""
import json
from pathlib import Path

from geometry import calc_area, calc_area_total


def main():
    shapes = json.loads((Path(__file__).parent / "data" / "shapes.json").read_text())
    for shape in shapes:
        print(f"{shape['name']}: {calc_area(shape):.2f}")
    print(f"total: {calc_area_total(shapes):.2f}")


if __name__ == "__main__":
    main()
