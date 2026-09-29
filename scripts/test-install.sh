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

assert_not_contains() {
  local text=$1 unexpected=$2
  [[ $text != *"$unexpected"* ]] || {
    printf 'installer output must not contain: %s\n' "$unexpected" >&2
    exit 1
  }
}

run_case() {
  local state=$1 expected=$2 override=${3:-none} env_file=${4:-missing}
  local case_name="$state-$override-$env_file"
  local home="$WORK/$case_name/home"
  local release_bin="$WORK/$case_name/release-bin"
  local fake_bin="$WORK/$case_name/fake-bin"
  local output

  mkdir -p "$home" "$release_bin" "$fake_bin"
  if [[ $env_file == present ]]; then
    install -Dm 0600 /dev/stdin "$home/.config/tasks-pending/env" <<'EOF'
TODOIST_API_TOKEN=preserve-this-token
EOF
  fi
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
  grep -Fqx "ExecStart=$home/.local/bin/tasks-pending serve --listen 127.0.0.1:61000" \
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
  if [[ $env_file == missing ]]; then
    assert_contains "$output" "Warning: token file is missing; authenticated sources will be unavailable."
  else
    assert_not_contains "$output" "Warning: token file is missing; authenticated sources will be unavailable."
    grep -Fqx 'TODOIST_API_TOKEN=preserve-this-token' "$home/.config/tasks-pending/env"
    test "$(stat -c %a "$home/.config/tasks-pending/env")" = 600
  fi
}

run_case true "systemctl --user restart tasks-pending" standard
run_case false "systemctl --user enable --now tasks-pending"
run_case true "update the remaining pending-api command" custom
run_case false "systemctl --user enable --now tasks-pending" none present

run_start_case() {
  local state=$1 expected=$2 override=${3:-none}
  local home="$WORK/start-$state-$override/home"
  local release_bin="$WORK/start-$state-$override/release-bin"
  local fake_bin="$WORK/start-$state-$override/fake-bin"
  local log="$WORK/start-$state-$override/systemctl.log"
  local output
  local status=0

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
  cat >"$fake_bin/systemctl" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$SYSTEMCTL_LOG"
if [[ $* == "--user is-active --quiet tasks-pending.service" ]]; then
  exit "$SYSTEMCTL_ACTIVE"
fi
EOF
  chmod +x "$fake_bin/systemctl"

  if output="$(
    HOME="$home" \
      XDG_CONFIG_HOME="$home/.config" \
      PATH="$fake_bin:$PATH" \
      PREFIX="$home/.local" \
      RELEASE_BIN="$release_bin" \
      SKIP_BUILD=1 \
      SYSTEMCTL_ACTIVE="$state" \
      SYSTEMCTL_LOG="$log" \
      "$ROOT/scripts/install.sh" --start
  )"; then
    status=0
  else
    status=$?
  fi

  if [[ $expected == error ]]; then
    test "$status" -ne 0
    ! grep -Fq -- '--user enable --now tasks-pending' "$log"
    ! grep -Fq -- '--user restart tasks-pending' "$log"
  else
    test "$status" -eq 0
    grep -Fqx -- "$expected" "$log"
    assert_contains "$output" "Warning: token file is missing; authenticated sources will be unavailable."
  fi
}

run_start_case 1 "--user enable --now tasks-pending"
run_start_case 0 "--user restart tasks-pending"
run_start_case 0 error custom
