"""D05 setup: a plain notice (Helvetica 12, no styles) as RTF in the
outbox, which only applications can reach."""
import os
import subprocess

TEXT = ("Office Move Notice\n"
        "Our team moves to the 5th floor on Friday, 28 November.\n"
        "Please pack your desk by Thursday 4 pm.\n"
        "IT will move computers and phones.\n")
folder = os.path.join(os.environ["HAME_OUTBOX"], "d05")
os.makedirs(folder, exist_ok=True)
txt = os.path.join(os.environ["HAME_RUN_DIR"], "notice.txt")
open(txt, "w").write(TEXT)
subprocess.run(["textutil", "-convert", "rtf", "-font", "Helvetica", "-fontsize", "12", txt,
                "-output", os.path.join(folder, "notice.rtf")], check=True)
