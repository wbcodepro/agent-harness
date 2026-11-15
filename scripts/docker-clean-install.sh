#!/usr/bin/env bash
# Builds and tests hame from a clean copy of the source in a fresh Linux
# container: only what `git archive` exports (no target/, .env, .claude/ or
# .solutions/), no cargo cache, an unprivileged user, an empty HAME_HOME.
#
# Usage: scripts/docker-clean-install.sh [REV] [IMAGE]
#   REV    git revision to export (default HEAD; commit first)
#   IMAGE  container image (default rust:1-bookworm)
#
# Needs Docker. Downloads crates from crates.io inside the container. The
# steps run inside are in scripts/clean-install-inner.sh, taken from the
# exported revision itself.
set -euo pipefail

rev="${1:-HEAD}"
image="${2:-rust:1-bookworm}"
cd "$(git rev-parse --show-toplevel)"

echo "== exporting $(git rev-parse --short "$rev") with git archive into $image"
git archive --format=tar "$rev" | docker run --rm -i -e CARGO_TERM_COLOR=never "$image" \
  bash -euo pipefail -c '
    mkdir /src && tar -x -C /src
    exec bash /src/scripts/clean-install-inner.sh
  '
