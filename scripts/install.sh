#!/usr/bin/env bash
# Builds a release and installs it under PREFIX (default ~/.local):
#
#   PREFIX/bin/tasks-pending
#   PREFIX/share/tasks-pending/{config.example.toml,env.example,icon}
#   Linux: ~/.config/systemd/user/tasks-pending.service
#   macOS: ~/Library/LaunchAgents/com.github.brunoomariano.tasks-pending.plist
#
# It never touches your config, env file or tokens, and never starts the
# service; it prints the next steps. `scripts/install.sh --uninstall` removes
# what it installed (your config stays).
#
# DESTDIR stages a system install (packaging): with DESTDIR set, the service
# file goes to DESTDIR/PREFIX/lib/systemd/user instead of your home.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
DESTDIR="${DESTDIR:-}"
BIN="$DESTDIR$PREFIX/bin"
SHARE="$DESTDIR$PREFIX/share/tasks-pending"
OS="$(uname -s)"
PLIST_NAME="com.github.brunoomariano.tasks-pending.plist"

if [[ -n $DESTDIR ]]; then
  UNIT_DIR="$DESTDIR$PREFIX/lib/systemd/user"
else
  UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
fi
AGENT_DIR="$HOME/Library/LaunchAgents"

uninstall() {
  rm -f "$BIN/tasks-pending" "$BIN/pending-api" "$BIN/pending-tui"
  rm -rf "$SHARE"
  if [[ $OS == Linux ]]; then
    if [[ -z $DESTDIR ]] && command -v systemctl >/dev/null; then
      systemctl --user disable --now tasks-pending.service 2>/dev/null || true
    fi
    rm -f "$UNIT_DIR/tasks-pending.service"
    if [[ -z $DESTDIR ]] && command -v systemctl >/dev/null; then
      systemctl --user daemon-reload || true
    fi
  elif [[ $OS == Darwin ]]; then
    launchctl bootout "gui/$(id -u)/${PLIST_NAME%.plist}" 2>/dev/null || true
    rm -f "$AGENT_DIR/$PLIST_NAME"
  fi
  echo "Removed TasksPending from $PREFIX (your config in ~/.config/tasks-pending stays)."
}

# The 0.1 service points to the retired pending-api executable. Migrate only
# the standard three-argument shape, preserving its tokens and listen address.
migrate_legacy_launchd_service() {
  local plist=$1
  local plistbuddy=/usr/libexec/PlistBuddy
  local program flag listen staged

  [[ -x $plistbuddy ]] || return 1
  program="$($plistbuddy -c "Print :ProgramArguments:0" "$plist" 2>/dev/null)" || return 1
  flag="$($plistbuddy -c "Print :ProgramArguments:1" "$plist" 2>/dev/null)" || return 1
  listen="$($plistbuddy -c "Print :ProgramArguments:2" "$plist" 2>/dev/null)" || return 1
  [[ $program == "$PREFIX/bin/pending-api" && $flag == --listen && -n $listen ]] || return 1
  if "$plistbuddy" -c "Print :ProgramArguments:3" "$plist" >/dev/null 2>&1; then
    return 1
  fi

  staged="$plist.migrate.$$"
  cp "$plist" "$staged" || return 1
  if ! "$plistbuddy" \
    -c "Delete :ProgramArguments" \
    -c "Add :ProgramArguments array" \
    -c "Add :ProgramArguments:0 string $PREFIX/bin/tasks-pending" \
    -c "Add :ProgramArguments:1 string serve" \
    -c "Add :ProgramArguments:2 string --listen" \
    -c "Add :ProgramArguments:3 string $listen" \
    "$staged"; then
    rm -f "$staged"
    return 1
  fi
  if ! mv "$staged" "$plist"; then
    rm -f "$staged"
    return 1
  fi
}

# `systemctl --user edit tasks-pending` stores custom ports in a drop-in. The
# documented 0.1 form needs the new subcommand as well as the new executable.
migrate_legacy_systemd_overrides() {
  local directory="$UNIT_DIR/tasks-pending.service.d"
  local override staged line listen changed

  for override in "$directory"/*.conf; do
    [[ -f $override ]] || continue
    grep -Fqx 'ExecStart=' "$override" || continue

    staged="$override.migrate.$$"
    changed=0
    while IFS= read -r line || [[ -n $line ]]; do
      if [[ $line == "ExecStart=$PREFIX/bin/pending-api --listen "* ]]; then
        listen="${line#"ExecStart=$PREFIX/bin/pending-api --listen "}"
        if [[ -n $listen && $listen != *[[:space:]]* ]]; then
          printf 'ExecStart=%s/bin/tasks-pending serve --listen %s\n' "$PREFIX" "$listen"
          changed=1
          continue
        fi
      fi
      printf '%s\n' "$line"
    done <"$override" >"$staged"

    if ((changed)); then
      mv "$staged" "$override"
      echo "Updated $(basename "$override") for tasks-pending and kept its listen address."
    else
      rm -f "$staged"
    fi
    if grep -Fq "ExecStart=$PREFIX/bin/pending-api" "$override"; then
      legacy_command_remaining=1
    fi
  done
}

if [[ ${1:-} == --uninstall ]]; then
  uninstall
  exit 0
fi

# From an unpacked release bundle (no Cargo.toml): install what it ships.
if [[ ! -f $ROOT/Cargo.toml ]]; then
  SKIP_BUILD=1
  RELEASE_BIN="${RELEASE_BIN:-$ROOT/bin}"
fi

# Build unless told to reuse existing artifacts.
if [[ ${SKIP_BUILD:-0} != 1 ]]; then
  npm --prefix "$ROOT/frontend" ci --no-audit --no-fund
  npm --prefix "$ROOT/frontend" run build
  cargo build --release --locked --manifest-path "$ROOT/Cargo.toml" -p tasks-pending
fi
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
RELEASE_BIN="${RELEASE_BIN:-$TARGET_DIR/release}"

install -d "$BIN" "$SHARE"
install -m 0755 "$RELEASE_BIN/tasks-pending" "$BIN/tasks-pending"
install -m 0644 "$ROOT/config.example.toml" "$SHARE/config.example.toml"
install -m 0644 "$ROOT/packaging/env.example" "$SHARE/env.example"
install -m 0644 "$ROOT/packaging/icons/tasks-pending.png" "$SHARE/tasks-pending.png"
install -m 0644 "$ROOT/packaging/icons/tasks-pending.svg" "$SHARE/tasks-pending.svg"
legacy_command_remaining=0

case $OS in
Linux)
  install -d "$UNIT_DIR"
  sed "s|@BINDIR@|$PREFIX/bin|g" "$ROOT/packaging/systemd/tasks-pending.service" \
    >"$UNIT_DIR/tasks-pending.service"
  chmod 0644 "$UNIT_DIR/tasks-pending.service"
  migrate_legacy_systemd_overrides
  service_was_active=0
  if [[ -z $DESTDIR ]] && command -v systemctl >/dev/null; then
    if systemctl --user is-active --quiet tasks-pending.service >/dev/null 2>&1; then
      service_was_active=1
    fi
    systemctl --user daemon-reload || true
  fi
  if ((legacy_command_remaining)); then
    next="update the remaining pending-api command in $UNIT_DIR/tasks-pending.service.d, then systemctl --user daemon-reload && systemctl --user restart tasks-pending"
  elif ((service_was_active)); then
    next="systemctl --user restart tasks-pending"
  else
    next="systemctl --user enable --now tasks-pending"
  fi
  ;;
Darwin)
  install -d "$AGENT_DIR" "$HOME/Library/Logs"
  plist="$AGENT_DIR/$PLIST_NAME"
  launchd_restart="launchctl bootout gui/\$(id -u)/${PLIST_NAME%.plist} 2>/dev/null || true; launchctl bootstrap gui/\$(id -u) ~/Library/LaunchAgents/$PLIST_NAME"
  if [[ -f $plist ]] && migrate_legacy_launchd_service "$plist"; then
    chmod 0600 "$plist"
    echo "Updated $PLIST_NAME for tasks-pending and kept its tokens and listen address."
    next="$launchd_restart"
  else
    # The installed plist holds user-managed tokens and settings.
    if [[ -f $plist ]]; then
      if grep -Fq "$PREFIX/bin/pending-api" "$plist"; then
        legacy_command_remaining=1
      fi
      plist="$plist.new"
      echo "Kept your $PLIST_NAME; the new one is $plist."
      next="copy ProgramArguments from $PLIST_NAME.new into $PLIST_NAME, then $launchd_restart"
    else
      next="$launchd_restart"
    fi
    sed -e "s|@BINDIR@|$PREFIX/bin|g" -e "s|@LOGDIR@|$HOME/Library/Logs|g" \
      "$ROOT/packaging/launchd/$PLIST_NAME" >"$plist"
    chmod 0600 "$plist"
  fi
  ;;
*)
  next="$PREFIX/bin/tasks-pending serve"
  ;;
esac

if ((legacy_command_remaining)); then
  echo "Kept the legacy pending-api and frontend until its custom command is updated."
else
  rm -f "$BIN/pending-api" "$BIN/pending-tui"
  rm -rf "$SHARE/frontend"
fi

[[ -n $DESTDIR ]] && exit 0

cat <<EOF

Installed TasksPending to $PREFIX.

Next steps (see docs/install.md):
  1. Config:  mkdir -p ~/.config/tasks-pending &&
              cp -n $SHARE/config.example.toml ~/.config/tasks-pending/config.toml
  2. Tokens:  cp -n $SHARE/env.example ~/.config/tasks-pending/env &&
              chmod 600 ~/.config/tasks-pending/env   (then fill it in)
  3. Daemon:  $next
  4. Open:    http://127.0.0.1:8080   (or run tasks-pending tui)
EOF
case ":$PATH:" in
*":$PREFIX/bin:"*) ;;
*) echo "Note: $PREFIX/bin is not on your PATH." ;;
esac
