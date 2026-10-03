#!/usr/bin/env bash
# Checks the Linux tarball that users download: its layout, CLI, embedded page,
# and JavaScript asset. Run after scripts/release-local.sh for a native target.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
NAME="tasks-pending-$VERSION-$TARGET"
ARCHIVE="$ROOT/dist/$NAME.tar.gz"
WORK="$(mktemp -d)"
LOG="$WORK/tasks-pending.log"
PORT="${PORT:-18080}"
pid=""

cleanup() {
  if [[ -n $pid ]]; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

test -f "$ARCHIVE"
tar -xzf "$ARCHIVE" -C "$WORK"
BUNDLE="$WORK/$NAME"

test -x "$BUNDLE/bin/tasks-pending"
# One executable, nothing else.
test "$(find "$BUNDLE/bin" -mindepth 1 | wc -l)" -eq 1
test ! -e "$BUNDLE/frontend"
for path in CHANGELOG.md LICENSE README.md config.example.toml docs/install.md docs/operations.md \
  packaging/systemd/tasks-pending.service scripts/install.sh; do
  test -e "$BUNDLE/$path"
done

"$BUNDLE/bin/tasks-pending" --help >/dev/null
"$BUNDLE/bin/tasks-pending" serve --help >/dev/null
"$BUNDLE/bin/tasks-pending" tui --help >/dev/null
"$BUNDLE/bin/tasks-pending" serve --listen "127.0.0.1:$PORT" >"$LOG" 2>&1 &
pid=$!

ready=0
for _ in $(seq 1 50); do
  if ! kill -0 "$pid" 2>/dev/null; then
    wait "$pid" || true
    cat "$LOG" >&2
    exit 1
  fi
  if curl -fsS "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.1
done
if (( !ready )); then
  cat "$LOG" >&2
  exit 1
fi

page="$(curl -fsS "http://127.0.0.1:$PORT/")"
kill -0 "$pid" 2>/dev/null
[[ $page == *'id="app"'* ]]
asset="$(printf '%s' "$page" | sed -n 's/.*src="\([^"]*\.js\)".*/\1/p')"
test -n "$asset"
curl -fsSI "http://127.0.0.1:$PORT$asset" | grep -Fqi 'content-type: application/javascript; charset=utf-8'
