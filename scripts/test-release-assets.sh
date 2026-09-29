#!/usr/bin/env bash
# Checks the stable latest/download aliases prepared for a GitHub release.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
ARTIFACTS="$WORK/artifacts"
VERSION=0.3.0
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$ARTIFACTS"
for target in \
  x86_64-unknown-linux-gnu \
  aarch64-unknown-linux-gnu \
  aarch64-apple-darwin \
  x86_64-apple-darwin; do
  printf '%s\n' "$target" >"$ARTIFACTS/tasks-pending-$VERSION-$target.tar.gz"
done
printf 'arch package\n' >"$ARTIFACTS/tasks-pending-bin-$VERSION-1-x86_64.pkg.tar.zst"

"$ROOT/scripts/prepare-release-assets.sh" "$VERSION" "$ARTIFACTS"

for target in \
  x86_64-unknown-linux-gnu \
  aarch64-unknown-linux-gnu \
  aarch64-apple-darwin \
  x86_64-apple-darwin; do
  asset="tasks-pending-$target.tar.gz"
  cmp "$ARTIFACTS/tasks-pending-$VERSION-$target.tar.gz" "$ARTIFACTS/$asset"
  (
    cd "$ARTIFACTS"
    sha256sum -c "$asset.sha256"
  )
done

cmp "$ARTIFACTS/tasks-pending-bin-$VERSION-1-x86_64.pkg.tar.zst" \
  "$ARTIFACTS/tasks-pending-bin-x86_64.pkg.tar.zst"
(
  cd "$ARTIFACTS"
  sha256sum -c tasks-pending-bin-x86_64.pkg.tar.zst.sha256
  sha256sum -c SHA256SUMS
)
test -x "$ARTIFACTS/tasks-pending-install.sh"
cmp "$ROOT/scripts/install-release.sh" "$ARTIFACTS/tasks-pending-install.sh"
