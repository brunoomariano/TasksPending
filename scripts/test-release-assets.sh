#!/usr/bin/env bash
# Checks the stable latest/download aliases prepared for a GitHub release.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
ARTIFACTS="$WORK/artifacts"
VERSION=0.6.0
trap 'rm -rf "$WORK"' EXIT

TARGET=x86_64-unknown-linux-gnu

mkdir -p "$ARTIFACTS"
printf '%s\n' "$TARGET" >"$ARTIFACTS/tasks-pending-$VERSION-$TARGET.tar.gz"
printf 'arch package\n' >"$ARTIFACTS/tasks-pending-bin-$VERSION-1-x86_64.pkg.tar.zst"

"$ROOT/scripts/prepare-release-assets.sh" "$VERSION" "$ARTIFACTS"

asset="tasks-pending-$TARGET.tar.gz"
cmp "$ARTIFACTS/tasks-pending-$VERSION-$TARGET.tar.gz" "$ARTIFACTS/$asset"
(
  cd "$ARTIFACTS"
  sha256sum -c "$asset.sha256"
)

cmp "$ARTIFACTS/tasks-pending-bin-$VERSION-1-x86_64.pkg.tar.zst" \
  "$ARTIFACTS/tasks-pending-bin-x86_64.pkg.tar.zst"
(
  cd "$ARTIFACTS"
  sha256sum -c tasks-pending-bin-x86_64.pkg.tar.zst.sha256
  sha256sum -c tasks-pending-install.sh.sha256
  sha256sum -c SHA256SUMS
)
test -x "$ARTIFACTS/tasks-pending-install.sh"
cmp "$ROOT/scripts/install-release.sh" "$ARTIFACTS/tasks-pending-install.sh"
