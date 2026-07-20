#!/usr/bin/env bash
# build.sh — release build. On the maintainer's host a machine-wide guard
# serialises builds and manages build-cache expiry; anywhere else this is a
# plain `cargo build --release`.
set -euo pipefail
REPO_DIR="$(cd "$(dirname "$0")" && pwd)"
GUARD="$HOME/.local/bin/buildguard"
ROOT="${CARGO_TARGET_DIR:-$REPO_DIR/target}"
cd "$REPO_DIR"
if [ -x "$GUARD" ]; then
    exec "$GUARD" rust "$ROOT" -- cargo build --release "$@"
fi
exec cargo build --release "$@"
