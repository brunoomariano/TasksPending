#!/usr/bin/env bash
# Packages a release bundle: dist/tasks-pending-<version>-<target>.tar.gz and
# its .sha256. The bundle holds the self-contained executable, the example
# config and env file, the service files and install.sh.
#
#   TARGET      Rust target triple (default: this machine's)
#   SKIP_BUILD  1 to package an executable already built with frontend assets
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
NAME="tasks-pending-$VERSION-$TARGET"
DIST="$ROOT/dist/$NAME"

if [[ ${SKIP_BUILD:-0} != 1 ]]; then
  npm --prefix "$ROOT/frontend" run build
  cargo build --release --locked --target "$TARGET" -p tasks-pending
fi

test -f "$ROOT/frontend/dist/index.html"

rm -rf "$DIST" "$ROOT/dist/$NAME.tar.gz" "$ROOT/dist/$NAME.tar.gz.sha256"
mkdir -p "$DIST/bin" "$DIST/scripts" "$DIST/docs"

install -m 0755 "$TARGET_DIR/$TARGET/release/tasks-pending" "$DIST/bin/tasks-pending"
cp -R "$ROOT/packaging" "$DIST/packaging"
install -m 0755 "$ROOT/scripts/install.sh" "$DIST/scripts/install.sh"
install -m 0644 "$ROOT/config.example.toml" "$DIST/config.example.toml"
install -m 0644 "$ROOT/README.md" "$DIST/README.md"
install -m 0644 "$ROOT/CHANGELOG.md" "$DIST/CHANGELOG.md"
install -m 0644 "$ROOT/LICENSE" "$DIST/LICENSE"
install -m 0644 "$ROOT/docs/install.md" "$ROOT/docs/operations.md" "$DIST/docs/"

tar -C "$ROOT/dist" -czf "$ROOT/dist/$NAME.tar.gz" "$NAME"
(
  cd "$ROOT/dist"
  if command -v sha256sum >/dev/null; then
    sha256sum "$NAME.tar.gz" >"$NAME.tar.gz.sha256"
  else
    shasum -a 256 "$NAME.tar.gz" >"$NAME.tar.gz.sha256"
  fi
)

echo "Built dist/$NAME.tar.gz"
