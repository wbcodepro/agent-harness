#!/usr/bin/env bash
# Builds the release artifacts locally and checks that nothing private is in
# them. Publishes nothing.
#
#   dist/hame-<version>-src.tar.gz             git archive of HEAD
#   dist/hame-<version>-<target>.tar.gz        release binary + license files
#   dist/SHA256SUMS
#
# Checks: no .env files, keys (sk-…, sk-ant-…, AKIA…, ghp_…, private key
# blocks), session logs, traces or action journals, target/, .claude/,
# .solutions/ or .hame/ in either archive; no build-machine home directory
# in the binary; the binary runs.
#
# Usage: scripts/release-check.sh [--skip-build]
#   --skip-build  only re-check existing archives in dist/
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
root=$(pwd)
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
target=$(rustc -vV | sed -n 's/^host: //p')
dist="$root/dist"
src="$dist/hame-$version-src.tar.gz"
bin="$dist/hame-$version-$target.tar.gz"
failures=0

fail() { echo "FAIL: $*"; failures=$((failures + 1)); }
pass() { echo "ok:   $*"; }

if [ "${1:-}" != "--skip-build" ]; then
  if [ -n "$(git status --porcelain)" ]; then
    echo "note: the working tree has uncommitted changes; the source archive is HEAD ($(git rev-parse --short HEAD)) and leaves them out"
  fi
  rm -rf "$dist"
  mkdir -p "$dist"

  echo "== source archive"
  git archive --format=tar.gz --prefix="hame-$version/" -o "$src" HEAD

  echo "== release binary for $target"
  # Remapping keeps the build machine's paths (and so the user name) out of
  # panic messages and debug data in the binary.
  RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=/build --remap-path-prefix=$root=/src" \
    cargo build --release --locked -p hame-cli --target "$target" --target-dir "$root/target/dist" 2>&1 | tail -2
  stage="$dist/hame-$version-$target"
  mkdir -p "$stage"
  cp "$root/target/dist/$target/release/hame" "$stage/"
  cp LICENSE README.md CHANGELOG.md SECURITY.md "$stage/"
  cp docs/release/THIRD-PARTY-LICENSES.md docs/release/RELEASE-NOTES.md "$stage/" 2>/dev/null || true
  tar -C "$dist" -czf "$bin" "hame-$version-$target"
  rm -rf "$stage"

  (cd "$dist" && shasum -a 256 "$(basename "$src")" "$(basename "$bin")" > SHA256SUMS)
fi

echo "== contents"
forbidden_paths='(^|/)(\.env(\..*)?|target|\.claude|\.solutions|\.hame|node_modules)(/|$)|(^|/)(session\.v[0-9]+\.jsonl|actions\.v[0-9]+\.jsonl|trace\.v[0-9]+\.jsonl|trusted-workflows\.json|id_rsa|id_ed25519)$|\.(pem|p12|key)$'
for archive in "$src" "$bin"; do
  name=$(basename "$archive")
  count=$(tar -tzf "$archive" | wc -l | tr -d ' ')
  bad=$(tar -tzf "$archive" | grep -E "$forbidden_paths" || true)
  if [ -n "$bad" ]; then
    fail "$name contains paths that must not ship:"
    echo "$bad" | sed 's/^/      /'
  else
    pass "$name: $count entries, none of .env, target/, .claude/, .solutions/, .hame/, session logs, traces, journals, key files"
  fi
done

echo "== secrets"
# Long enough not to match the short fake keys the tests use. `sk-` must
# start a word, so a heading anchor like `#…-task-set-2026-…` is not a key.
secret_re='(^|[^A-Za-z0-9])sk-(ant-)?[A-Za-z0-9_-]{20,}|AKIA[0-9A-Z]{16}|ghp_[A-Za-z0-9]{36}|xox[abpr]-[A-Za-z0-9-]{10,}|-----BEGIN ([A-Z]+ )*PRIVATE KEY-----'
scan=$(mktemp -d)
trap 'rm -rf "$scan"' EXIT
for archive in "$src" "$bin"; do
  name=$(basename "$archive")
  dir="$scan/$name"
  mkdir -p "$dir"
  tar -xzf "$archive" -C "$dir"
  hits=$(grep -rIlaE "$secret_re" "$dir" 2>/dev/null || true)
  # The binary is not text; search its bytes too.
  binhits=$(find "$dir" -type f -name hame -exec grep -laE "$secret_re" {} + 2>/dev/null || true)
  if [ -n "$hits$binhits" ]; then
    fail "$name: something that looks like a key:"
    for f in $hits $binhits; do
      echo "      ${f#"$dir/"}: $(grep -aoE "$secret_re" "$f" | head -3 | sed -E 's/(.{8}).*/\1…/' | tr '\n' ' ')"
    done
  else
    pass "$name: no key patterns (sk-, sk-ant-, AKIA, ghp_, xox?-, private key blocks)"
  fi
done

echo "== binary"
exe=$(find "$scan" -type f -name hame | head -1)
if [ -n "$exe" ]; then
  if "$exe" --version >/dev/null 2>&1; then
    pass "$("$exe" --version) runs; $(du -h "$exe" | cut -f1) ($(file -b "$exe" | cut -d, -f1))"
  else
    fail "the binary does not run"
  fi
  leaks=$(grep -ac "$HOME" "$exe" || true)
  if [ "$leaks" != "0" ]; then
    fail "the binary contains the build machine's home directory ($leaks matches)"
  else
    pass "no build-machine home directory in the binary"
  fi
fi

echo "== checksums"
cat "$dist/SHA256SUMS"

if [ "$failures" -gt 0 ]; then
  echo "$failures check(s) failed"
  exit 1
fi
echo "all checks passed"
