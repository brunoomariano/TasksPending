#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST="$ROOT/dist/tasks-pending"

cargo build --workspace --release --locked
npm --prefix "$ROOT/frontend" run build

rm -rf "$DIST"
mkdir -p "$DIST/bin" "$DIST/frontend"

install -m 0755 "$ROOT/target/release/pending-api" "$DIST/bin/pending-api"
install -m 0755 "$ROOT/target/release/pending-tui" "$DIST/bin/pending-tui"
cp -R "$ROOT/frontend/dist/." "$DIST/frontend/"
install -m 0644 "$ROOT/README.md" "$DIST/README.md"

tar -C "$ROOT/dist" -czf "$ROOT/dist/tasks-pending.tar.gz" tasks-pending
sha256sum "$ROOT/dist/tasks-pending.tar.gz" > "$ROOT/dist/tasks-pending.tar.gz.sha256"

echo "Built $ROOT/dist/tasks-pending.tar.gz"
