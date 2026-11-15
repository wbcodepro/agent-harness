"""O04 checker, offline: a real Numbers export of the finished chart
(sample-export.xlsx), mutated copies, and drawn bar images."""
import os
import sys
import tempfile
import unittest
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import check  # noqa: E402
from office import Problem  # noqa: E402

SAMPLE = os.path.join(HERE, "sample-export.xlsx")


def mutated(old, new):
    path = os.path.join(tempfile.mkdtemp(), "revenue-chart.xlsx")
    with zipfile.ZipFile(SAMPLE) as src, zipfile.ZipFile(path, "w") as dst:
        for item in src.infolist():
            data = src.read(item.filename)
            if item.filename.startswith("xl/charts/chart"):
                xml = data.decode()
                assert old in xml, old
                data = xml.replace(old, new, 1).encode()
            dst.writestr(item, data)
    return path


def bars(heights, size=(500, 300)):
    from PIL import Image, ImageDraw

    img = Image.new("RGB", size, "white")
    d = ImageDraw.Draw(img)
    d.text((200, 5), check.TITLE, fill="black")
    d.rectangle((20, 10, 30, 20), fill=(80, 120, 190))  # a legend swatch
    for i, hgt in enumerate(heights):
        x = 60 + i * 70
        d.rectangle((x, 280 - hgt, x + 30, 280), fill=(80, 120, 190))
    path = os.path.join(tempfile.mkdtemp(), "chart.png")
    img.save(path)
    return path


class O04(unittest.TestCase):
    def test_the_real_export_and_a_matching_drawing_pass(self):
        self.assertIn("mapped", check.verify_chart(SAMPLE))
        heights = check.bar_heights(bars([v // 3 for v in check.REVENUE]), (0, 0, 500, 300))
        self.assertEqual(len(heights), 6)
        self.assertIn("6 bars", check.verify_bars(heights))

    def test_wrong_charts_fail(self):
        for old, new in [("Revenue, January to June 2026", "Title"),
                         ("Revenue (million VND)", "Value Axis"),
                         ("<c:v>Feb</c:v>", "<c:v>Untitled 2</c:v>"),
                         ("<c:v>389.000000</c:v>", "<c:v>398.000000</c:v>"),
                         ('<c:barDir val="col"/>', '<c:barDir val="bar"/>')]:
            with self.assertRaises(Problem, msg=old):
                check.verify_chart(mutated(old, new))
        with self.assertRaises(Problem):
            check.verify_chart("/nonexistent.xlsx")
        five = check.bar_heights(bars([v // 3 for v in check.REVENUE[:5]]), (0, 0, 500, 300))
        with self.assertRaises(Problem):
            check.verify_bars(five)
        shuffled = check.bar_heights(bars([v // 3 for v in reversed(check.REVENUE)]), (0, 0, 500, 300))
        with self.assertRaises(Problem):
            check.verify_bars(shuffled)


if __name__ == "__main__":
    unittest.main()
