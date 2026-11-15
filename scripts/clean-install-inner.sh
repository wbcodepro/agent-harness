#!/usr/bin/env bash
# Runs inside the container started by scripts/docker-clean-install.sh, as
# root, with the exported source in /src. Builds and tests as an
# unprivileged user.
set -euo pipefail

uname -srm
. /etc/os-release && echo "$PRETTY_NAME"

# Anything that must not ship is absent from the export.
for p in target .env .claude .solutions; do
  if [ -e "/src/$p" ]; then
    echo "FAIL: $p is in the export"
    exit 1
  fi
done

command -v python3 >/dev/null || { apt-get update -qq && apt-get install -y -qq python3 >/dev/null; }
id builder >/dev/null 2>&1 || useradd -m builder
chown -R builder /src

if [ "$(id -u)" = 0 ] && [ -z "${HAME_INNER_USER:-}" ]; then
  # Plugin tests build a WebAssembly example (a documented prerequisite).
  rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || echo "note: wasm32 target not added"
  exec su builder -c "HAME_INNER_USER=1 PATH=/usr/local/cargo/bin:\$PATH RUSTUP_HOME=/usr/local/rustup CARGO_HOME=\$HOME/.cargo bash /src/scripts/clean-install-inner.sh"
fi

cd /src
echo "user: $(id -un)"
rustc --version
cargo --version
git --version
python3 --version

echo "== cargo build --release -p hame-cli"
start=$(date +%s)
cargo build --release -p hame-cli 2>&1 | tail -3
echo "build: $(( $(date +%s) - start )) s"

echo "== hame --help"
./target/release/hame --help
./target/release/hame --version

echo "== hame doctor (empty HAME_HOME, no key)"
mkdir -p /tmp/project
( cd /tmp/project && HAME_HOME=/tmp/hame-home /src/target/release/hame doctor ) || echo "doctor exit: $?"

echo "== cargo test --workspace"
start=$(date +%s)
cargo test --workspace --no-fail-fast >/tmp/test.log 2>&1 || true
echo "tests: $(( $(date +%s) - start )) s"
grep -E "^test result|FAILED|panicked" /tmp/test.log || true
echo "== failing tests (if any)"
grep -E "^test .* FAILED$" /tmp/test.log || echo "none"
echo "== summary"
awk '/^test result/ { p += $4; f += $6; i += $8 } END { print "passed " p ", failed " f ", ignored " i }' /tmp/test.log
if grep -q "^test result: FAILED" /tmp/test.log || grep -q "^error" /tmp/test.log; then
  grep -E "^error" -A10 /tmp/test.log | head -40 || true
  exit 1
fi
