"""Checks of the browser helper's pure functions: redirect resolution, file
names, and saving and reading workspace files without following links.

Run as `python3 -I helper_checks.py <path to browser_helper.py>`; exits
non-zero on the first failure. Needs neither Playwright nor a browser.
"""

import importlib.util
import os
import stat
import sys
import tempfile

spec = importlib.util.spec_from_file_location("browser_helper", sys.argv[1])
h = importlib.util.module_from_spec(spec)
spec.loader.exec_module(h)
POSIX = os.name == "posix"


def refused(code, fn, *args):
    try:
        fn(*args)
    except h.OpError as e:
        assert e.code == code, f"{fn.__name__}{args}: {e.code} {e.message}, expected {code}"
        return e
    raise AssertionError(f"{fn.__name__}{args} was not refused")


def check_redirects():
    base = "http://127.0.0.1:8080/a/b?q=1"
    cases = {
        # WHATWG reads these as another host; urljoin would not.
        "/\\evil.example/x": "http://evil.example/x",
        "\\\\evil.example/x": "http://evil.example/x",
        "http:\\\\evil.example/x": "http://evil.example/x",
        "http:///evil.example/x": "http://evil.example/x",
        "////evil.example/x": "http://evil.example/x",
        "http:/\\evil.example/x": "http://evil.example/x",
        "https:evil.example/x": "https://evil.example/x",
        " \t//evil.example/x\n": "http://evil.example/x",
        "//e\tvil.example/x": "http://evil.example/x",
        "http://127.0.0.1:8080@evil.example/": "http://evil.example/",
        "http://evil.example\\@127.0.0.1:8080/": "http://evil.example/@127.0.0.1:8080/",
        # Same origin.
        "/ok": "http://127.0.0.1:8080/ok",
        "ok\\x": "http://127.0.0.1:8080/a/ok/x",
        "http:ok": "http://127.0.0.1:8080/a/ok",
        "?q=\\x": "http://127.0.0.1:8080/a/b?q=\\x",
        "//127.0.0.1:8080/x": "http://127.0.0.1:8080/x",
        "": "http://127.0.0.1:8080/a/b?q=1",
    }
    for location, want in cases.items():
        got = h.resolve_redirect(base, location)
        assert got == want, f"{location!r} -> {got!r}, expected {want!r}"
    # Unsure: refused by the caller.
    for location in ("//ev%69l.example/", "//évil.example/", "http://a b/"):
        assert h.resolve_redirect(base, location) is None, location
    # Not a web origin: never granted.
    helper = h.Helper()
    helper.allow = [["*", "http://127.0.0.1:*"]]
    for location in ("javascript:alert(1)", "file:///etc/passwd", "/\\evil.example/x"):
        target = h.resolve_redirect(base, location)
        assert target is None or not helper.allowed("p", target), (location, target)
    assert helper.allowed("p", h.resolve_redirect(base, "/next"))


def check_names():
    cases = {
        "../x": "x",
        "..\\..\\x.csv": "x.csv",
        ".envrc": "envrc",
        "a/b": "b",
        "a\\b": "b",
        "..": "download",
        "": "download",
        "...": "download",
        "re‮port\x07.csv": "report.csv",
        'q<u>e:s"t|i?o*n.txt': "question.txt",
        "CON.txt": "_CON.txt",
        "name. ": "name",
        "ledger-2026-01-10-2026-01-20.csv": "ledger-2026-01-10-2026-01-20.csv",
        "Tiếng Việt.csv": "Tiếng Việt.csv",
    }
    for name, want in cases.items():
        got = h.safe_filename(name)
        assert got == want, f"{name!r} -> {got!r}, expected {want!r}"
    long = h.safe_filename("é" * 200 + ".csv")
    assert len(long.encode()) <= h.MAX_NAME_BYTES and long.endswith(".csv"), long


def check_saving(tmp):
    root = os.path.join(tmp, "work")
    outside = os.path.join(tmp, "outside")
    os.makedirs(root)
    os.makedirs(outside)
    victim = os.path.join(outside, "rc")
    src = os.path.join(tmp, "download.bin")
    with open(victim, "w") as f:
        f.write("VICTIM")
    with open(src, "wb") as f:
        f.write(b"payload")

    def read(*parts):
        with open(os.path.join(root, *parts), "rb") as f:
            return f.read()

    path, size, sha = h.save_file(src, root, ["new", "dir"], "out.csv", False)
    assert path == os.path.join(root, "new", "dir", "out.csv") and size == 7, (path, size)
    assert read("new", "dir", "out.csv") == b"payload"
    # An existing file is kept unless asked.
    refused("exists", h.save_file, src, root, ["new", "dir"], "out.csv", False)
    with open(os.path.join(root, "keep.txt"), "w") as f:
        f.write("old")
    refused("exists", h.save_file, src, root, [], "keep.txt", False)
    assert read("keep.txt") == b"old"
    h.save_file(src, root, [], "keep.txt", True)
    assert read("keep.txt") == b"payload"
    refused("invalid_input", h.save_file, src, root, [], "new", True)
    for bad in ("..", ".", "a/b", ""):
        refused("invalid_input", h.save_file, src, root, [], bad, False)
    # No temporary files are left behind.
    assert sorted(os.listdir(root)) == ["keep.txt", "new"], os.listdir(root)

    if not POSIX:
        return
    os.symlink(victim, os.path.join(root, "link.csv"))
    os.symlink(outside, os.path.join(root, "away"))
    os.symlink(os.path.join(tmp, "nothing"), os.path.join(root, "dangling"))
    for overwrite in (False, True):
        refused("unauthorized", h.save_file, src, root, [], "link.csv", overwrite)
        refused("unauthorized", h.save_file, src, root, ["away"], "x.csv", overwrite)
        refused("unauthorized", h.save_file, src, root, ["away", "deeper"], "x.csv", overwrite)
        refused("unauthorized", h.save_file, src, root, [], "dangling", overwrite)
        refused("unauthorized", h.save_file, src, root, ["dangling"], "x.csv", overwrite)
    assert sorted(os.listdir(outside)) == ["rc"] and not os.path.exists(os.path.join(tmp, "nothing"))

    # Replacing a file that has another hard link leaves the other alone.
    os.link(victim, os.path.join(root, "hard.txt"))
    h.save_file(src, root, [], "hard.txt", True)
    assert read("hard.txt") == b"payload"

    # A link that appears while the file is being written.
    real_copy = h._copy_into

    def racing(name, then=None):
        def copy(source, fd):
            result = real_copy(source, fd)
            os.symlink(victim, os.path.join(root, name))
            if then:
                then()
            return result
        return copy

    try:
        h._copy_into = racing("race1.csv")
        refused("unauthorized", h.save_file, src, root, [], "race1.csv", False)
        h._copy_into = racing("race2.csv")
        refused("unauthorized", h.save_file, src, root, [], "race2.csv", True)
        # Even past the last check, a rename replaces the link itself.
        real_check = h._check_target
        h._copy_into = racing("race3.csv", then=lambda: setattr(h, "_check_target", lambda *a: None))
        h.save_file(src, root, [], "race3.csv", True)
        h._check_target = real_check
        assert not os.path.islink(os.path.join(root, "race3.csv")) and read("race3.csv") == b"payload"
    finally:
        h._copy_into = real_copy
    with open(victim) as f:
        assert f.read() == "VICTIM"
    assert not [n for n in os.listdir(root) if n.endswith(".part")], os.listdir(root)


def check_uploads(tmp):
    root = os.path.join(tmp, "uwork")
    private = os.path.join(tmp, "private")
    os.makedirs(os.path.join(root, "docs"))
    os.makedirs(private)
    secret = os.path.join(tmp, "secret.txt")
    with open(secret, "w") as f:
        f.write("SECRET")
    with open(os.path.join(root, "docs", "invoice.pdf"), "wb") as f:
        f.write(b"%PDF")
    copy = h.copy_upload(root, ["docs", "invoice.pdf"], private)
    assert os.path.basename(copy) == "invoice.pdf" and copy.startswith(private)
    with open(copy, "rb") as f:
        assert f.read() == b"%PDF"
    refused("not_found", h.copy_upload, root, ["docs", "missing.pdf"], private)
    refused("invalid_input", h.copy_upload, root, ["docs"], private)
    if not POSIX:
        return
    os.symlink(secret, os.path.join(root, "docs", "link.pdf"))
    os.symlink(tmp, os.path.join(root, "up"))
    refused("unauthorized", h.copy_upload, root, ["docs", "link.pdf"], private)
    refused("unauthorized", h.copy_upload, root, ["up", "secret.txt"], private)
    os.link(secret, os.path.join(root, "docs", "hard.txt"))
    refused("unauthorized", h.copy_upload, root, ["docs", "hard.txt"], private)
    os.mkfifo(os.path.join(root, "docs", "pipe"))
    refused("invalid_input", h.copy_upload, root, ["docs", "pipe"], private)
    assert all(stat.S_ISREG(os.lstat(os.path.join(d, n)).st_mode)
               for d, _, names in os.walk(private) for n in names)


check_redirects()
check_names()
with tempfile.TemporaryDirectory() as tmp:
    check_saving(os.path.realpath(tmp))
    check_uploads(os.path.realpath(tmp))
print("helper checks passed", file=sys.stderr)
