#!/usr/bin/env python3
"""Reproduce verifier findings with synthetic fixtures on macOS, offline."""

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[4])
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("This recorded reproduction uses the macOS sandbox wrapper.")
    repo = args.repo.resolve()
    if not (repo / "Cargo.lock").is_file():
        parser.error("--repo must identify the hame workspace.")
    with tempfile.TemporaryDirectory(prefix="hame-acceptance-build-") as scratch:
        project = Path(scratch)
        (project / "src").mkdir()
        shutil.copyfile(Path(__file__).with_name("probe.rs"), project / "src/main.rs")
        manifest = [
            '[package]', 'name = "hame-acceptance-probe"',
            'version = "0.0.0"', 'edition = "2024"', '', '[dependencies]',
        ]
        for name in ("hame-agent", "hame-session", "hame-verify", "hame-sandbox"):
            manifest.append(f'{name} = {{ path = {json.dumps(str(repo / "crates" / name))} }}')
        manifest.append('tokio = { version = "1", features = ["rt", "time"] }')
        (project / "Cargo.toml").write_text("\n".join(manifest) + "\n")
        shutil.copyfile(repo / "Cargo.lock", project / "Cargo.lock")
        return subprocess.run([
            "cargo", "run", "--offline", "--manifest-path", str(project / "Cargo.toml"),
            "--target-dir", str(repo / "target"),
        ], cwd=repo).returncode


if __name__ == "__main__":
    raise SystemExit(main())
