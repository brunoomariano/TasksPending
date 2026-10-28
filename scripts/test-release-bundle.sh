#!/usr/bin/env bash
# Checks the Linux tarball that users download: its layout, the CLI and the
# daemon's API. Run after scripts/release-local.sh for a native target.
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
# Every member is a plain file or a directory: the installers refuse
# anything else, so a link in the bundle would make every install fail.
tar -tzvf "$ARCHIVE" >"$WORK/members"
if odd="$(cut -c1 "$WORK/members" | grep -v '^[-d]$')"; then
  echo "the archive holds members that are not plain files or directories ($odd):" >&2
  grep -v '^[-d]' "$WORK/members" >&2
  exit 1
fi
tar -xzf "$ARCHIVE" -C "$WORK"
BUNDLE="$WORK/$NAME"

test -x "$BUNDLE/bin/tasks-pending"
# One executable, nothing else.
test "$(find "$BUNDLE/bin" -mindepth 1 | wc -l)" -eq 1
for path in CHANGELOG.md LICENSE README.md config.example.toml docs/install.md docs/operations.md \
  packaging/systemd/tasks-pending.service \
  packaging/applications/io.github.brunoomariano.TasksPending.desktop \
  packaging/icons/tasks-pending.svg packaging/icons/tasks-pending.png scripts/install.sh; do
  test -e "$BUNDLE/$path"
done

"$BUNDLE/bin/tasks-pending" --help >/dev/null
"$BUNDLE/bin/tasks-pending" app --help >/dev/null
"$BUNDLE/bin/tasks-pending" serve --help >/dev/null
"$BUNDLE/bin/tasks-pending" sandbox --help >/dev/null
"$BUNDLE/bin/tasks-pending" setup --help >/dev/null
# Every flag docs/install.md names for `upgrade` is in the release.
upgrade_help="$("$BUNDLE/bin/tasks-pending" upgrade --help)"
for flag in --check --version --allow-downgrade; do
  [[ $upgrade_help == *"$flag"* ]] || {
    echo "tasks-pending upgrade has no $flag" >&2
    exit 1
  }
done
"$BUNDLE/bin/tasks-pending" --version | grep -Fq "$VERSION"
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

curl -fsS "http://127.0.0.1:$PORT/healthz" | grep -Fq "\"version\":\"$VERSION\""
curl -fsS "http://127.0.0.1:$PORT/api/v1/snapshot" | grep -Fq '"boards"'
curl -fsS "http://127.0.0.1:$PORT/" | grep -Fq 'TasksPending'
kill -0 "$pid" 2>/dev/null
