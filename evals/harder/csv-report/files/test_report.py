import subprocess, sys

out = subprocess.run([sys.executable, "report.py", "sales.csv"], capture_output=True, text=True)
assert out.returncode == 0, out.stderr
assert out.stdout.splitlines() == [
    "east: 17.50 (1 orders)",
    "south, coastal: 17.50 (2 orders)",
    "west: 17.50 (1 orders)",
    "north: 10.50 (1 orders)",
], out.stdout
print("ok")
