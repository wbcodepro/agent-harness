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

Binaries for other targets are built on those platforms (or by the `ci` workflow, extended with an upload step, once authorized) with the same command; each run adds its own archive. Cross-compiling the desktop helper is not needed: it is compiled from source on the user's Mac at first use.

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
