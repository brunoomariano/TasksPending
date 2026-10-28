#!/usr/bin/env bash
# Adds stable latest/download aliases and their checksums to release artifacts.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION=${1:?usage: prepare-release-assets.sh VERSION ARTIFACTS_DIR}
ARTIFACTS=${2:?usage: prepare-release-assets.sh VERSION ARTIFACTS_DIR}
TARGETS=(
  x86_64-unknown-linux-gnu
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

# The package's release number is the PKGBUILD's, not always 1.
pkgrel="$(sed -n 's/^pkgrel=//p' "$ROOT/packaging/arch/PKGBUILD" | head -n 1)"
test -n "$pkgrel"
versioned_package="$ARTIFACTS/tasks-pending-bin-$VERSION-$pkgrel-x86_64.pkg.tar.zst"
stable_package="$ARTIFACTS/tasks-pending-bin-x86_64.pkg.tar.zst"
test -f "$versioned_package"
cp "$versioned_package" "$stable_package"
(
  cd "$ARTIFACTS"
  # Both names get a checksum, as the bundles do.
  for package in "$versioned_package" "$stable_package"; do
    sha256sum "$(basename "$package")" >"$(basename "$package").sha256"
  done
)

install -m 0755 "$ROOT/scripts/install-release.sh" "$ARTIFACTS/tasks-pending-install.sh"
(
  cd "$ARTIFACTS"
  # The installer has its own checksum, like every other asset, so a
  # download of it can be checked before it runs.
  sha256sum tasks-pending-install.sh >tasks-pending-install.sh.sha256
  sha256sum ./*.tar.gz ./*.pkg.tar.zst tasks-pending-install.sh | sed 's| \./| |' >SHA256SUMS
)
