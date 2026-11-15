"""O03 checker, offline: the reference export with edits applied to its XML,
right and wrong."""
import os
import shutil
import sys
import tempfile
import unittest
import zipfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check  # noqa: E402
from office import Problem  # noqa: E402


def edited(*replacements):
    """The reference docx with plain string replacements in its XML."""
    path = os.path.join(tempfile.mkdtemp(), "ops-report-q4.docx")
    with zipfile.ZipFile(check.REFERENCE) as src, zipfile.ZipFile(path, "w") as dst:
        for item in src.infolist():
            data = src.read(item.filename)
            if item.filename == "word/document.xml":
                xml = data.decode()
                for old, new in replacements:
                    assert old in xml, old
                    xml = xml.replace(old, new, 1)
                data = xml.encode()
            dst.writestr(item, data)
    return path


RIGHT = [("Q3 Operations", "Q4 Operations"), ("Headcount is 42.", "Headcount is 45."),
         ("96 percent", "97 percent")]


class O03(unittest.TestCase):
    def test_the_edits_with_formatting_kept_pass(self):
        path = edited(*RIGHT)
        self.assertIn("preserved", check.verify_docx(path))
        self.assertIn("rendered lines", check.verify_rendering(path))

    def test_missing_edits_lost_formatting_and_other_changes_fail(self):
        untouched = os.path.join(tempfile.mkdtemp(), "ops-report-q4.docx")
        shutil.copy(check.REFERENCE, untouched)
        lost_bold = edited(*RIGHT, ('<w:rFonts w:ascii="Cambria Bold" w:hAnsi="Cambria Bold"/><w:sz w:val="44"/>',
                                    '<w:sz w:val="44"/>'))
        lost_center = edited(*RIGHT, ('<w:jc w:val="center"/>', ""))
        extra_text = edited(*RIGHT, ("board meeting", "board session"))
        for path in (untouched, lost_bold, lost_center, extra_text):
            with self.assertRaises(Problem, msg=path):
                check.verify_docx(path)
        with self.assertRaises(Problem):
            check.verify_docx("/nonexistent.docx")
        # A title that lost its centring is drawn elsewhere.
        with self.assertRaises(Problem):
            check.verify_rendering(lost_center)


if __name__ == "__main__":
    unittest.main()
