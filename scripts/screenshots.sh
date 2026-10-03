#!/usr/bin/env bash
# Regenerates the README screenshots in docs/assets from the sandbox (simulated
# cards, no config or credentials), with a headless Chromium.
#
# Needs Node 22 or newer and a Chromium-based browser: `chromium`,
# `chromium-browser` or `google-chrome-stable` on PATH, or CHROMIUM=<binary>.
# The source icons are loaded from the Dashboard Icons CDN, so it needs
# network access.
#
# Environment: OUT (where the pictures go, docs/assets by default), APP_PORT
# and DEVTOOLS_PORT (local ports for the sandbox and the browser, 61002 and
# 61003 by default; both must be free).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${OUT:-$ROOT/docs/assets}"
APP_PORT="${APP_PORT:-61002}"
DEVTOOLS_PORT="${DEVTOOLS_PORT:-61003}"

browser="${CHROMIUM:-}"
for candidate in chromium chromium-browser google-chrome-stable google-chrome; do
  [[ -n $browser ]] && break
  command -v "$candidate" >/dev/null && browser="$candidate"
done
[[ -n $browser ]] || {
  echo "no Chromium-based browser found; set CHROMIUM=<binary>" >&2
  exit 1
}

npm --prefix "$ROOT/frontend" run build >/dev/null
cargo build --quiet --manifest-path "$ROOT/Cargo.toml" -p tasks-pending
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

# Something already answering on a port would be photographed (or driven)
# in place of what this script starts.
for port in "$APP_PORT" "$DEVTOOLS_PORT"; do
  if curl -sS -o /dev/null --max-time 2 "http://127.0.0.1:$port/" 2>/dev/null; then
    echo "port $port is already in use; stop what is listening there or set APP_PORT/DEVTOOLS_PORT" >&2
    exit 1
  fi
done

profile="$(mktemp -d)"
app_pid=""
browser_pid=""
cleanup() {
  for pid in $browser_pid $app_pid; do
    kill "$pid" 2>/dev/null || true
    # The browser still writes to its profile until it is gone.
    wait "$pid" 2>/dev/null || true
  done
  rm -rf "$profile"
}
trap cleanup EXIT

"$TARGET_DIR/debug/tasks-pending" sandbox --listen "127.0.0.1:$APP_PORT" \
  --static-dir "$ROOT/frontend/dist" >"$profile/sandbox.log" 2>&1 &
app_pid=$!
"$browser" --headless=new --disable-gpu --no-first-run --hide-scrollbars \
  --remote-debugging-port="$DEVTOOLS_PORT" --user-data-dir="$profile/browser" about:blank \
  >"$profile/browser.log" 2>&1 &
browser_pid=$!

# Waits for `url` to answer, giving up at once if the process `pid` (whose
# output is in `log`) has exited.
wait_for() {
  local url=$1 pid=$2 log=$3
  for _ in $(seq 1 100); do
    if curl -fsS -o /dev/null "$url" 2>/dev/null; then
      return 0
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "the process serving $url exited:" >&2
      cat "$log" >&2
      return 1
    fi
    sleep 0.2
  done
  echo "nothing answered at $url:" >&2
  cat "$log" >&2
  return 1
}
wait_for "http://127.0.0.1:$APP_PORT/healthz" "$app_pid" "$profile/sandbox.log"
wait_for "http://127.0.0.1:$DEVTOOLS_PORT/json" "$browser_pid" "$profile/browser.log"

node "$ROOT/scripts/screenshots.mjs" "http://127.0.0.1:$APP_PORT/" "$DEVTOOLS_PORT" "$OUT"
