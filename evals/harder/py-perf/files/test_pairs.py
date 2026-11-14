import random, subprocess, sys
from pairs import count_pairs

assert count_pairs([], 5) == 0
assert count_pairs([1, 4, 2, 3, 4, 1], 5) == 5
assert count_pairs([2, 2, 2, 2], 4) == 6
assert count_pairs([5], 5) == 0
assert count_pairs([0, 0, 5, -5], 0) == 2

random.seed(11)
small = [random.randint(-50, 50) for _ in range(400)]
slow = sum(1 for i in range(len(small)) for j in range(i + 1, len(small)) if small[i] + small[j] == 7)
assert count_pairs(small, 7) == slow

# The large case runs in its own process, so a slow version is stopped
# at the limit instead of running for hours.
big = (
    "import random; from pairs import count_pairs; random.seed(11); "
    "[random.randint(-50, 50) for _ in range(400)]; "
    "print(count_pairs([random.randint(-1000, 1000) for _ in range(300_000)], 13))"
)
try:
    out = subprocess.run([sys.executable, "-c", big], capture_output=True, text=True, timeout=5)
except subprocess.TimeoutExpired:
    raise SystemExit("count_pairs is too slow: 300000 values took more than 5 seconds")
assert out.returncode == 0, out.stderr
assert out.stdout.strip() == "22355931", out.stdout
print("ok")
