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
rm -f "$BIN/pending-api" "$BIN/pending-tui"
install -m 0644 "$ROOT/config.example.toml" "$SHARE/config.example.toml"
install -m 0644 "$ROOT/packaging/env.example" "$SHARE/env.example"
install -m 0644 "$ROOT/packaging/icons/tasks-pending.png" "$SHARE/tasks-pending.png"
install -m 0644 "$ROOT/packaging/icons/tasks-pending.svg" "$SHARE/tasks-pending.svg"

case $OS in
Linux)
  install -d "$UNIT_DIR"
  sed "s|@BINDIR@|$PREFIX/bin|g" "$ROOT/packaging/systemd/tasks-pending.service" \
    >"$UNIT_DIR/tasks-pending.service"
  chmod 0644 "$UNIT_DIR/tasks-pending.service"
  if [[ -z $DESTDIR ]] && command -v systemctl >/dev/null; then
    systemctl --user daemon-reload || true
  fi
  next="systemctl --user enable --now tasks-pending"
  ;;
Darwin)
  install -d "$AGENT_DIR" "$HOME/Library/Logs"
  # The installed plist holds your tokens: never overwrite it.
  plist="$AGENT_DIR/$PLIST_NAME"
  if [[ -f $plist ]]; then
    plist="$plist.new"
    echo "Kept your $PLIST_NAME (it holds your tokens); the new one is $plist."
  fi
  sed -e "s|@BINDIR@|$PREFIX/bin|g" -e "s|@LOGDIR@|$HOME/Library/Logs|g" \
    "$ROOT/packaging/launchd/$PLIST_NAME" >"$plist"
  chmod 0600 "$plist"
  next="launchctl bootstrap gui/\$(id -u) ~/Library/LaunchAgents/$PLIST_NAME"
  ;;
*)
  next="$PREFIX/bin/tasks-pending serve"
  ;;
esac

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
