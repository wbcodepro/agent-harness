import io, json, contextlib, pathlib
import config, client

defaults = json.load(open("defaults.json"))
assert defaults.get("request_timeout_ms") == 30000 and "timeout" not in defaults, defaults
assert config.load()["request_timeout_ms"] == 30000
assert "timeout" not in config.load()

example = pathlib.Path("example.ini").read_text()
assert "request_timeout_ms = 10000" in example and "\ntimeout" not in example, example
assert config.load("example.ini")["request_timeout_ms"] == 10000

pathlib.Path("old.ini").write_text("host = old.example\ntimeout = 7\n")
err = io.StringIO()
with contextlib.redirect_stderr(err):
    old = config.load("old.ini")
assert old["request_timeout_ms"] == 7000 and "timeout" not in old, old
warning = err.getvalue().strip().splitlines()
assert len(warning) == 1 and "timeout" in warning[0] and "request_timeout_ms" in warning[0], warning
err = io.StringIO()
with contextlib.redirect_stderr(err):
    config.load("example.ini")
assert err.getvalue() == "", "a config with the new key must not warn"

assert client.describe() == "localhost: wait 30000ms, retry 2x", client.describe()
assert client.deadline_seconds() == 90.0, client.deadline_seconds()
assert client.deadline_seconds("old.ini") == 21.0
readme = pathlib.Path("README.md").read_text()
assert "`request_timeout_ms`" in readme and "30000" in readme and "`timeout`: seconds" not in readme
print("ok")
