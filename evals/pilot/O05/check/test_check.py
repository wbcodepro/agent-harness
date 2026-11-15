"""O05 checker, offline: PDFs drawn with PyMuPDF, right and wrong."""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check  # noqa: E402
from office import Problem  # noqa: E402


def pdf(title_font="helvetica-bold", title_size=22, beds_font="helvetica-bold", size=(595.28, 841.89),
        pages=1, drop=None):
    import fitz

    doc = fitz.open()
    page = doc.new_page(width=size[0], height=size[1])
    y = 80
    lines = [("Riverside Community Garden", title_font, title_size),
             ("Open every day from 7:00 to 19:00. Members can rent one of ", "helvetica", 11),
             ("48 raised beds", beds_font, 11), (" for the growing season.", "helvetica", 11),
             ("Workshops", "helvetica-bold", 15),
             ("Composting basics on Saturday mornings, and seed saving.", "helvetica", 11),
             ("Contact", "helvetica-bold", 15),
             ("Write to garden@riverside.example or visit the shed.", "helvetica-oblique", 11)]
    for text, font, sz in lines:
        if drop and drop in text:
            continue
        page.insert_text((72, y), text, fontname={"helvetica": "helv", "helvetica-bold": "hebo",
                                                  "helvetica-oblique": "heit"}[font], fontsize=sz)
        y += sz * 2
    for _ in range(pages - 1):
        doc.new_page()
    path = os.path.join(tempfile.mkdtemp(), "brochure.pdf")
    doc.save(path)
    return path


class O05(unittest.TestCase):
    def test_a_right_pdf_reopened_in_preview_passes(self):
        path = pdf()
        self.assertIn("A4", check.verify_pdf(path))
        check.verify_reopened(path, [{"document": path}])

    def test_wrong_pdfs_and_no_reopen_fail(self):
        for path in (pdf(title_font="helvetica"), pdf(title_size=11), pdf(beds_font="helvetica"),
                     pdf(size=(612, 792)), pdf(pages=2), pdf(drop="Workshops")):
            with self.assertRaises(Problem, msg=path):
                check.verify_pdf(path)
        broken = os.path.join(tempfile.mkdtemp(), "brochure.pdf")
        with open(broken, "wb") as f:
            f.write(b"%PDF-1.4 not really")
        with self.assertRaises(Problem):
            check.verify_pdf(broken)
        with self.assertRaises(Problem):
            check.verify_pdf("/nonexistent.pdf")
        with self.assertRaises(Problem):
            check.verify_reopened(pdf(), [{"document": "/elsewhere/brochure.pdf"}])


if __name__ == "__main__":
    unittest.main()
