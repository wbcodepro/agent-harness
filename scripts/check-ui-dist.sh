#!/bin/sh
# Fails when crates/hame-web/ui-dist is not what `npm run build` makes from
# the current sources of apps/ui, so the interface embedded in `hame web`
# never drifts from its code.
#
# Usage, from the repository root (after `npm ci`):  sh scripts/check-ui-dist.sh
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

if [ ! -d node_modules ]; then
    echo "check-ui-dist: run \`npm ci\` first" >&2
    exit 2
fi

out=$(mktemp -d "${TMPDIR:-/tmp}/hame-ui-dist.XXXXXX")
trap 'rm -rf "$out"' EXIT INT TERM

HAME_UI_OUT="$out" npm run build --workspace apps/ui --silent >/dev/null

if diff -r "$out" crates/hame-web/ui-dist >/dev/null; then
    echo "check-ui-dist: crates/hame-web/ui-dist is up to date"
else
    echo "check-ui-dist: crates/hame-web/ui-dist is stale; run \`npm run build\` and commit the result" >&2
    diff -r "$out" crates/hame-web/ui-dist >&2 || true
    exit 1
fi
