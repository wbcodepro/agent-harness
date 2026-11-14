"""A few of the examples of SPEC.md. Run: python3 test_merge3.py"""
import unittest

from merge3 import merge3


def T(letters):
    return "".join(x + "\n" for x in letters.split())


class Examples(unittest.TestCase):
    def test_clean(self):
        self.assertEqual(merge3(T("a b c"), T("a X c"), T("a b c d")),
                         (T("a X c d"), 0))

    def test_conflict(self):
        text, n = merge3(T("a b c"), T("a X c"), T("a Y c"))
        self.assertEqual(n, 1)
        self.assertEqual(
            text, "a\n<<<<<<< ours\nX\n=======\nY\n>>>>>>> theirs\nc\n")

    def test_final_newline(self):
        self.assertEqual(merge3("a\nb", "a\nb\n", "A\nb"), ("A\nb\n", 0))


if __name__ == "__main__":
    unittest.main()
