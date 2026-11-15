"""D05 checker, offline: RTF made by textutil from styled HTML, right and
wrong, and the untouched notice from setup."""
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import check  # noqa: E402
from office import Problem  # noqa: E402


def rtf(title="text-align:center;font:bold 20px Helvetica", pack="font:italic 12px Helvetica",
        lines=check.LINES):
    body = "".join(
        f'<p style="{style}">{text}</p>'
        for text, style in zip(lines, [title, "font:12px Helvetica", pack, "font:12px Helvetica"]))
    d = tempfile.mkdtemp()
    with open(os.path.join(d, "n.html"), "w") as f:
        f.write(f"<html><body>{body}</body></html>")
    out = os.path.join(d, "notice.rtf")
    subprocess.run(["textutil", "-convert", "rtf", os.path.join(d, "n.html"), "-output", out], check=True)
    return out


def untouched():
    d = tempfile.mkdtemp()
    env = dict(os.environ, HAME_OUTBOX=d, HAME_RUN_DIR=d)
    subprocess.run([sys.executable, os.path.join(HERE, "setup.py")], env=env, check=True)
    return os.path.join(d, "d05", "notice.rtf")


class D05(unittest.TestCase):
    def test_the_requested_formatting_passes(self):
        path = rtf()
        self.assertIn("centred", check.verify_rtf(path))
        self.assertIn("centred", check.verify_rendering(path))

    def test_untouched_partial_and_changed_documents_fail(self):
        cases = [untouched(), rtf(title="text-align:center;font:20px Helvetica"),
                 rtf(title="font:bold 20px Helvetica"), rtf(title="text-align:center;font:bold 14px Helvetica"),
                 rtf(pack="font:12px Helvetica"),
                 rtf(lines=["Office Move Notice!"] + check.LINES[1:])]
        for path in cases:
            with self.assertRaises(Problem, msg=path):
                check.verify_rtf(path)
        with self.assertRaises(Problem):
            check.verify_rendering(untouched())
        with self.assertRaises(Problem):
            check.verify_rendering(rtf(title="font:bold 20px Helvetica"))


if __name__ == "__main__":
    unittest.main()
