# Release instructions

How to cut a hame release. **Pushing branches or tags, creating a GitHub release, uploading artifacts, publishing crates and running GitHub Actions all need the repository owner's explicit authorization for that release.** Everything up to step 6 is local and safe to repeat.

## 1. Prepare

- Merge the release branch state you intend to ship (for 0.1.0-rc.1: `unified-harness` plus the release-docs branch).
- Make sure nobody else is still landing changes for this version.

## 2. Version

The version lives once, in `Cargo.toml` (`[workspace.package] version`); every crate inherits it. Pre-releases use SemVer pre-release tags: `0.1.0-rc.1`, `0.1.0-rc.2`, then `0.1.0`.

```sh
sed -i '' 's/^version = ".*"$/version = "0.1.0"/' Cargo.toml    # first match is [workspace.package]
cargo update --workspace          # rewrites only the workspace crates' versions in Cargo.lock
git diff --stat Cargo.lock        # only hame-* entries should change
cargo run -q -p hame-cli -- --version
```

Then in `CHANGELOG.md`, rename `## Unreleased` to `## 0.1.0 — YYYY-MM-DD` and start a new empty `## Unreleased`. Update `docs/release/RELEASE-NOTES.md` (version, date, verification numbers).

## 3. Verify

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                                   # record the counts
scripts/docker-clean-install.sh HEAD                     # clean Linux build + tests (commit first)
python3 -I scripts/license-report.py --check             # regenerates THIRD-PARTY-LICENSES.md and sbom.cdx.json
git diff --exit-code docs/release/THIRD-PARTY-LICENSES.md docs/release/sbom.cdx.json || echo "commit the regenerated files"
```

Optional, on a machine with the prerequisites and the owner's agreement (they launch real applications or call paid APIs):

```sh
cargo test -p hame-desktop --test gui -- --ignored --test-threads=1        # drives TextEdit
HAME_LIVE_ANTHROPIC=1 cargo test -p hame-llm --test live_anthropic -- --ignored   # a few cents
```

The `ci` workflow (`.github/workflows/ci.yml`, manual trigger) runs fmt, clippy and tests on Ubuntu, macOS and Windows. Run it only with the owner's go-ahead (`gh workflow run ci`): Actions minutes are limited. Windows can instead be checked by hand with `docs/WINDOWS.md`.

Record the results (date, commit, counts) in `docs/release/TRACKER.md` and the release notes.

## 4. Commit and tag (local)

```sh
git commit -am "Release 0.1.0"
git tag -a v0.1.0 -m "hame 0.1.0"
```

Tags are `v` + the version (`v0.1.0-rc.1`).

## 5. Build the artifacts

```sh
scripts/release-check.sh
```

It writes, under `dist/` (ignored by git):

- `hame-<version>-src.tar.gz` — `git archive` of `HEAD` with prefix `hame-<version>/`;
- `hame-<version>-<host target>.tar.gz` — the release binary (built with `--locked` and `--remap-path-prefix` so the build machine's paths stay out of it) plus `LICENSE`, `README.md`, `CHANGELOG.md`, `SECURITY.md`, `THIRD-PARTY-LICENSES.md`, `RELEASE-NOTES.md`;
- `SHA256SUMS`.

and fails if either archive contains `.env` files, key material (`sk-…`, `sk-ant-…`, `AKIA…`, `ghp_…`, Slack tokens, private key blocks), session logs, traces, action journals, trust lists, `target/`, `.claude/`, `.solutions/` or `.hame/`, if the binary contains the build machine's home directory, or if it does not run. `scripts/release-check.sh --skip-build` re-checks existing archives.

Binaries for other targets are built on those platforms with the same command, or, once the owner authorizes the Actions minutes, by the manual `release-artifacts` workflow (`.github/workflows/release.yml`: license check, then a release build per shipped target, kept as 7-day workflow artifacts with `.sha256` files; it publishes nothing). That workflow has not been run yet. The secret and path checks of `release-check.sh` should be repeated on the downloaded archives (`--skip-build` after placing them in `dist/` with the source archive). Cross-compiling the desktop helper is not needed: it is compiled from source on the user's Mac at first use.

macOS binaries are not signed or notarized yet; users who download one must clear the quarantine attribute (`xattr -d com.apple.quarantine hame`) or build from source. Signing needs an Apple Developer ID and is a decision for the owner.

## 6. Check before publishing

- Read the release notes once more against `git log <previous tag>..HEAD`.
- Confirm `SHA256SUMS` matches the files you will upload.
- Make sure no file in `dist/` was produced from a dirty tree (the script warns).

## 7. Publish (owner's authorization required)

Only after the owner says so, for this specific release:

```sh
git push origin <release branch>
git push origin v0.1.0
gh release create v0.1.0 dist/hame-0.1.0-*.tar.gz dist/SHA256SUMS \
  --title "hame 0.1.0" --notes-file docs/release/RELEASE-NOTES.md      # add --prerelease for an rc
```

Publishing to crates.io is not planned: the crates depend on each other by path and are not designed as separate libraries. If that changes, each crate needs a `version` on its path dependencies and `cargo publish` in dependency order.

## 8. After

- Bump to the next development version if desired, and reopen `## Unreleased` in the changelog.
- Announce where the owner chooses.

## Record: 0.1.0-rc.1 release check

`scripts/release-check.sh` on macOS 26.6.2 arm64, 2026-11-15, at commit `31e0bed` (the source archive and its checksum change with every later commit; rerun before publishing):

```text
== source archive
== release binary for aarch64-apple-darwin
    Finished `release` profile [optimized] target(s) in 1m 21s
== contents
ok:   hame-0.1.0-rc.1-src.tar.gz: 931 entries, none of .env, target/, .claude/, .solutions/, .hame/, session logs, traces, journals, key files
ok:   hame-0.1.0-rc.1-aarch64-apple-darwin.tar.gz: 8 entries, none of .env, target/, .claude/, .solutions/, .hame/, session logs, traces, journals, key files
== secrets
ok:   hame-0.1.0-rc.1-src.tar.gz: no key patterns (sk-, sk-ant-, AKIA, ghp_, xox?-, private key blocks)
ok:   hame-0.1.0-rc.1-aarch64-apple-darwin.tar.gz: no key patterns (sk-, sk-ant-, AKIA, ghp_, xox?-, private key blocks)
== binary
ok:   hame 0.1.0-rc.1 runs;  42M (Mach-O 64-bit executable arm64)
ok:   no build-machine home directory in the binary
== checksums
860b72377c4b3456d8535eac90d9f3ece7006b2b3dff99cf67e863fc9eb63a1f  hame-0.1.0-rc.1-src.tar.gz
72431249f21fb5e17b19b25ed27fdcbb86ed90ab6afc3464f56df1e4f99dc9ae  hame-0.1.0-rc.1-aarch64-apple-darwin.tar.gz
all checks passed
```

The checker itself was tested by replacing the source archive with one holding a `.env` with an `sk-ant-…` key, a `.claude/` directory and a `session.v1.jsonl`: it reported all four paths and the key, and failed.
