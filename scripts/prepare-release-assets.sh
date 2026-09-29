#!/usr/bin/env bash
# Adds stable latest/download aliases and their checksums to release artifacts.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION=${1:?usage: prepare-release-assets.sh VERSION ARTIFACTS_DIR}
ARTIFACTS=${2:?usage: prepare-release-assets.sh VERSION ARTIFACTS_DIR}
TARGETS=(
  x86_64-unknown-linux-gnu
  aarch64-unknown-linux-gnu
  aarch64-apple-darwin
  x86_64-apple-darwin
)

test -d "$ARTIFACTS"
for target in "${TARGETS[@]}"; do
  versioned="$ARTIFACTS/tasks-pending-$VERSION-$target.tar.gz"
  stable="$ARTIFACTS/tasks-pending-$target.tar.gz"
  test -f "$versioned"
  cp "$versioned" "$stable"
  (
    cd "$ARTIFACTS"
    sha256sum "$(basename "$stable")" >"$(basename "$stable").sha256"
  )
done

versioned_package="$ARTIFACTS/tasks-pending-bin-$VERSION-1-x86_64.pkg.tar.zst"
stable_package="$ARTIFACTS/tasks-pending-bin-x86_64.pkg.tar.zst"
test -f "$versioned_package"
cp "$versioned_package" "$stable_package"
(
  cd "$ARTIFACTS"
  sha256sum "$(basename "$stable_package")" >"$(basename "$stable_package").sha256"
)

install -m 0755 "$ROOT/scripts/install-release.sh" "$ARTIFACTS/tasks-pending-install.sh"
(
  cd "$ARTIFACTS"
  sha256sum ./*.tar.gz ./*.pkg.tar.zst tasks-pending-install.sh | sed 's| \./| |' >SHA256SUMS
)
