#!/bin/sh
# Installs or updates the latest TasksPending release for this machine.
set -eu

REPOSITORY=${TASKS_PENDING_REPOSITORY:-brunoomariano/TasksPending}
DOWNLOAD_BASE=${TASKS_PENDING_DOWNLOAD_BASE:-"https://github.com/$REPOSITORY/releases/latest/download"}
OS_RELEASE=${TASKS_PENDING_OS_RELEASE:-/etc/os-release}
WORK=$(mktemp -d "${TMPDIR:-/tmp}/tasks-pending.XXXXXX")

cleanup() {
  rm -rf "$WORK"
}
trap cleanup 0 1 2 3 15

fail() {
  printf '%s\n' "$*" >&2
  exit 1
}

download() {
  curl --fail --location --retry 3 --retry-delay 1 --output "$2" "$1"
}

verify_checksum() {
  artifact=$1
  checksum=$2

  if command -v sha256sum >/dev/null 2>&1; then
    (
      cd "$WORK"
      sha256sum -c "$(basename "$checksum")" >&2
    )
  elif command -v shasum >/dev/null 2>&1; then
    (
      cd "$WORK"
      shasum -a 256 -c "$(basename "$checksum")" >&2
    )
  else
    fail "sha256sum or shasum is required to verify the download"
  fi

  test -f "$artifact"
}

download_checked() {
  asset=$1
  artifact="$WORK/$asset"
  checksum="$artifact.sha256"

  download "$DOWNLOAD_BASE/$asset" "$artifact"
  download "$DOWNLOAD_BASE/$asset.sha256" "$checksum"
  verify_checksum "$artifact" "$checksum"
  printf '%s\n' "$artifact"
}

install_bundle() {
  archive=$(download_checked "tasks-pending-$1.tar.gz")
  tar -xzf "$archive" -C "$WORK"
  bundle=$(find "$WORK" -mindepth 1 -maxdepth 1 -type d -name 'tasks-pending-*' -print -quit)

  test -n "$bundle" || fail "release archive did not contain an installation directory"
  test -x "$bundle/scripts/install.sh" || fail "release archive did not contain its installer"
  "$bundle/scripts/install.sh" --start
}

restart_linux_service() {
  command -v systemctl >/dev/null 2>&1 || fail "systemctl is required to start TasksPending"
  systemctl --user daemon-reload
  if systemctl --user is-active --quiet tasks-pending; then
    systemctl --user restart tasks-pending
  else
    systemctl --user enable --now tasks-pending
  fi
}

install_arch_package() {
  package=$(download_checked "tasks-pending-bin-x86_64.pkg.tar.zst")
  command -v pacman >/dev/null 2>&1 || fail "pacman is required to install the Arch package"

  if [ "$(id -u)" -eq 0 ]; then
    pacman -U "$package"
  elif command -v sudo >/dev/null 2>&1; then
    sudo pacman -U "$package"
  else
    fail "sudo is required to install the Arch package"
  fi

  restart_linux_service
}

os=$(uname -s)
machine=$(uname -m)
release_id=""
if [ "$os" = Linux ] && [ -r "$OS_RELEASE" ]; then
  release_id=$(sed -n 's/^ID=//p' "$OS_RELEASE" | head -n 1)
  release_id=${release_id#\"}
  release_id=${release_id%\"}
fi

case "$os:$machine" in
Linux:x86_64 | Linux:amd64)
  if [ "$release_id" = arch ]; then
    install_arch_package
  else
    install_bundle x86_64-unknown-linux-gnu
  fi
  ;;
Linux:aarch64 | Linux:arm64)
  install_bundle aarch64-unknown-linux-gnu
  ;;
Darwin:arm64)
  install_bundle aarch64-apple-darwin
  ;;
Darwin:x86_64)
  install_bundle x86_64-apple-darwin
  ;;
*)
  fail "no TasksPending release is available for $os on $machine"
  ;;
esac

printf '%s\n' "TasksPending is running at http://127.0.0.1:61000"
