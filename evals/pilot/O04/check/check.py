"""O04: a column chart of the six months, read two ways.
Data mapping: the chart Numbers exported (xlsx chart XML) has one series
whose categories are Jan..Jun and whose values are the table's revenue,
the requested title and the value axis title with its unit.
Visual: the chart still open in Numbers is captured from the screen and
its bars are counted; their heights must follow the revenue figures."""
import json
import os
import subprocess
import sys
import tempfile
from decimal import Decimal, InvalidOperation

sys.path.insert(0, os.path.join(os.environ.get("HAME_SUITE_DIR", os.path.dirname(__file__) + "/../.."), "fixture"))
from office import Problem, xlsx_chart  # noqa: E402

MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun"]
REVENUE = [412, 389, 455, 501, 478, 530]
TITLE = "Revenue, January to June 2026"
AXIS = "Revenue (million VND)"


def chart_number(text):
    """Chart caches write numbers as '412.000000'."""
    try:
        return Decimal(text)
    except (InvalidOperation, TypeError):
        return None


def verify_chart(path):
    if not os.path.isfile(path):
        raise Problem(f"{os.path.basename(path)} was not exported")
    chart = xlsx_chart(path)
    if chart is None:
        raise Problem("the exported workbook has no chart")
    if chart["kind"] != "barChart" or chart["bar_dir"] != "col":
        raise Problem(f"the chart is a {chart['kind']} ({chart['bar_dir']}), not a column chart")
    if len(chart["series"]) != 1:
        raise Problem(f"the chart has {len(chart['series'])} series, expected one (revenue): {chart['series']}")
    s = chart["series"][0]
    if s["categories"] != MONTHS:
        raise Problem(f"the categories are {s['categories']}, expected {MONTHS}")
    if [chart_number(v) for v in s["values"]] != REVENUE:
        raise Problem(f"the values are {s['values']}, expected {REVENUE}")
    if chart["title"].strip() != TITLE:
        raise Problem(f"the chart title is {chart['title']!r}, expected {TITLE!r}")
    if chart["axes"].get("valAx", "").strip() != AXIS:
        raise Problem(f"the value axis title is {chart['axes'].get('valAx')!r}, expected {AXIS!r}")
    return "column chart, Jan..Jun mapped to revenue, title and axis label with unit"


def bar_heights(png, crop):
    """Heights in pixels of the coloured bars in `crop` (left, top, right,
    bottom pixels) of `png`, left to right. A bar is a run of columns in
    which strongly coloured pixels cover at least a tenth of the crop."""
    from PIL import Image

    img = Image.open(png).convert("RGB").crop(crop)
    hsv = img.convert("HSV")
    w, h = img.size
    px = hsv.load()
    coverage = []
    for x in range(w):
        coverage.append(sum(1 for y in range(h) if px[x, y][1] > 90 and 50 < px[x, y][2] < 245))
    runs, start = [], None
    for x, c in enumerate(coverage + [0]):
        if c >= h / 10 and start is None:
            start = x
        elif c < h / 10 and start is not None:
            if x - start >= 3:
                runs.append(max(coverage[start:x]))
            start = None
    return runs


def verify_bars(heights):
    if len(heights) != len(REVENUE):
        raise Problem(f"the chart on screen shows {len(heights)} bars, expected {len(REVENUE)}")
    n = len(heights)
    mx, my = sum(heights) / n, sum(REVENUE) / n
    cov = sum((a - mx) * (b - my) for a, b in zip(heights, REVENUE))
    var = (sum((a - mx) ** 2 for a in heights) * sum((b - my) ** 2 for b in REVENUE)) ** 0.5
    r = cov / var if var else 0
    if r < 0.98:
        raise Problem(f"bar heights {heights} do not follow the revenue {REVENUE} (r = {r:.3f})")
    return f"{n} bars on screen, heights follow the data (r = {r:.3f})"


def capture_chart():
    """Captures the open revenue document's window and returns the PNG and
    the chart's crop box in its pixels."""
    from pilot import NUMBERS, ax

    out = ax("frame", NUMBERS, "revenue", "Column Chart")
    if out.returncode != 0:
        raise Problem("no column chart is open in the revenue document")
    f = json.loads(out.stdout)
    png = os.path.join(tempfile.mkdtemp(), "window.png")
    subprocess.run(["screencapture", "-x", "-o", "-l", str(f["window_id"]), png], check=True)
    from PIL import Image

    scale = Image.open(png).size[0] / f["window"][2]
    wx, wy = f["window"][0], f["window"][1]
    ex, ey, ew, eh = f["element"]
    crop = tuple(int(v * scale) for v in (ex - wx, ey - wy, ex - wx + ew, ey - wy + eh))
    return png, crop


def main():
    from pilot import fail, ok, strays

    if strays():
        fail(f"saved outside the requested folder: {strays()}")
    folder = os.path.join(os.environ["HAME_OUTBOX"], "o04")
    try:
        mapped = verify_chart(os.path.join(folder, "revenue-chart.xlsx"))
        png, crop = capture_chart()
        drawn = verify_bars(bar_heights(png, crop))
    except Problem as e:
        fail(f"{e} (folder holds {sorted(os.listdir(folder))})")
    extra = sorted(set(os.listdir(folder)) - {"revenue.numbers", "revenue-chart.xlsx"})
    if extra:
        fail(f"other files in the folder: {extra}")
    ok(f"{mapped}; {drawn}")


if __name__ == "__main__":
    main()
