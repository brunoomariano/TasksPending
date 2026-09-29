#!/usr/bin/env bash
# Creates an Omarchy web app (a launcher that opens the dashboard in its own
# window) with the TasksPending icon. Needs the daemon running at
# http://127.0.0.1:8080 (see docs/install.md).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
URL="${TASKS_PENDING_URL:-http://127.0.0.1:8080}"
ICON="$ROOT/packaging/icons/tasks-pending.png"
[[ -f $ICON ]] || ICON="${PREFIX:-$HOME/.local}/share/tasks-pending/tasks-pending.png"

if ! command -v omarchy-webapp-install >/dev/null; then
  echo "omarchy-webapp-install not found: this target is for Omarchy." >&2
  echo "Elsewhere, open $URL in your browser (or install it as an app from there)." >&2
  exit 1
fi

omarchy-webapp-install "TasksPending" "$URL" "$ICON"
echo "Created the TasksPending web app; find it in the app launcher."
