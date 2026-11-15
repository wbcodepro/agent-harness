"""B05: the theme saved in the "work" profile, "personal" untouched. Read
from the profiles on disk with a separate browser, after the agent's
browser has closed."""
import os
import sys

sys.path.insert(0, os.path.join(os.environ["HAME_SUITE_DIR"], "fixture"))
from pilot import fail, ok, profile_get, state  # noqa: E402

work = profile_get("work", "theme")
personal = profile_get("personal", "theme")
saved = state()["prefs_saved"]
if work != "dark":
    fail(f"the work profile's saved theme is {work!r}, not 'dark' (saves seen by the site: {saved})")
if personal != "light":
    fail(f"the personal profile's theme changed to {personal!r}")
ok(f"work=dark, personal=light; {len(saved)} save(s)")
