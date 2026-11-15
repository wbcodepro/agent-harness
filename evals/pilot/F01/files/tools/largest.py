"""Names the largest shape."""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from geometry.area import calc_area as area_of  # noqa: E402


def largest(shapes):
    return max(shapes, key=area_of)["name"]


if __name__ == "__main__":
    data = Path(__file__).resolve().parent.parent / "data" / "shapes.json"
    print(largest(json.loads(data.read_text())))
