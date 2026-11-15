"""Readers for the files the office and cross-application tasks produce,
shared by their hidden checkers and the checkers' own tests. Never copied
into a workspace.

    xlsx_sheets(path)        {sheet: {"A1": Cell(value, formula)}} from openpyxl
    number(value)            a Decimal from a number or a plainly written numeric text
    formula_refs(formula)    normalized cell references a formula uses
    docx_paragraphs(path)    [(text, [(bold, italic, size) per character], alignment)]
    pdf_spans(path)          [(text, font, size, bold)] per text span, from PyMuPDF
    rtf_paragraphs(path)     [(text, {bold, italic, size, align})] through textutil's HTML
    render(path, size)       a PNG of the document's first page, through Quick Look
    ink_bands(png)           horizontal bands of dark pixels: (top, bottom, left, right)
    xlsx_chart(path)         the first chart's kind, title, axis titles, categories, values
"""

import html
import os
import re
import subprocess
import tempfile
import zipfile
import xml.etree.ElementTree as ET
from collections import namedtuple
from decimal import Decimal

Cell = namedtuple("Cell", "value formula")


class Problem(Exception):
    """A checker's finding: the run did not do what the task asked."""


def only_sheet_with(sheets, header):
    """The cells of the one sheet whose first row starts with `header`."""
    found = [cells for cells in sheets.values()
             if [cells.get(f"{col}1", Cell(None, None)).value for col in "ABCDEFG"[:len(header)]] == header]
    if len(found) != 1:
        firsts = {name: [cells.get(f"{c}1", Cell(None, None)).value for c in "ABCD"]
                  for name, cells in sheets.items()}
        raise Problem(f"expected one sheet headed {header}; first rows are {firsts}")
    return found[0]


def rows_of(cells, columns, first=2):
    """The values of `columns` in each row from `first` down to the last
    row that has anything in it."""
    last = max([int(re.sub(r"[A-Z]+", "", a)) for a in cells] + [first - 1])
    return [[cells[f"{c}{r}"].value if f"{c}{r}" in cells else None for c in columns]
            for r in range(first, last + 1)]


def xlsx_sheets(path):
    """Every non-empty cell of every sheet: its cached value and, for a
    formula cell, the formula text without the leading '='."""
    import openpyxl
    from openpyxl.worksheet.formula import ArrayFormula

    formulas = openpyxl.load_workbook(path, data_only=False)
    values = openpyxl.load_workbook(path, data_only=True)
    out = {}
    for ws in formulas.worksheets:
        cached = values[ws.title]
        cells = {}
        for row in ws.iter_rows():
            for c in row:
                raw = c.value
                formula = None
                if isinstance(raw, ArrayFormula):
                    formula = raw.text or ""
                elif isinstance(raw, str) and raw.startswith("="):
                    formula = raw
                if formula is not None:
                    formula = formula.lstrip("=")
                value = cached[c.coordinate].value
                if formula is None and raw is not None:
                    value = raw
                if value is None and formula is None:
                    continue
                cells[c.coordinate] = Cell(value, formula)
        out[ws.title] = cells
    return out


def number(value):
    """A Decimal for a number, or for text that is plainly a number
    (digits with at most one '.' or ',' and one or two decimals); None for
    anything else, so text a locale misread is not taken as a value."""
    if isinstance(value, bool) or value is None:
        return None
    if isinstance(value, (int, float)):
        return Decimal(str(value))
    text = str(value).strip()
    if re.fullmatch(r"-?\d+", text):
        return Decimal(text)
    m = re.fullmatch(r"(-?\d+)[.,](\d{1,2})", text)
    if m:
        return Decimal(f"{m.group(1)}.{m.group(2)}")
    return None


def formula_refs(formula):
    """The cell references and ranges in a formula, normalized (no '$',
    upper case, sheet and table prefixes dropped)."""
    text = (formula or "").upper().replace("$", "")
    text = re.sub(r"'[^']*'!|[A-Z0-9 _]+!|[A-Z0-9 _]+::", "", text)
    return re.findall(r"[A-Z]{1,3}\d+(?::[A-Z]{1,3}\d+)?", text)


W = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"


def _on(el, name):
    e = el.find(W + name) if el is not None else None
    if e is None:
        return None
    return e.get(W + "val", "true") not in ("0", "false", "off")


def docx_paragraphs(path):
    """Each body paragraph: its text, the effective (bold, italic, size in
    points) of every character, and its alignment. Pages writes bold and
    italic as font names ("Cambria Bold"); Word as <w:b/> and <w:i/>; both
    count."""
    root = ET.fromstring(zipfile.ZipFile(path).read("word/document.xml"))
    out = []
    for p in root.iter(W + "p"):
        ppr = p.find(W + "pPr")
        jc = ppr.find(W + "jc") if ppr is not None else None
        align = jc.get(W + "val") if jc is not None else "left"
        text, marks = "", []
        for r in p.iter(W + "r"):
            rpr = r.find(W + "rPr")
            fonts = rpr.find(W + "rFonts") if rpr is not None else None
            face = (fonts.get(W + "ascii") or "") if fonts is not None else ""
            bold = bool(_on(rpr, "b")) or "Bold" in face
            italic = bool(_on(rpr, "i")) or "Italic" in face
            sz = rpr.find(W + "sz") if rpr is not None else None
            size = int(sz.get(W + "val")) / 2 if sz is not None else None
            for t in r.iter(W + "t"):
                s = t.text or ""
                text += s
                marks += [(bold, italic, size)] * len(s)
        out.append((text, marks, align))
    return out


def pdf_spans(path):
    """Every text span of a PDF: text, font name, size, bold."""
    import fitz

    spans = []
    with fitz.open(path) as doc:
        for page in doc:
            for block in page.get_text("dict")["blocks"]:
                for line in block.get("lines", []):
                    for s in line["spans"]:
                        if s["text"].strip():
                            bold = bool(s["flags"] & 16) or "Bold" in s["font"]
                            spans.append((s["text"], s["font"], round(s["size"], 1), bold))
    return spans


def pdf_info(path):
    import fitz

    with fitz.open(path) as doc:
        page = doc[0]
        return {"pages": doc.page_count, "metadata": dict(doc.metadata or {}),
                "width": page.rect.width, "height": page.rect.height,
                "text": "".join(p.get_text() for p in doc)}


def rtf_paragraphs(path):
    """Each paragraph of an RTF (or any textutil-readable) document with
    its formatting, read from textutil's HTML: alignment and size from the
    paragraph's and spans' CSS, bold and italic from <b>/<i> or the font."""
    out = subprocess.run(["textutil", "-convert", "html", "-stdout", path],
                         capture_output=True, text=True, check=False)
    if out.returncode != 0:
        raise ValueError(f"textutil could not read {path}: {out.stderr.strip()}")
    doc = out.stdout
    css = {}
    for sel, body in re.findall(r"([\w.]+)\s*\{([^}]*)\}", doc):
        css[sel] = body
    paragraphs = []
    for cls, inner in re.findall(r'<p class="(\w+)">(.*?)</p>', doc, flags=re.S):
        pcss = css.get(f"p.{cls}", "")
        align = re.search(r"text-align:\s*(\w+)", pcss)
        text = html.unescape(re.sub(r"<[^>]+>", "", inner)).replace("\xa0", " ")
        sizes = [float(x) for x in re.findall(r"font:[^;]*?([\d.]+)px", pcss)]
        bolds, italics = [], []
        for scls in re.findall(r'<span class="(\w+)"', inner):
            scss = css.get(f"span.{scls}", "")
            sizes += [float(x) for x in re.findall(r"font:[^;]*?([\d.]+)px", scss)]
            bolds.append("bold" in scss.lower())
            italics.append("italic" in scss.lower())
        font = re.search(r"font:([^;]*)", pcss)
        face = font.group(1) if font else ""
        bold = "<b>" in inner or "Bold" in face or (bolds and all(bolds))
        italic = "<i>" in inner or "Italic" in face or "Oblique" in face or (italics and all(italics))
        if not text.strip():
            continue
        paragraphs.append((text.strip(), {"bold": bool(bold), "italic": bool(italic),
                                          "size": max(sizes) if sizes else None,
                                          "align": align.group(1) if align else "left"}))
    return paragraphs


def render(path, size=1000):
    """A PNG rendering of the document's first page through Quick Look
    (the same renderer Finder previews use)."""
    out = tempfile.mkdtemp(prefix="pilot-render-")
    subprocess.run(["qlmanage", "-t", "-s", str(size), "-o", out, path],
                   capture_output=True, check=False, timeout=60)
    pngs = [f for f in os.listdir(out) if f.endswith(".png")]
    if not pngs:
        raise ValueError(f"Quick Look rendered nothing for {path}")
    return os.path.join(out, pngs[0])


def ink_bands(png, threshold=110, min_rows=2):
    """Horizontal bands of dark pixels (text lines), top to bottom, each
    (top, bottom, left, right) in pixels, and the image width."""
    from PIL import Image

    img = Image.open(png).convert("L")
    w, h = img.size
    px = img.load()
    rows = []
    for y in range(h):
        xs = [x for x in range(0, w) if px[x, y] < threshold]
        rows.append((min(xs), max(xs)) if xs else None)
    bands, start = [], None
    for y, r in enumerate(rows + [None]):
        if r is not None and start is None:
            start = y
        elif r is None and start is not None:
            if y - start >= min_rows:
                seg = [rows[i] for i in range(start, y)]
                bands.append((start, y - 1, min(a for a, _ in seg), max(b for _, b in seg)))
            start = None
    return bands, w


C = "{http://schemas.openxmlformats.org/drawingml/2006/chart}"
A = "{http://schemas.openxmlformats.org/drawingml/2006/main}"


def _rich_text(el):
    if el is None:
        return ""
    return "".join(t.text or "" for t in el.iter(A + "t")) or "".join(
        v.text or "" for v in el.iter(C + "v"))


def _points(el):
    if el is None:
        return []
    pts = sorted(el.iter(C + "pt"), key=lambda p: int(p.get("idx", "0")))
    return [(p.find(C + "v").text or "") for p in pts if p.find(C + "v") is not None]


def xlsx_chart(path):
    """The first chart in an xlsx: kind (barChart...), bar direction, title,
    axis titles, and each series' categories and values."""
    z = zipfile.ZipFile(path)
    names = sorted(n for n in z.namelist() if re.match(r"xl/charts/chart\d+\.xml$", n))
    if not names:
        return None
    root = ET.fromstring(z.read(names[0]))
    chart = root.find(C + "chart")
    title = _rich_text(chart.find(C + "title"))
    plot = chart.find(C + "plotArea")
    kinds = [el for el in plot if el.tag.endswith("Chart")]
    kind = kinds[0].tag.split("}")[1] if kinds else ""
    bar_dir = kinds[0].find(C + "barDir").get("val") if kinds and kinds[0].find(C + "barDir") is not None else None
    series = []
    for ser in plot.iter(C + "ser"):
        series.append({"name": _rich_text(ser.find(C + "tx")),
                       "categories": _points(ser.find(C + "cat")),
                       "values": _points(ser.find(C + "val"))})
    axes = {}
    for ax in plot:
        if ax.tag in (C + "valAx", C + "catAx", C + "dateAx"):
            axes[ax.tag.split("}")[1]] = _rich_text(ax.find(C + "title"))
    return {"kind": kind, "bar_dir": bar_dir, "title": title, "axes": axes, "series": series}
