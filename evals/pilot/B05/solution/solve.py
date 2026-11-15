"""B05 reference: saves the theme in the work profile's storage."""
import os
import sys

sys.path.insert(0, os.path.join(os.environ["HAME_SUITE_DIR"], "fixture"))
from pilot import profile_set  # noqa: E402

profile_set("work", "theme", "dark")
