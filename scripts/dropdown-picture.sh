#!/usr/bin/env bash
# Pictures `tasks-pending sandbox` dropped down on the Omarchy desktop.
# Goes to an empty workspace, starts the sandbox there (simulated cards
# only), drops it down through its own `toggle` action (the code path the
# bar icon runs), pictures the focused monitor with grim, sends it away,
# quits the sandbox through its `quit` action and goes back to the
# workspace it started on. A TasksPending window already running is kept
# out of the picture meanwhile, and put back. Should a step fail, the
# sandbox it started is stopped, by its PID, before that.
set -euo pipefail

out=${1:?usage: dropdown-picture.sh <picture.png>}
bin=${TASKS_PENDING_BIN:-tasks-pending}
object=/io/github/brunoomariano/TasksPending

dispatch() {
  local answer
  answer=$(hyprctl dispatch "$1")
  if [ "$(printf '%s' "$answer" | tr -d '[:space:]')" != ok ]; then
    echo "Hyprland refused $1: $answer" >&2
    return 1
  fi
}

move() {
  dispatch "hl.dsp.window.move({ workspace = \"$2\", window = \"address:$1\", follow = false })"
}

back=$(hyprctl -j activeworkspace | jq -r .id)
# A TasksPending already running drops down in the same special workspace,
# at the same place, and its cards would show through the sandbox: it waits
# in another special workspace until the picture is taken.
aside=$(hyprctl -j clients |
  jq -r '.[] | select(.workspace.name == "special:tasks-pending") | .address')
# The sandbox's PID while it runs, for a step that fails before it quits.
pid=
restore() {
  local address
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    kill "$pid" || true
  fi
  for address in $aside; do
    move "$address" special:tasks-pending || true
  done
  dispatch "hl.dsp.focus({ workspace = \"$back\" })" || true
}
trap restore EXIT
for address in $aside; do
  move "$address" special:tasks-pending-aside
done
dispatch 'hl.dsp.focus({ workspace = "empty" })'
sleep 0.5

setsid "$bin" sandbox >/dev/null 2>&1 &
pid=$!

name=
for _ in $(seq 50); do
  name=$(busctl --user --unique --json=short list |
    jq -r --argjson pid "$pid" '[.[] | select(.pid == $pid)][0].name // empty')
  [ -n "$name" ] && break
  sleep 0.2
done
if [ -z "$name" ]; then
  echo "the sandbox (pid $pid) did not show up on the session bus" >&2
  exit 1
fi

action() {
  gdbus call --session --dest "$name" --object-path "$object" \
    --method org.gtk.Actions.Activate "$1" '@av []' '@a{sv} {}' >/dev/null
}

sleep 2
action toggle
sleep 2
monitor=$(hyprctl -j monitors | jq -r '.[] | select(.focused) | .name')
grim -o "$monitor" "$out"
action toggle
sleep 0.5
action quit
pid=
echo "Saved $out ($monitor)."
