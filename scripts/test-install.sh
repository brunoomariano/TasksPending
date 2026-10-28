#!/usr/bin/env bash
# Exercises the installer with a fake executable and systemctl, so running
# it does not alter the machine running the test.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
APP_ID="io.github.brunoomariano.TasksPending"
trap 'rm -rf "${WORK:?}"' EXIT

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

# A stand-in executable that records how it is called.
make_fake_binary() {
  cat >"$1" <<'EOF2'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$BINARY_LOG"
EOF2
  chmod +x "$1"
}

run_case() {
  local override=${1:-none} env_file=${2:-missing}
  local case_name="$override-$env_file"
  local custom_command
  local home="$WORK/$case_name/home"
  local release_bin="$WORK/$case_name/release-bin"
  local output

  mkdir -p "$home" "$release_bin"
  if [[ $env_file == present ]]; then
    install -Dm 0600 /dev/stdin "$home/.config/tasks-pending/env" <<'EOF2'
TODOIST_API_TOKEN=preserve-this-token
EOF2
  fi
  # A custom port lives in a drop-in, which reinstalling must leave alone.
  custom_command="ExecStart=$home/.local/bin/tasks-pending serve --listen 127.0.0.1:8090"
  if [[ $override == custom ]]; then
    mkdir -p "$home/.config/systemd/user/tasks-pending.service.d"
    printf '[Service]\nExecStart=\n%s\n' "$custom_command" \
      >"$home/.config/systemd/user/tasks-pending.service.d/override.conf"
  fi
  make_fake_binary "$release_bin/tasks-pending"

  output="$(
    HOME="$home" \
      XDG_CONFIG_HOME="$home/.config" \
      PREFIX="$home/.local" \
      RELEASE_BIN="$release_bin" \
      SKIP_BUILD=1 \
      BINARY_LOG="$WORK/$case_name/binary.log" \
      "$ROOT/scripts/install.sh"
  )"

  test -x "$home/.local/bin/tasks-pending"
  test ! -L "$home/.local/bin/tasks-pending"
  grep -Fqx "ExecStart=$home/.local/bin/tasks-pending serve --listen 127.0.0.1:61000" \
    "$home/.config/systemd/user/tasks-pending.service"
  # The weather widget runs Omarchy's commands, which may live outside /usr/bin.
  grep -Eq '^Environment=PATH=.*:/usr/share/omarchy/bin:%h/\.local/share/omarchy/bin$' \
    "$home/.config/systemd/user/tasks-pending.service"
  # The desktop entry opens the installed executable and names the icon.
  grep -Fqx "Exec=$home/.local/bin/tasks-pending" "$home/.local/share/applications/$APP_ID.desktop"
  grep -Fqx "Icon=$APP_ID" "$home/.local/share/applications/$APP_ID.desktop"
  test -f "$home/.local/share/icons/hicolor/scalable/apps/$APP_ID.svg"
  test -f "$home/.local/share/icons/hicolor/256x256/apps/$APP_ID.png"
  test -f "$home/.local/share/tasks-pending/config.example.toml"
  test -f "$home/.local/share/tasks-pending/env.example"
  if [[ $override == custom ]]; then
    grep -Fqx "$custom_command" \
      "$home/.config/systemd/user/tasks-pending.service.d/override.conf"
  fi
  # Only the one executable is installed, and it was not run.
  test "$(find "$home/.local/bin" -mindepth 1 | wc -l)" -eq 1
  test ! -e "$WORK/$case_name/binary.log"
  assert_contains "$output" "Daemon:  $home/.local/bin/tasks-pending setup"
  assert_contains "$output" "Open:    $home/.local/bin/tasks-pending"
  if [[ $env_file == missing ]]; then
    assert_contains "$output" "Warning: token file is missing; authenticated sources will be unavailable."
  else
    assert_not_contains "$output" "Warning: token file is missing; authenticated sources will be unavailable."
    grep -Fqx 'TODOIST_API_TOKEN=preserve-this-token' "$home/.config/tasks-pending/env"
    test "$(stat -c %a "$home/.config/tasks-pending/env")" = 600
  fi
}

run_case
run_case custom
run_case none present

# --start hands over to `tasks-pending setup`, which starts the daemon.
run_start_case() {
  local home="$WORK/start/home"
  local release_bin="$WORK/start/release-bin"
  local log="$WORK/start/binary.log"

  mkdir -p "$home" "$release_bin"
  make_fake_binary "$release_bin/tasks-pending"
  HOME="$home" \
    XDG_CONFIG_HOME="$home/.config" \
    PREFIX="$home/.local" \
    RELEASE_BIN="$release_bin" \
    SKIP_BUILD=1 \
    BINARY_LOG="$log" \
    "$ROOT/scripts/install.sh" --start >/dev/null
  grep -Fqx -- "setup" "$log"
}
run_start_case

# INSTALL_LINK=1 links the executable instead of copying it, so a later
# `tasks-pending upgrade` can swap the release directory under the link.
run_link_case() {
  local home="$WORK/link/home"
  local lib="$WORK/link/lib/tasks-pending"

  mkdir -p "$home" "$lib/bin"
  make_fake_binary "$lib/bin/tasks-pending"
  HOME="$home" \
    XDG_CONFIG_HOME="$home/.config" \
    PREFIX="$home/.local" \
    RELEASE_BIN="$lib/bin" \
    INSTALL_LINK=1 \
    SKIP_BUILD=1 \
    BINARY_LOG="$WORK/link/binary.log" \
    "$ROOT/scripts/install.sh" >/dev/null
  test -L "$home/.local/bin/tasks-pending"
  test "$(readlink "$home/.local/bin/tasks-pending")" = "$lib/bin/tasks-pending"
}
run_link_case

# BIN_DIR puts the executable, and what points at it, somewhere other than
# PREFIX/bin; the release installer passes TASKS_PENDING_BIN_DIR this way.
run_bin_dir_case() {
  local home="$WORK/bin-dir/home"
  local release_bin="$WORK/bin-dir/release-bin"

  mkdir -p "$home" "$release_bin"
  make_fake_binary "$release_bin/tasks-pending"
  HOME="$home" \
    XDG_CONFIG_HOME="$home/.config" \
    PREFIX="$home/.local" \
    BIN_DIR="$home/tools" \
    RELEASE_BIN="$release_bin" \
    SKIP_BUILD=1 \
    BINARY_LOG="$WORK/bin-dir/binary.log" \
    "$ROOT/scripts/install.sh" >/dev/null
  test -x "$home/tools/tasks-pending"
  test ! -e "$home/.local/bin/tasks-pending"
  grep -Fqx "ExecStart=$home/tools/tasks-pending serve --listen 127.0.0.1:61000" \
    "$home/.config/systemd/user/tasks-pending.service"
  grep -Fqx "Exec=$home/tools/tasks-pending" "$home/.local/share/applications/$APP_ID.desktop"
}
run_bin_dir_case

# DESTDIR stages a system install: the service file goes under the prefix,
# nothing runs, and the home directory is untouched.
run_destdir_case() {
  local home="$WORK/destdir/home"
  local stage="$WORK/destdir/stage"
  local release_bin="$WORK/destdir/release-bin"

  mkdir -p "$home" "$release_bin"
  make_fake_binary "$release_bin/tasks-pending"
  HOME="$home" \
    PREFIX=/usr \
    DESTDIR="$stage" \
    RELEASE_BIN="$release_bin" \
    SKIP_BUILD=1 \
    BINARY_LOG="$WORK/destdir/binary.log" \
    "$ROOT/scripts/install.sh" >/dev/null
  test -x "$stage/usr/bin/tasks-pending"
  grep -Fqx "ExecStart=/usr/bin/tasks-pending serve --listen 127.0.0.1:61000" \
    "$stage/usr/lib/systemd/user/tasks-pending.service"
  grep -Fqx "Exec=/usr/bin/tasks-pending" "$stage/usr/share/applications/$APP_ID.desktop"
  test -f "$stage/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"
  test ! -e "$home/.config"
  test ! -e "$WORK/destdir/binary.log"
}
run_destdir_case

# --uninstall removes what was installed and nothing else.
run_uninstall_case() {
  local home="$WORK/uninstall/home"
  local release_bin="$WORK/uninstall/release-bin"
  local fake_bin="$WORK/uninstall/fake-bin"

  mkdir -p "$home" "$release_bin" "$fake_bin"
  make_fake_binary "$release_bin/tasks-pending"
  install -Dm 0600 /dev/stdin "$home/.config/tasks-pending/env" <<'EOF2'
TODOIST_API_TOKEN=preserve-this-token
EOF2
  ln -s "$(type -P true)" "$fake_bin/systemctl"
  for flag in "" --uninstall; do
    # shellcheck disable=SC2086
    HOME="$home" \
      XDG_CONFIG_HOME="$home/.config" \
      PATH="$fake_bin:$PATH" \
      PREFIX="$home/.local" \
      RELEASE_BIN="$release_bin" \
      SKIP_BUILD=1 \
      BINARY_LOG="$WORK/uninstall/binary.log" \
      "$ROOT/scripts/install.sh" $flag >/dev/null
  done
  test ! -e "$home/.local/bin/tasks-pending"
  test ! -e "$home/.local/share/applications/$APP_ID.desktop"
  test ! -e "$home/.local/share/icons/hicolor/scalable/apps/$APP_ID.svg"
  test ! -e "$home/.config/systemd/user/tasks-pending.service"
  grep -Fqx 'TODOIST_API_TOKEN=preserve-this-token' "$home/.config/tasks-pending/env"
}
run_uninstall_case
