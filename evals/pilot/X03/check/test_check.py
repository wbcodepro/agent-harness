"""X03 checker, offline: the fixture flyer edited by text replacement, PDFs
drawn with PyMuPDF."""
import os
import shutil
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import check  # noqa: E402
from office import Problem  # noqa: E402

FLYER = os.path.join(HERE, "..", "files", "flyer.rtf")


def flyer(*edits):
    text = open(FLYER, encoding="latin-1").read()
    for old, new in edits:
        assert old in text, old
        text = text.replace(old, new)
    path = os.path.join(tempfile.mkdtemp(), "flyer.rtf")
    with open(path, "w", encoding="latin-1") as f:
        f.write(text)
    return path


def pdf(lines, title_font="hebo", title_size=22):
    import fitz

    doc = fitz.open()
    page = doc.new_page()
    y = 80
    for i, line in enumerate(lines):
        page.insert_text((72, y), line.replace("é", "e") if i else line,
                         fontname=title_font if i == 0 else "helv", fontsize=title_size if i == 0 else 13)
        y += 40
    path = os.path.join(tempfile.mkdtemp(), "flyer.pdf")
    doc.save(path)
    return path


EDITS = [("22 November", "29 November"), ("Room 4B", "Room 2A")]


class X03(unittest.TestCase):
    def test_edited_flyer_and_its_pdf_pass(self):
        self.assertIn("updated", check.verify_source(flyer(*EDITS)))
        self.assertIn("new date", check.verify_pdf(pdf(check.LINES)))

    def test_stale_or_unformatted_results_fail(self):
        untouched = os.path.join(tempfile.mkdtemp(), "flyer.rtf")
        shutil.copy(FLYER, untouched)
        for path in (untouched, flyer(EDITS[0]), flyer(*EDITS, ("\\f0\\b\\fs44", "\\f1\\fs44")),
                     flyer(*EDITS, ("\\f2\\i ", "\\f1 "))):
            with self.assertRaises(Problem, msg=path):
                check.verify_source(path)
        old = [check.LINES[0], "Saturday 22 November 2026, 10:00 to 13:00"] + check.LINES[2:]
        for path in (pdf(old), pdf(check.LINES, title_font="helv"), pdf(check.LINES, title_size=13)):
            with self.assertRaises(Problem):
                check.verify_pdf(path)
        with self.assertRaises(Problem):
            check.verify_pdf("/nonexistent/flyer.pdf")


if __name__ == "__main__":
    unittest.main()
