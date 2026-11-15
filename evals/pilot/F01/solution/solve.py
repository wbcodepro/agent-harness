"""Reference solution: rename the identifier in every Python file."""
import os
import re

old = re.compile(r"\bcalc_area\b")
for folder, _, names in os.walk("."):
    for name in names:
        if name.endswith(".py"):
            path = os.path.join(folder, name)
            with open(path, encoding="utf-8") as f:
                text = f.read()
            new = old.sub("compute_area", text)
            if new != text:
                with open(path, "w", encoding="utf-8", newline="\n") as f:
                    f.write(new)
