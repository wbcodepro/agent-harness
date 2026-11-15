"""Reference solution: edit the text, keeping comments."""
path = "config/app.toml"
with open(path, encoding="utf-8") as f:
    text = f.read()
text = text.replace("schema_version = 1", "schema_version = 2")
text = text.replace(
    "timeout = 45       # seconds; the VPN is slow",
    "timeout_ms = 45000  # the VPN is slow",
)
text += "\n[cache]\nenabled = true\nttl_seconds = 300\n\n[telemetry]\nenabled = false\n"
with open(path, "w", encoding="utf-8", newline="\n") as f:
    f.write(text)
