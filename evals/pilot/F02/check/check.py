"""Hidden checker. Runs with the workspace as the current directory."""
import hashlib
import os
import sys

problems = []


def fail(message):
    problems.append(message)


def sha256(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def unchanged(path, digest):
    if not os.path.isfile(path):
        fail(f"{path} is missing")
    elif sha256(path) != digest:
        fail(f"{path} was changed")


def finish():
    if problems:
        for problem in problems:
            print("FAIL:", problem)
        sys.exit(1)
    print("PASS")
    sys.exit(0)
import csv
import shutil
import subprocess
import tempfile

unchanged("input/orders.csv", "2bdb4e8289f10aefff916ccb322c00694741940e9e9ca43a95d7419f4ffbac22")


def rows(path):
    with open(path, newline="", encoding="utf-8") as f:
        return [row for row in csv.reader(f) if row]


expected = [['customer', 'orders', 'total'], ['Le, Chi', '1', '1250.00'], ['Nguyen, An', '2', '150.50'], ['Bob Smith', '2', '99.99'], ['Tran, Binh', '1', '99.75'], ['Carol', '1', '15.00'], ['Dave', '1', '0.50']]
if not os.path.isfile("output/summary.csv"):
    fail("output/summary.csv is missing")
elif rows("output/summary.csv") != expected:
    fail(f"output/summary.csv is wrong: {rows('output/summary.csv')!r}")

# The fixed script on data it has not seen.
with tempfile.TemporaryDirectory() as scratch:
    shutil.copy("transform.py", scratch)
    os.makedirs(os.path.join(scratch, "input"))
    shutil.copy(
        os.path.join(os.environ["HAME_CHECK_DIR"], "alt_orders.csv"),
        os.path.join(scratch, "input", "orders.csv"),
    )
    ran = subprocess.run(
        [sys.executable, "-B", "transform.py"],
        cwd=scratch, capture_output=True, text=True, timeout=60,
    )
    produced = os.path.join(scratch, "output", "summary.csv")
    if ran.returncode != 0 or not os.path.isfile(produced):
        fail(f"transform.py fails on other input: {ran.stderr[-300:]!r}")
    elif rows(produced) != [['customer', 'orders', 'total'], ['Zed, Ann', '1', '2000.00'], ['Amy', '2', '30.30'], ['Bo', '1', '30.30'], ['Cy', '1', '30.30']]:
        fail(f"transform.py is wrong on other input: {rows(produced)!r}")

finish()
