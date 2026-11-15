#!/usr/bin/env python3
"""Third-party license report and CycloneDX SBOM for the hame binary.

Reads `cargo metadata` once per shipped platform, walks the dependency graph
from the `hame-cli` package (normal and build dependencies; dev-dependencies
are not shipped), and writes:

- docs/release/THIRD-PARTY-LICENSES.md: every third-party crate, version,
  license expression and the platforms it is built for, with non-permissive
  licenses flagged;
- docs/release/sbom.cdx.json: a CycloneDX 1.5 JSON SBOM of the same graph.

Usage (from anywhere in the repository):
    python3 -I scripts/license-report.py [--check]

--check exits 1 when a crate has a license outside the accepted set (see
ACCEPTED and REVIEWED below) instead of only reporting it. Needs cargo and
network-free access to the crates already in Cargo.lock (cargo fetch).
"""

import argparse
import hashlib
import datetime
import json
import os
import re
import subprocess
import sys
import uuid

PLATFORMS = [
    ("aarch64-apple-darwin", "macOS arm64"),
    ("x86_64-apple-darwin", "macOS x86_64"),
    ("x86_64-unknown-linux-gnu", "Linux x86_64"),
    ("aarch64-unknown-linux-gnu", "Linux arm64"),
    ("x86_64-pc-windows-msvc", "Windows x86_64"),
]
ROOT_PACKAGE = "hame-cli"

# Licenses that impose no conditions beyond keeping notices.
PERMISSIVE = {
    "MIT", "MIT-0", "Apache-2.0", "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause", "BSD-3-Clause", "0BSD", "ISC", "Zlib", "BSL-1.0",
    "Unicode-3.0", "Unicode-DFS-2016", "CC0-1.0", "Unlicense",
    "CDLA-Permissive-2.0", "bzip2-1.0.6",
}
# Accepted, but worth a note in the report.
REVIEWED = {
    "MPL-2.0": "file-level copyleft: the crate's own files stay under MPL-2.0; "
               "acceptable while they are used unmodified (source is on crates.io)",
    "WTFPL": "permissive in effect, unusual wording",
}
ACCEPTED = PERMISSIVE | set(REVIEWED)


def repo_root():
    out = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True,
                         text=True, check=True)
    return out.stdout.strip()


def metadata(root, triple):
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", triple],
        cwd=root, capture_output=True, text=True, check=True)
    return json.loads(out.stdout)


def shipped(meta):
    """Package ids reachable from the root package, and which of them are
    reachable only through build-dependencies (build scripts, proc macros'
    helpers): those run at build time and are not linked into the binary."""
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = next(p["id"] for p in meta["packages"]
                if p["name"] == ROOT_PACKAGE and p["source"] is None)
    linked, build_only = set(), set()

    def walk(start, into):
        stack = [start]
        while stack:
            pid = stack.pop()
            if pid in into:
                continue
            into.add(pid)
            for dep in nodes[pid]["deps"]:
                kinds = {k["kind"] for k in dep["dep_kinds"]}
                if None in kinds:
                    stack.append(dep["pkg"])

    walk(root, linked)
    # Build-dependencies of anything linked, and what they pull in.
    frontier = set()
    for pid in list(linked):
        for dep in nodes[pid]["deps"]:
            if "build" in {k["kind"] for k in dep["dep_kinds"]}:
                frontier.add(dep["pkg"])
    for pid in frontier:
        walk(pid, build_only)
    build_only -= linked
    # Proc macros run in the compiler, not in the binary.
    for pid in list(linked):
        if any("proc-macro" in t["kind"] for t in packages[pid]["targets"]):
            linked.discard(pid)
            build_only.add(pid)
    return packages, nodes, linked, build_only


def alternatives(expression):
    """An SPDX expression as a list of alternatives, each a list of licenses
    that all apply. Handles OR, AND, WITH, parentheses one level deep and
    the old `/` separator."""
    expr = expression.replace("/", " OR ")
    expr = re.sub(r"[()]", " ", expr)
    alts = []
    for alt in re.split(r"\s+OR\s+", expr.strip()):
        parts = [p.strip() for p in re.split(r"\s+AND\s+", alt) if p.strip()]
        # `X+` means X or any later version; judged as X.
        alts.append([re.sub(r"\s+", " ", p).removesuffix("+") for p in parts])
    return alts


def verdict(license_expr):
    """(status, chosen) where status is permissive | reviewed | flagged and
    chosen is the alternative hame relies on."""
    if not license_expr:
        return "flagged", "no SPDX license field; read the crate's license file"
    best = None
    for alt in alternatives(license_expr):
        if all(p in PERMISSIVE for p in alt):
            return "permissive", " AND ".join(alt)
        if all(p in ACCEPTED for p in alt) and best is None:
            best = alt
    if best:
        return "reviewed", " AND ".join(best)
    return "flagged", license_expr


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true",
                        help="exit 1 if a crate's license is not accepted")
    args = parser.parse_args()
    root = repo_root()

    crates = {}  # id -> record
    graph = {}   # id -> set of dependency ids (union over platforms)
    workspace_version = None
    for triple, label in PLATFORMS:
        meta = metadata(root, triple)
        packages, nodes, linked, build_only = shipped(meta)
        for pid in linked | build_only:
            pkg = packages[pid]
            if pkg["source"] is None:
                if pkg["name"] == ROOT_PACKAGE:
                    workspace_version = pkg["version"]
                continue
            rec = crates.setdefault(pid, {
                "name": pkg["name"],
                "version": pkg["version"],
                "license": pkg.get("license") or "",
                "license_file": pkg.get("license_file"),
                "repository": pkg.get("repository") or "",
                "description": (pkg.get("description") or "").strip(),
                "platforms": [],
                "linked": False,
            })
            rec["platforms"].append(label)
            rec["linked"] = rec["linked"] or pid in linked
            graph.setdefault(pid, set()).update(
                d["pkg"] for d in nodes[pid]["deps"]
                if packages[d["pkg"]]["source"] is not None
                and d["pkg"] in (linked | build_only))

    records = sorted(crates.values(), key=lambda r: (r["name"], r["version"]))
    for rec in records:
        rec["status"], rec["chosen"] = verdict(rec["license"])

    write_markdown(root, records, workspace_version)
    write_sbom(root, crates, graph, workspace_version)

    flagged = [r for r in records if r["status"] == "flagged"]
    reviewed = [r for r in records if r["status"] == "reviewed"]
    print(f"{len(records)} third-party crates "
          f"({sum(r['linked'] for r in records)} linked into the binary, "
          f"{sum(not r['linked'] for r in records)} build-time only); "
          f"{len(reviewed)} reviewed, {len(flagged)} flagged")
    for r in reviewed + flagged:
        print(f"  {r['status']}: {r['name']} {r['version']}: {r['license'] or r['license_file']}")
    if args.check and flagged:
        sys.exit(1)


PLATFORM_LABELS = [label for _, label in PLATFORMS]


def platforms_text(rec):
    if len(rec["platforms"]) == len(PLATFORMS):
        return "all"
    return ", ".join(p for p in PLATFORM_LABELS if p in rec["platforms"])


def write_markdown(root, records, version):
    path = os.path.join(root, "docs", "release", "THIRD-PARTY-LICENSES.md")
    linked = [r for r in records if r["linked"]]
    build = [r for r in records if not r["linked"]]
    reviewed = [r for r in records if r["status"] == "reviewed"]
    flagged = [r for r in records if r["status"] == "flagged"]
    counts = {}
    for r in records:
        counts[r["chosen"]] = counts.get(r["chosen"], 0) + 1
    lines = [
        "# Third-party licenses",
        "",
        f"Generated by `scripts/license-report.py` from `cargo metadata --locked` for hame "
        f"{version}, for the platforms hame is built for: {', '.join(PLATFORM_LABELS)}. "
        "Regenerate it whenever `Cargo.lock` changes. hame itself is MIT (see `LICENSE`).",
        "",
        f"**{len(records)} third-party crates**: {len(linked)} are linked into the `hame` binary "
        f"on at least one platform; {len(build)} run only at build time (build scripts and "
        "procedural macros) and are not distributed. Dev-dependencies (tests only) are not "
        "listed.",
        "",
        "## Findings",
        "",
    ]
    if not flagged:
        lines.append("- No crate has a copyleft or unknown license that would restrict "
                     "distributing the binary.")
    for r in flagged:
        lines.append(f"- **Flagged**: `{r['name']}` {r['version']}: "
                     f"`{r['license'] or r['license_file']}`; needs review before release.")
    for r in reviewed:
        why = REVIEWED.get(r["chosen"].split(" AND ")[0], "")
        for lic, note in REVIEWED.items():
            if lic in r["chosen"]:
                why = note
        lines.append(f"- `{r['name']}` {r['version']} is `{r['license']}` "
                     f"({'linked' if r['linked'] else 'build time only'}): {why}.")
    lines += [
        "- Where a crate offers a choice (`A OR B`), hame relies on the permissive alternative "
        "shown in the *Used under* column (for example `GPL-2.0-only OR BSD-3-Clause` is used "
        "under BSD-3-Clause).",
        "- Binary distributions must carry the license texts and notices of the linked crates "
        "(MIT, BSD and Apache-2.0 require it). `cargo about` or a copy of each crate's license "
        "file can produce that bundle; this report is the inventory, not the bundle.",
        "",
        "### Licenses used",
        "",
        "| License (as used) | Crates |",
        "|---|---:|",
    ]
    for lic, n in sorted(counts.items(), key=lambda kv: (-kv[1], kv[0])):
        lines.append(f"| `{lic}` | {n} |")
    lines += [
        "",
        "## Runtime components not distributed with hame",
        "",
        "Some features use software the user installs separately. hame does not ship, link or "
        "redistribute them.",
        "",
        "| Component | Needed by | License | Notes |",
        "|---|---|---|---|",
        "| Python 3 | browser tool, `script` tool, Python syntax checks | PSF-2.0 | user-installed |",
        "| Playwright for Python (`playwright` on PyPI) | browser tool | Apache-2.0 | "
        "`pip install playwright`; downloads its own driver (Node.js, MIT) |",
        "| Google Chrome | browser tool (default `chrome` channel) | proprietary (Google Chrome "
        "Terms of Service) | user-installed; `chromium` channel uses Playwright's Chromium "
        "build (BSD-3-Clause and others) after `playwright install chromium` |",
        "| Swift toolchain (`swiftc`, Xcode command line tools) | desktop tool on macOS (builds the "
        "helper on first use) | Apache-2.0 with Runtime Library Exception (Swift); Xcode tools "
        "under Apple's license | user-installed; the compiled helper links Apple system "
        "frameworks only |",
        "| Git, bash | checkpoints, shell tool | GPL-2.0 / GPL-3.0 | run as separate programs, "
        "not linked |",
        "| Language servers, MCP servers, plugins | optional | their own | configured by the user |",
        "",
        "## Crates linked into the binary",
        "",
        "| Crate | Version | License | Used under | Platforms |",
        "|---|---|---|---|---|",
    ]
    for r in linked:
        lines.append(row(r))
    lines += [
        "",
        "## Build-time only crates",
        "",
        "| Crate | Version | License | Used under | Platforms |",
        "|---|---|---|---|---|",
    ]
    for r in build:
        lines.append(row(r))
    lines.append("")
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines))


def row(r):
    mark = {"permissive": "", "reviewed": " (reviewed)", "flagged": " **(flagged)**"}[r["status"]]
    lic = r["license"] or f"file: {r['license_file']}"
    return (f"| `{r['name']}` | {r['version']} | `{lic}` | `{r['chosen']}`{mark} | "
            f"{platforms_text(r)} |")


def purl(rec):
    return f"pkg:cargo/{rec['name']}@{rec['version']}"


def write_sbom(root, crates, graph, version):
    path = os.path.join(root, "docs", "release", "sbom.cdx.json")
    commit = subprocess.run(["git", "rev-parse", "HEAD"], cwd=root, capture_output=True,
                            text=True).stdout.strip()
    root_ref = f"pkg:cargo/hame-cli@{version}"
    components = []
    for pid, rec in sorted(crates.items(), key=lambda kv: (kv[1]["name"], kv[1]["version"])):
        comp = {
            "type": "library",
            "bom-ref": purl(rec),
            "name": rec["name"],
            "version": rec["version"],
            "purl": purl(rec),
            "scope": "required" if rec["linked"] else "excluded",
        }
        if rec["description"]:
            comp["description"] = rec["description"]
        if rec["license"]:
            comp["licenses"] = [{"expression": rec["license"].replace("/", " OR ")}]
        if rec["repository"]:
            comp["externalReferences"] = [{"type": "vcs", "url": rec["repository"]}]
        comp["properties"] = [{"name": "hame:platforms", "value": platforms_text(rec)}]
        components.append(comp)
    direct = sorted({purl(crates[d]) for d in direct_deps(root, crates)})
    dependencies = [{"ref": root_ref, "dependsOn": direct}]
    for pid, deps in sorted(graph.items(), key=lambda kv: purl(crates[kv[0]])):
        dependencies.append({
            "ref": purl(crates[pid]),
            "dependsOn": sorted({purl(crates[d]) for d in deps if d in crates}),
        })
    # Reproducible: the serial number and timestamp derive from the commit
    # and the lockfile, so regenerating an unchanged tree changes nothing.
    with open(os.path.join(root, "Cargo.lock"), "rb") as f:
        lock = f.read()
    serial = uuid.uuid5(uuid.NAMESPACE_URL, f"hame:{version}:{commit}:{hashlib.sha256(lock).hexdigest()}")
    when = subprocess.run(["git", "log", "-1", "--format=%cI"], cwd=root, capture_output=True,
                          text=True).stdout.strip() or datetime.datetime.now(
                              datetime.timezone.utc).isoformat()
    bom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": f"urn:uuid:{serial}",
        "version": 1,
        "metadata": {
            "timestamp": when,
            "tools": {"components": [{"type": "application", "name": "scripts/license-report.py"}]},
            "component": {
                "type": "application",
                "bom-ref": root_ref,
                "name": "hame",
                "version": version,
                "purl": root_ref,
                "licenses": [{"license": {"id": "MIT"}}],
                "properties": [{"name": "hame:source-commit", "value": commit}],
            },
        },
        "components": components,
        "dependencies": dependencies,
    }
    with open(path, "w", encoding="utf-8") as f:
        json.dump(bom, f, indent=2)
        f.write("\n")


_DIRECT = None


def direct_deps(root, crates):
    """Third-party crates any workspace crate depends on directly (normal
    or build), on any platform."""
    global _DIRECT
    if _DIRECT is None:
        _DIRECT = set()
        for triple, _ in PLATFORMS:
            meta = metadata(root, triple)
            packages = {p["id"]: p for p in meta["packages"]}
            for node in meta["resolve"]["nodes"]:
                if packages[node["id"]]["source"] is not None:
                    continue
                for dep in node["deps"]:
                    kinds = {k["kind"] for k in dep["dep_kinds"]}
                    if (None in kinds or "build" in kinds) and dep["pkg"] in crates:
                        _DIRECT.add(dep["pkg"])
    return _DIRECT


if __name__ == "__main__":
    main()
