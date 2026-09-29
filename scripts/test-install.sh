#!/usr/bin/env bash
# Exercises the installer with a fake systemctl, so upgrades do not alter the
# machine running the test.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

assert_contains() {
  local text=$1 expected=$2
  [[ $text == *"$expected"* ]] || {
    printf 'expected installer output to contain: %s\n' "$expected" >&2
    exit 1
  }
}

run_case() {
  local state=$1 expected=$2 override=${3:-none}
  local home="$WORK/$state-$override/home"
  local release_bin="$WORK/$state-$override/release-bin"
  local fake_bin="$WORK/$state-$override/fake-bin"
  local output

  mkdir -p "$home" "$release_bin" "$fake_bin"
  if [[ $override != none ]]; then
    mkdir -p "$home/.config/systemd/user/tasks-pending.service.d"
    printf '[Service]\nExecStart=\nExecStart=%s/bin/pending-api --listen 127.0.0.1:8090%s\n' "$home/.local" \
      "$([[ $override == custom ]] && printf ' --custom-option')" \
      >"$home/.config/systemd/user/tasks-pending.service.d/override.conf"
  fi
  install -m 0755 "$(type -P true)" "$release_bin/tasks-pending"
  if [[ $override == custom ]]; then
    install -Dm 0755 "$(type -P true)" "$home/.local/bin/pending-api"
    install -Dm 0644 /dev/null "$home/.local/share/tasks-pending/frontend/index.html"
  fi
  ln -s "$(type -P "$state")" "$fake_bin/systemctl"

  output="$(
    HOME="$home" \
      XDG_CONFIG_HOME="$home/.config" \
      PATH="$fake_bin:$PATH" \
      PREFIX="$home/.local" \
      RELEASE_BIN="$release_bin" \
      SKIP_BUILD=1 \
      "$ROOT/scripts/install.sh"
  )"

  test -x "$home/.local/bin/tasks-pending"
  grep -Fqx "ExecStart=$home/.local/bin/tasks-pending serve --listen 127.0.0.1:8080" \
    "$home/.config/systemd/user/tasks-pending.service"
  if [[ $override == standard ]]; then
    grep -Fqx "ExecStart=$home/.local/bin/tasks-pending serve --listen 127.0.0.1:8090" \
      "$home/.config/systemd/user/tasks-pending.service.d/override.conf"
    test ! -e "$home/.local/bin/pending-api"
    test ! -e "$home/.local/share/tasks-pending/frontend"
  elif [[ $override == custom ]]; then
    grep -Fqx "ExecStart=$home/.local/bin/pending-api --listen 127.0.0.1:8090 --custom-option" \
      "$home/.config/systemd/user/tasks-pending.service.d/override.conf"
    test -x "$home/.local/bin/pending-api"
    test -f "$home/.local/share/tasks-pending/frontend/index.html"
  else
    test ! -e "$home/.local/bin/pending-api"
    test ! -e "$home/.local/share/tasks-pending/frontend"
  fi
  assert_contains "$output" "Daemon:  $expected"
}

run_case true "systemctl --user restart tasks-pending" standard
run_case false "systemctl --user enable --now tasks-pending"
run_case true "update the remaining pending-api command" custom
