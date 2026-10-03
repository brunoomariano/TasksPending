#!/usr/bin/env bash
# Builds a release and installs it under PREFIX (default ~/.local):
#
#   PREFIX/bin/tasks-pending
#   PREFIX/share/tasks-pending/{config.example.toml,env.example,icon}
#   Linux: ~/.config/systemd/user/tasks-pending.service
#   macOS: ~/Library/LaunchAgents/com.github.brunoomariano.tasks-pending.plist
#
# It never touches your config, env file or tokens. By default it prints the
# next steps; `--start` starts a new daemon or restarts an active one.
# `scripts/install.sh --uninstall` removes what it installed (your config
# stays).
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
ENV_FILE="$HOME/.config/tasks-pending/env"

if [[ -n $DESTDIR ]]; then
  UNIT_DIR="$DESTDIR$PREFIX/lib/systemd/user"
else
  UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
fi
AGENT_DIR="$HOME/Library/LaunchAgents"
START_SERVICE=0

uninstall() {
  rm -f "$BIN/tasks-pending"
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

case ${1:-} in
"") ;;
--start) START_SERVICE=1 ;;
--uninstall)
  uninstall
  exit 0
  ;;
*)
  echo "usage: $0 [--start|--uninstall]" >&2
  exit 2
  ;;
esac

if ((START_SERVICE)) && [[ -n $DESTDIR ]]; then
  echo "--start cannot be used with DESTDIR" >&2
  exit 2
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

if [[ $OS == Linux && -z $DESTDIR && ! -f $ENV_FILE ]]; then
  cat <<EOF
Warning: token file is missing; authenticated sources will be unavailable.
Create $ENV_FILE from $SHARE/env.example, fill in the required values, then restart the daemon.
EOF
fi

case $OS in
Linux)
  install -d "$UNIT_DIR"
  sed "s|@BINDIR@|$PREFIX/bin|g" "$ROOT/packaging/systemd/tasks-pending.service" \
    >"$UNIT_DIR/tasks-pending.service"
  chmod 0644 "$UNIT_DIR/tasks-pending.service"
  service_was_active=0
  if [[ -z $DESTDIR ]] && command -v systemctl >/dev/null; then
    if systemctl --user is-active --quiet tasks-pending.service >/dev/null 2>&1; then
      service_was_active=1
    fi
    systemctl --user daemon-reload || true
  fi
  if ((service_was_active)); then
    next="systemctl --user restart tasks-pending"
  else
    next="systemctl --user enable --now tasks-pending"
  fi
  ;;
Darwin)
  install -d "$AGENT_DIR" "$HOME/Library/Logs"
  plist="$AGENT_DIR/$PLIST_NAME"
  launchd_restart="launchctl bootout gui/\$(id -u)/${PLIST_NAME%.plist} 2>/dev/null || true; launchctl bootstrap gui/\$(id -u) ~/Library/LaunchAgents/$PLIST_NAME"
  # The installed plist holds user-managed tokens and settings.
  if [[ -f $plist ]]; then
    plist="$plist.new"
    echo "Kept your $PLIST_NAME; the new one is $plist."
    next="compare $PLIST_NAME.new with $PLIST_NAME, then $launchd_restart"
  else
    next="$launchd_restart"
  fi
  sed -e "s|@BINDIR@|$PREFIX/bin|g" -e "s|@LOGDIR@|$HOME/Library/Logs|g" \
    "$ROOT/packaging/launchd/$PLIST_NAME" >"$plist"
  chmod 0600 "$plist"
  ;;
*)
  next="$PREFIX/bin/tasks-pending serve"
  ;;
esac

[[ -n $DESTDIR ]] && exit 0

if ((START_SERVICE)); then
  case $OS in
  Linux)
    command -v systemctl >/dev/null || {
      echo "systemctl is required to start the Linux user service" >&2
      exit 1
    }
    if ((service_was_active)); then
      systemctl --user restart tasks-pending
    else
      systemctl --user enable --now tasks-pending
    fi
    next="running at http://127.0.0.1:61000"
    ;;
  Darwin)
    launchctl bootout "gui/$(id -u)/${PLIST_NAME%.plist}" 2>/dev/null || true
    launchctl bootstrap "gui/$(id -u)" "$AGENT_DIR/$PLIST_NAME"
    next="running at http://127.0.0.1:61000"
    ;;
  *)
    echo "automatic startup is only available on Linux and macOS" >&2
    exit 1
    ;;
  esac
fi

cat <<EOF

Installed TasksPending to $PREFIX.

Next steps (see docs/install.md):
  1. Config:  mkdir -p ~/.config/tasks-pending &&
              cp -n $SHARE/config.example.toml ~/.config/tasks-pending/config.toml
  2. Tokens:  cp -n $SHARE/env.example ~/.config/tasks-pending/env &&
              chmod 600 ~/.config/tasks-pending/env   (then fill it in)
  3. Daemon:  $next
  4. Open:    http://127.0.0.1:61000  (or run tasks-pending tui)
EOF
case ":$PATH:" in
*":$PREFIX/bin:"*) ;;
*) echo "Note: $PREFIX/bin is not on your PATH." ;;
esac
