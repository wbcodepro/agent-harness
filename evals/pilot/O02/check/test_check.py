"""O02 checker, offline: synthetic workbooks with and without formulas."""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check  # noqa: E402
from office import Problem  # noqa: E402


def workbook(line=lambda r: f"=B{r}*C{r}", total="=SUM(D2:D6)", paper=6, cached=True):
    """A workbook as Numbers exports it; with `cached`, formula values are
    written as Numbers caches them (openpyxl cannot calculate, so the test
    writes the cached values into a values-only copy)."""
    import openpyxl

    items = [(n, paper if n == "Paper" else q, p) for n, q, p in check.ITEMS]
    path = os.path.join(tempfile.mkdtemp(), "order-final.xlsx")
    for data_only in (False, True):
        wb = openpyxl.Workbook()
        ws = wb.active
        ws.title = "Order"
        ws.append(check.HEADER)
        for r, (n, q, p) in enumerate(items, start=2):
            f = line(r)
            ws.append([n, q, p, (q * p if (data_only and cached and f and str(f).startswith("=")) else f)])
        ws.append(["Total", None, None,
                   (sum(q * p for _, q, p in items) if data_only and cached and str(total).startswith("=") else total)])
        if not data_only:
            wb.save(path)
        else:
            values = wb
    # Merge: formulas from the first, cached values injected into the XML.
    _inject_cached(path, values)
    return path


def _inject_cached(path, values):
    """Writes the values-only workbook's numbers as <v> of the formula cells."""
    import re
    import shutil
    import zipfile

    ws = values.active
    cached = {c.coordinate: c.value for row in ws.iter_rows() for c in row if c.value is not None}
    tmp = path + ".tmp"
    with zipfile.ZipFile(path) as src, zipfile.ZipFile(tmp, "w") as dst:
        for item in src.infolist():
            data = src.read(item.filename)
            if item.filename == "xl/worksheets/sheet1.xml":
                xml = data.decode()

                def fill(m):
                    ref, body = m.group(1), m.group(3)
                    if "<f>" in body and ref in cached and isinstance(cached[ref], (int, float)):
                        body = re.sub(r"<v>.*?</v>|<v/>", "", body) + f"<v>{cached[ref]}</v>"
                    return f'<c r="{ref}"{m.group(2)}>{body}</c>'
                xml = re.sub(r'<c r="([A-Z]+\d+)"([^>]*)>(.*?)</c>', fill, xml)
                data = xml.encode()
            dst.writestr(item, data)
    shutil.move(tmp, path)


class O02(unittest.TestCase):
    def test_formulas_with_recalculated_values_pass(self):
        self.assertIn("total 1098000", check.verify_workbook(workbook()))
        check.verify_workbook(workbook(total="=D2+D3+D4+D5+D6"))
        check.verify_workbook(workbook(line=lambda r: f"=C{r}*B{r}"))
        check.verify_live({"D7": "1098000, Formula result"})
        check.verify_live({"D7": "1.098.000, Formula result"})

    def test_typed_values_wrong_refs_and_stale_totals_fail(self):
        cases = [
            workbook(line=lambda r: None, total=None),  # untouched
            workbook(line=lambda r: f"=B{r}*C{r}", total=None),
            workbook(line=lambda r: "=B2*C2"),  # every row points at row 2
            workbook(total="=SUM(D2:D5)"),
            workbook(paper=4),
            workbook(cached=False),
        ]
        for path in cases:
            with self.assertRaises(Problem):
                check.verify_workbook(path)
        with self.assertRaises(Problem):
            check.verify_live({"D7": "978000, Formula result"})


if __name__ == "__main__":
    unittest.main()
