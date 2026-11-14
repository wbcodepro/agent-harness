"""Checks merge3.py against SPEC.md."""
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
PY = sys.executable


# ---- deterministic generator for the large inputs -------------------------
class LCG:
    def __init__(self, seed):
        self.s = seed & 0xFFFFFFFFFFFFFFFF

    def next(self):
        self.s = (self.s * 6364136223846793005 + 1442695040888963407) & 0xFFFFFFFFFFFFFFFF
        return self.s >> 33

    def below(self, n):
        return self.next() % n


COMMON = ["", "    }", "}", "    return x", "    else:", "        pass", "    x += 1", "{"]


def big_case(seed, n, edits_ours, edits_theirs):
    g = LCG(seed)
    base = []
    for i in range(n):
        r = g.below(10)
        if r < 6:
            base.append(COMMON[g.below(len(COMMON))])
        else:
            base.append("    v%d = f(%d)" % (g.below(400), g.below(50)))

    def edit(lines, count, tag):
        lines = list(lines)
        done = 0
        while done < count:
            pos = g.below(len(lines))
            r = g.below(4)
            if r == 0:
                del lines[pos]
                done += 1
            elif r == 1:
                lines.insert(pos, COMMON[g.below(len(COMMON))])
                done += 1
            elif r == 2:
                lines.insert(pos, "    %s_new%d()" % (tag, g.below(1000)))
                done += 1
            else:
                if done + 2 > count:
                    continue
                lines[pos] = "    %s_chg%d()" % (tag, g.below(1000))
                done += 2
        return lines

    ours = list(base)
    theirs = list(base)
    step = n // 8
    for k in range(7):
        pos = step // 2 + k * step + g.below(50)
        if k < 5:
            ours[pos] = "    o_both%d()" % k
            theirs[pos] = "    t_both%d()" % k
        else:
            ours[pos] = "    same%d()" % k
            theirs[pos] = "    same%d()" % k
    ours = edit(ours, edits_ours, "o")
    theirs = edit(theirs, edits_theirs, "t")

    def j(ls):
        return "\n".join(ls) + "\n"
    return j(base), j(ours), j(theirs)


def scrambled_case(seed, n):
    """Texts of n lines that have little in common (rule 30)."""
    g = LCG(seed)
    words = ["w%d" % k for k in range(12)]
    mk = lambda: "".join(words[g.below(len(words))] + "\n" for _ in range(n))
    return mk(), mk(), mk()


BIG = [
    # seed, lines, edits ours, edits theirs, style
    (101, 20000, 86, 86, "merge"),
    (102, 20000, 60, 84, "diff3"),
]
SCRAMBLED = [(7, 300, "merge"), (8, 300, "diff3")]


def child(kind, idx):
    sys.path.insert(0, os.getcwd())
    import merge3
    if kind == "big":
        seed, n, eo, et, style = BIG[idx]
        b, o, t = big_case(seed, n, eo, et)
    else:
        seed, n, style = SCRAMBLED[idx]
        b, o, t = scrambled_case(seed, n)
    t0 = time.time()
    text, conflicts = merge3.merge3(b, o, t, style=style)
    dt = time.time() - t0
    print(json.dumps({"sha": hashlib.sha256(text.encode("utf-8")).hexdigest(),
                      "conflicts": conflicts, "seconds": dt}))


def main():
    with open(os.path.join(HERE, "check_cases.json"), encoding="utf-8") as fh:
        data = json.load(fh)
    failures = []

    def fail(name, why):
        failures.append("%s: %s" % (name, why))

    sys.path.insert(0, os.getcwd())
    try:
        import merge3
    except Exception as e:  # noqa
        print("cannot import merge3:", repr(e))
        return 1

    # ---- function cases
    for c in data["cases"]:
        kw = {}
        for k in ("style", "ignore_ws"):
            if k in c:
                kw[k] = c[k]
        if "labels" in c:
            kw["labels"] = tuple(c["labels"])
        try:
            got = merge3.merge3(c["base"], c["ours"], c["theirs"], **kw)
        except BaseException as e:  # noqa
            fail(c["name"], "raised %r" % (e,))
            continue
        if (not isinstance(got, tuple) or len(got) != 2 or type(got[0]) is not str
                or type(got[1]) is not int):
            fail(c["name"], "result is not (str, int): %r" % (got,))
        elif got != (c["text"], c["conflicts"]):
            fail(c["name"], "got %r, want %r" % (got, (c["text"], c["conflicts"])))

    # ---- labels given as a list work like a tuple
    try:
        got = merge3.merge3("a\n", "b\n", "c\n", labels=["x", "y", "z"])
        if got != ("<<<<<<< x\nb\n=======\nc\n>>>>>>> z\n", 1):
            fail("labels-list", "got %r" % (got,))
    except BaseException as e:  # noqa
        fail("labels-list", "raised %r" % (e,))

    # ---- argument errors (rule 24)
    bad = [
        ("style", dict(style="zdiff"), ValueError),
        ("style-none", dict(style=None), ValueError),
        ("labels-two", dict(labels=("a", "b")), ValueError),
        ("labels-four", dict(labels=("a", "b", "c", "d")), ValueError),
        ("labels-newline", dict(labels=("a", "b\n", "c")), ValueError),
        ("labels-nonstr", dict(labels=("a", 1, "c")), ValueError),
        ("labels-string", dict(labels="abc"), ValueError),
    ]
    for name, kw, exc in bad:
        try:
            merge3.merge3("a\n", "a\n", "a\n", **kw)
            fail("arg-" + name, "no exception")
        except exc:
            pass
        except BaseException as e:  # noqa
            fail("arg-" + name, "raised %r, want %s" % (e, exc.__name__))
    for name, args in [("base-bytes", (b"a\n", "a\n", "a\n")),
                       ("ours-none", ("a\n", None, "a\n")),
                       ("theirs-list", ("a\n", "a\n", ["a"]))]:
        try:
            merge3.merge3(*args)
            fail("arg-" + name, "no exception")
        except TypeError:
            pass
        except BaseException as e:  # noqa
            fail("arg-" + name, "raised %r, want TypeError" % (e,))

    # ---- command line
    tmp = tempfile.mkdtemp(prefix="merge3check")

    def put(name, content):
        p = os.path.join(tmp, name)
        with open(p, "wb") as fh:
            fh.write(content)
        return p

    def run(args):
        try:
            r = subprocess.run([PY, "merge3.py"] + args, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, stdin=subprocess.DEVNULL, timeout=10)
            return r.returncode, r.stdout, r.stderr
        except subprocess.TimeoutExpired:
            return -99, b"", b""

    n = 0
    for c in data["cli"]:
        n += 1
        o = put("o%d.txt" % n, c["ours"].encode("utf-8"))
        b = put("b%d.txt" % n, c["base"].encode("utf-8"))
        t = put("t%d.txt" % n, c["theirs"].encode("utf-8"))
        files = [o, b, t]
        opts = list(c["opts"])
        args = files + opts if c.get("opts_last") else opts + files
        want_out = c["text"].encode("utf-8")
        want_code = 1 if c["conflicts"] else 0
        code, out, err = run(args)
        if code != want_code or out != want_out:
            fail("cli-" + c["name"], "exit %r stdout %r, want exit %r stdout %r"
                 % (code, out, want_code, want_out))
        # the same through -o
        dest = os.path.join(tmp, "out%d.txt" % n)
        code, out, err = run(args + ["-o", dest])
        got = None
        if os.path.exists(dest):
            with open(dest, "rb") as fh:
                got = fh.read()
        if code != want_code or out != b"" or got != want_out:
            fail("cli-o-" + c["name"], "exit %r stdout %r file %r, want exit %r file %r"
                 % (code, out, got, want_code, want_out))

    o = put("eo.txt", b"a\n")
    b = put("eb.txt", b"a\n")
    t = put("et.txt", b"a\n")
    latin = put("latin.txt", b"caf\xe9\n")
    missing = os.path.join(tmp, "does-not-exist.txt")
    keep = put("keep.txt", b"precious")
    fresh = os.path.join(tmp, "fresh.txt")
    errors = [
        ("two-files", [o, b]),
        ("four-files", [o, b, t, t]),
        ("no-files", []),
        ("unknown-option", ["--frobnicate", o, b, t]),
        ("four-labels", ["-L", "1", "-L", "2", "-L", "3", "-L", "4", o, b, t]),
        ("missing-file", [o, missing, t]),
        ("not-utf8", [o, b, latin]),
        ("not-utf8-ours", [latin, b, t]),
    ]
    for name, args in errors:
        code, out, err = run(args)
        if code != 2 or out != b"" or err.strip() == b"":
            fail("cli-err-" + name, "exit %r stdout %r stderr %r, want exit 2, empty stdout, a message"
                 % (code, out, err[:80]))
    for name, args in [("missing-file", [o, missing, t]), ("not-utf8", [o, b, latin]),
                       ("two-files", [o, b])]:
        code, out, err = run(args + ["-o", keep])
        with open(keep, "rb") as fh:
            kept = fh.read()
        if code != 2 or kept != b"precious":
            fail("cli-err-o-keep-" + name, "exit %r, -o file now %r" % (code, kept))
        code, out, err = run(["-o", fresh] + args)
        if code != 2 or os.path.exists(fresh):
            fail("cli-err-o-fresh-" + name, "exit %r, -o file exists: %r" % (code, os.path.exists(fresh)))
            if os.path.exists(fresh):
                os.remove(fresh)

    # ---- size and speed (rules 30 and 31), each in its own process
    def timed(kind, idx, want, limit):
        try:
            r = subprocess.run([PY, os.path.abspath(__file__), "--child", kind, str(idx)],
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               stdin=subprocess.DEVNULL, timeout=limit)
        except subprocess.TimeoutExpired:
            fail("%s-%d" % (kind, idx), "not finished after %d seconds" % limit)
            return
        try:
            got = json.loads(r.stdout.decode().strip().splitlines()[-1])
        except Exception:
            fail("%s-%d" % (kind, idx), "crashed: %r" % (r.stderr[-300:],))
            return
        if got["sha"] != want["sha"] or got["conflicts"] != want["conflicts"]:
            fail("%s-%d" % (kind, idx), "wrong result (conflicts %r, want %r)"
                 % (got["conflicts"], want["conflicts"]))

    for i, want in enumerate(data["scrambled"]):
        timed("scrambled", i, want, 10)      # rule 30 says 2 seconds
    for i, want in enumerate(data["big"]):
        timed("big", i, want, 30)            # rule 31 says 10 seconds

    total = len(data["cases"]) + len(data["cli"]) * 2 + len(data["big"]) + len(data["scrambled"])
    if failures:
        print("%d FAILED" % len(failures))
        for f in failures[:15]:
            print("  " + f[:600])
        return 1
    print("ok (%d cases and the argument, command-line and speed checks)" % total)
    return 0


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--child":
        child(sys.argv[2], int(sys.argv[3]))
        sys.exit(0)
    sys.exit(main())
