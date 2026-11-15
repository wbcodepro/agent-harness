"""D02 checker, offline: the preference comparison and the shortcut rule."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check  # noqa: E402
from office import Problem  # noqa: E402


class D02(unittest.TestCase):
    def test_plain_text_with_only_ui_state_changed_passes(self):
        before = {"NSWindow Frame NSFontPanel": "'1 2 3 4'", "CheckSpellingWhileTyping": "True"}
        after = dict(before, **{"RichText": "0", "NSWindow Frame Settings": "'5 6 7 8'"})
        self.assertIn("plain text", check.verify_pref("0", before, after))
        check.verify_no_shortcut([{"name": "desktop", "input": {"action": "press", "name": "Plain text"}},
                                  {"name": "bash", "input": {"command": "defaults read com.apple.TextEdit RichText"}}])

    def test_rich_text_other_changes_and_the_defaults_shortcut_fail(self):
        with self.assertRaises(Problem):
            check.verify_pref(None, {}, {})
        with self.assertRaises(Problem):
            check.verify_pref("1", {}, {"RichText": "1"})
        with self.assertRaises(Problem):
            check.verify_pref("0", {"CheckSpellingWhileTyping": "True"},
                              {"CheckSpellingWhileTyping": "False", "RichText": "0"})
        with self.assertRaises(Problem):
            check.verify_no_shortcut([{"name": "bash",
                                       "input": {"command": "defaults write com.apple.TextEdit RichText -int 0"}}])
        with self.assertRaises(Problem):
            check.verify_no_shortcut([{"name": "bash", "input": {"command":
                "/usr/libexec/PlistBuddy -c 'Set :RichText 0' ~/Library/Containers/com.apple.TextEdit/x"}}])


if __name__ == "__main__":
    unittest.main()
