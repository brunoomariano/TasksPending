#!/usr/bin/env bash
# Builds a release and installs it under PREFIX (default ~/.local):
#
#   PREFIX/bin/tasks-pending
#   PREFIX/share/tasks-pending/{config.example.toml,env.example}
#   PREFIX/share/applications/io.github.brunoomariano.TasksPending.desktop
#   PREFIX/share/icons/hicolor/{scalable,256x256}/apps/io.github.brunoomariano.TasksPending.*
#   ~/.config/systemd/user/tasks-pending.service
#
# It never touches your config, env file or tokens. By default it prints the
# next steps; `--start` runs `tasks-pending setup`, which starts a new daemon
# or restarts an active one. `scripts/install.sh --uninstall` removes what it
# installed (your config stays).
#
# DESTDIR stages a system install (packaging): with DESTDIR set, the service
# file goes to DESTDIR/PREFIX/lib/systemd/user instead of your home.
#
# From an unpacked release bundle (no Cargo.toml) nothing is built: the
# bundle's executable is installed. INSTALL_LINK=1 links PREFIX/bin to it
# instead of copying, so `tasks-pending upgrade` can swap the bundle.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
DESTDIR="${DESTDIR:-}"
BIN="$DESTDIR$PREFIX/bin"
SHARE="$DESTDIR$PREFIX/share/tasks-pending"
APPS="$DESTDIR$PREFIX/share/applications"
ICONS="$DESTDIR$PREFIX/share/icons/hicolor"
APP_ID="io.github.brunoomariano.TasksPending"
ENV_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/tasks-pending/env"

if [[ -n $DESTDIR ]]; then
  UNIT_DIR="$DESTDIR$PREFIX/lib/systemd/user"
else
  UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
fi
START_SERVICE=0

uninstall() {
  if [[ -z $DESTDIR ]] && command -v systemctl >/dev/null; then
    systemctl --user disable --now tasks-pending.service 2>/dev/null || true
  fi
  rm -f "${BIN:?}/tasks-pending"
  rm -rf "${SHARE:?}"
  rm -f "${APPS:?}/${APP_ID:?}.desktop"
  rm -f "${ICONS:?}/scalable/apps/${APP_ID:?}.svg" "${ICONS:?}/256x256/apps/${APP_ID:?}.png"
  rm -f "${UNIT_DIR:?}/tasks-pending.service"
  if [[ -z $DESTDIR ]] && command -v systemctl >/dev/null; then
    systemctl --user daemon-reload || true
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

# Build unless told to reuse an existing executable.
if [[ ${SKIP_BUILD:-0} != 1 ]]; then
  cargo build --release --locked --manifest-path "$ROOT/Cargo.toml" -p tasks-pending
fi
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
RELEASE_BIN="${RELEASE_BIN:-$TARGET_DIR/release}"

install -d "$BIN" "$SHARE" "$APPS" "$ICONS/scalable/apps" "$ICONS/256x256/apps" "$UNIT_DIR"
if [[ ${INSTALL_LINK:-0} == 1 ]]; then
  ln -sfn "$RELEASE_BIN/tasks-pending" "$BIN/tasks-pending"
else
  install -m 0755 "$RELEASE_BIN/tasks-pending" "$BIN/tasks-pending"
fi
install -m 0644 "$ROOT/config.example.toml" "$SHARE/config.example.toml"
install -m 0644 "$ROOT/packaging/env.example" "$SHARE/env.example"
install -m 0644 "$ROOT/packaging/icons/tasks-pending.svg" "$ICONS/scalable/apps/$APP_ID.svg"
install -m 0644 "$ROOT/packaging/icons/tasks-pending.png" "$ICONS/256x256/apps/$APP_ID.png"
sed "s|@BINDIR@|$PREFIX/bin|g" "$ROOT/packaging/applications/$APP_ID.desktop" \
  >"$APPS/$APP_ID.desktop"
chmod 0644 "$APPS/$APP_ID.desktop"
sed "s|@BINDIR@|$PREFIX/bin|g" "$ROOT/packaging/systemd/tasks-pending.service" \
  >"$UNIT_DIR/tasks-pending.service"
chmod 0644 "$UNIT_DIR/tasks-pending.service"

[[ -n $DESTDIR ]] && exit 0

if [[ ! -f $ENV_FILE ]]; then
  cat <<EOT
Warning: token file is missing; authenticated sources will be unavailable.
Create $ENV_FILE from $SHARE/env.example, fill in the required values, then restart the daemon.
EOT
fi

if ((START_SERVICE)); then
  exec "$BIN/tasks-pending" setup
fi

cat <<EOT

Installed TasksPending to $PREFIX.

Next steps (see docs/install.md):
  1. Config:  mkdir -p ~/.config/tasks-pending &&
              cp -n $SHARE/config.example.toml ~/.config/tasks-pending/config.toml
  2. Tokens:  cp -n $SHARE/env.example ~/.config/tasks-pending/env &&
              chmod 600 ~/.config/tasks-pending/env   (then fill it in)
  3. Daemon:  $PREFIX/bin/tasks-pending setup
  4. Open:    $PREFIX/bin/tasks-pending  (or TasksPending in the app launcher)
EOT
case ":$PATH:" in
*":$PREFIX/bin:"*) ;;
*) echo "Note: $PREFIX/bin is not on your PATH." ;;
esac
