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
try:
    import tomllib
except ImportError:
    print("FAIL: the checker needs Python 3.11 or later (tomllib)")
    sys.exit(2)

unchanged("config/app.defaults.toml", "cd902735b66025b46247d0eb800bbf99c60a73c28e1d26c4baece91dd0d1d70b")
try:
    with open("config/app.toml", "rb") as f:
        config = tomllib.load(f)
    with open("config/app.toml", encoding="utf-8") as f:
        text = f.read()
except Exception as e:  # noqa: BLE001
    fail(f"config/app.toml cannot be parsed: {e!r}")
    finish()

def get(path):
    node = config
    for part in path.split("."):
        if not isinstance(node, dict) or part not in node:
            return KeyError
        node = node[part]
    return node

want = {
    "schema_version": 2,
    "server.host": "0.0.0.0",
    "server.port": 9090,
    "server.timeout_ms": 45000,
    "log.level": "debug",
    "log.file": "/var/log/app/app.log",
    "cache.enabled": True,
    "cache.ttl_seconds": 300,
    "telemetry.enabled": False,
    "local.editor": "nvim",
    "local.greeting": "Xin chào",
}
for path, value in want.items():
    got = get(path)
    if got is KeyError:
        fail(f"{path} is missing")
    elif got != value or type(got) is not type(value):
        fail(f"{path} is {got!r}, expected {value!r}")
if get("server.timeout") is not KeyError:
    fail("server.timeout is still there")
for comment in (
    "# Local settings for this machine. Keep my changes!",
    "# my own settings, not in the defaults",
    "# listen on all interfaces for the LAN demo",
):
    if comment not in text:
        fail(f"the user's comment {comment!r} was lost")

finish()
