#!/usr/bin/env bash
# Exercises the public release installer against local download fixtures:
# no network, no changes outside a temporary directory.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
FIXTURES="$WORK/fixtures"
FAKE_BIN="$WORK/fake-bin"
VERSION=0.6.0
TARGET=x86_64-unknown-linux-gnu
trap 'rm -rf "${WORK:?}"' EXIT

fail() {
  printf '%s\n' "$*" >&2
  exit 1
}

mkdir -p "$FIXTURES" "$FAKE_BIN"

# A release bundle whose executable and installer record how they are run.
make_bundle() {
  local bundle="$WORK/build/tasks-pending-$VERSION-$TARGET"
  local asset="tasks-pending-$TARGET.tar.gz"

  mkdir -p "$bundle/bin" "$bundle/scripts"
  cat >"$bundle/bin/tasks-pending" <<'EOF2'
#!/bin/sh
printf '%s\n' "$*" >>"$BINARY_LOG"
[ "$1" = --version ] && echo "tasks-pending 0.6.0"
exit 0
EOF2
  # Like the real installer, it links PREFIX/bin to the bundle's executable.
  cat >"$bundle/scripts/install.sh" <<'EOF2'
#!/bin/sh
printf 'install.sh %s PREFIX=%s INSTALL_LINK=%s RELEASE_BIN=%s\n' "$*" "$PREFIX" "$INSTALL_LINK" "$RELEASE_BIN" >>"$BINARY_LOG"
mkdir -p "$PREFIX/bin"
ln -sfn "$RELEASE_BIN/tasks-pending" "$PREFIX/bin/tasks-pending"
EOF2
  chmod +x "$bundle/bin/tasks-pending" "$bundle/scripts/install.sh"
  tar -C "$WORK/build" -czf "$FIXTURES/$asset" "${bundle##*/}"
  (
    cd "$FIXTURES"
    sha256sum "$asset" >"$asset.sha256"
  )
}
make_bundle

# An archive with a member that would escape the install directory.
make_bad_bundle() {
  local bundle="$WORK/bad/tasks-pending-$VERSION-$TARGET"
  mkdir -p "$bundle/bin" "$WORK/bad/outside"
  : >"$bundle/bin/tasks-pending"
  : >"$WORK/bad/outside/file"
  # GNU tar strips a leading ../ unless told otherwise, so the member is
  # written with -P to keep it.
  tar -C "$WORK/bad/$(basename "$bundle")" -P -czf "$FIXTURES/escaping.tar.gz" bin "../outside/file"
  (
    cd "$FIXTURES"
    sha256sum escaping.tar.gz >escaping.tar.gz.sha256
  )
}
make_bad_bundle

touch "$FIXTURES/tasks-pending-bin-x86_64.pkg.tar.zst"
(
  cd "$FIXTURES"
  sha256sum tasks-pending-bin-x86_64.pkg.tar.zst >tasks-pending-bin-x86_64.pkg.tar.zst.sha256
)

cat >"$FAKE_BIN/uname" <<'EOF2'
#!/bin/sh
case $1 in
-s) printf '%s\n' "$MOCK_UNAME_S" ;;
-m) printf '%s\n' "$MOCK_UNAME_M" ;;
*) exit 2 ;;
esac
EOF2
# curl copies the fixture named by the URL's last part and logs the URL.
cat >"$FAKE_BIN/curl" <<'EOF2'
#!/bin/sh
output=""
url=""
while [ "$#" -gt 0 ]; do
  case $1 in
  -o|--output)
    output=$2
    shift 2
    ;;
  --retry|--retry-delay|--proto)
    shift 2
    ;;
  --tlsv1.2|--fail|--location|--silent|--show-error)
    shift
    ;;
  *)
    url=$1
    shift
    ;;
  esac
done
printf '%s\n' "$url" >>"$CURL_LOG"
asset=${url##*/}
if [ "${CORRUPT_ASSET:-}" = "$asset" ]; then
  printf 'corrupt' >"$output"
elif [ "${ESCAPING_ASSET:-}" = "$asset" ]; then
  cp "$FIXTURES/escaping.tar.gz" "$output"
elif [ "${ESCAPING_ASSET:-}.sha256" = "$asset" ]; then
  sed "s/escaping.tar.gz/${ESCAPING_ASSET}/" "$FIXTURES/escaping.tar.gz.sha256" >"$output"
else
  cp "$FIXTURES/$asset" "$output" 2>/dev/null || exit 22
fi
EOF2
cat >"$FAKE_BIN/sudo" <<'EOF2'
#!/bin/sh
printf '%s\n' "$*" >>"$PACMAN_LOG"
EOF2
cat >"$FAKE_BIN/pacman" <<'EOF2'
#!/bin/sh
exit 0
EOF2
cat >"$FAKE_BIN/tasks-pending" <<'EOF2'
#!/bin/sh
printf 'path %s\n' "$*" >>"$BINARY_LOG"
EOF2
chmod +x "$FAKE_BIN"/*

run_installer() {
  local home=$1
  shift
  mkdir -p "$home"
  HOME="$home" \
    PATH="$FAKE_BIN:$PATH" \
    FIXTURES="$FIXTURES" \
    BINARY_LOG="$home/binary.log" \
    CURL_LOG="$home/curl.log" \
    PACMAN_LOG="$home/pacman.log" \
    TASKS_PENDING_RELEASE_BASE_URL="https://example.test/releases" \
    TASKS_PENDING_OS_RELEASE="${OS_RELEASE:-$WORK/non-arch-os-release}" \
    MOCK_UNAME_S="${MOCK_UNAME_S:-Linux}" \
    MOCK_UNAME_M="${MOCK_UNAME_M:-x86_64}" \
    "$@" \
    sh "$ROOT/scripts/install-release.sh"
}

# A plain Linux install unpacks the release, links it and runs the bundle's
# installer with --start, which hands over to `tasks-pending setup`.
home="$WORK/linux"
output="$(run_installer "$home" env)"
lib="$home/.local/lib/tasks-pending"
test -x "$lib/bin/tasks-pending"
test -x "$lib/scripts/install.sh"
test ! -e "$lib.staged"
test ! -e "$lib.previous"
grep -Fq "install.sh --start PREFIX=$home/.local INSTALL_LINK=1 RELEASE_BIN=$lib/bin" "$home/binary.log"
grep -Fq "https://example.test/releases/latest/download/tasks-pending-$TARGET.tar.gz" "$home/curl.log"
grep -Fq "https://example.test/releases/latest/download/tasks-pending-$TARGET.tar.gz.sha256" "$home/curl.log"
[[ $output == *"installed tasks-pending 0.6.0 to $lib"* ]] || fail "unexpected output: $output"

# Running it again replaces the release and leaves no staging directories.
: >"$lib/marker"
run_installer "$home" env >/dev/null
test ! -e "$lib/marker"
test ! -e "$lib.staged"
test ! -e "$lib.previous"

# A pinned version downloads from that release's directory.
home="$WORK/pinned"
run_installer "$home" env TASKS_PENDING_VERSION=0.6.0 >/dev/null
grep -Fq "https://example.test/releases/download/v0.6.0/tasks-pending-$TARGET.tar.gz" "$home/curl.log"
run_installer "$home" env TASKS_PENDING_VERSION=v0.6.0 >/dev/null
grep -Fq "https://example.test/releases/download/v0.6.0/tasks-pending-$TARGET.tar.gz.sha256" "$home/curl.log"

# Custom directories are honoured.
home="$WORK/dirs"
run_installer "$home" env TASKS_PENDING_LIB_DIR="$home/opt/tp" TASKS_PENDING_BIN_DIR="$home/bin" >/dev/null
test -x "$home/opt/tp/bin/tasks-pending"
grep -Fq "PREFIX=$home INSTALL_LINK=1 RELEASE_BIN=$home/opt/tp/bin" "$home/binary.log"

# A corrupt archive is refused before anything is unpacked.
home="$WORK/corrupt"
if run_installer "$home" env CORRUPT_ASSET="tasks-pending-$TARGET.tar.gz" >/dev/null 2>&1; then
  fail "the installer accepted a corrupt archive"
fi
test ! -e "$home/.local/lib/tasks-pending"
test ! -e "$home/binary.log"

# An archive with a member that escapes the directory is refused.
home="$WORK/escaping"
if run_installer "$home" env ESCAPING_ASSET="tasks-pending-$TARGET.tar.gz" >/dev/null 2>&1; then
  fail "the installer accepted an archive with an escaping member"
fi
test ! -e "$home/.local/lib/tasks-pending"

# Other platforms are refused.
for platform in "Linux aarch64" "Darwin arm64" "Linux s390x"; do
  read -r os machine <<<"$platform"
  home="$WORK/platform-$machine"
  if MOCK_UNAME_S="$os" MOCK_UNAME_M="$machine" run_installer "$home" env >/dev/null 2>&1; then
    fail "the installer accepted $platform"
  fi
done

# os-release is read, never executed.
malicious_os_release="$WORK/malicious-os-release"
marker="$WORK/os-release-executed"
printf 'ID=arch; touch %s\n' "$marker" >"$malicious_os_release"
home="$WORK/os-release"
OS_RELEASE="$malicious_os_release" run_installer "$home" env >/dev/null
test ! -e "$marker" || fail "the installer executed os-release contents"

# On Arch Linux the package is installed with pacman and set up.
printf 'ID="arch"\n' >"$WORK/arch-os-release"
home="$WORK/arch"
OS_RELEASE="$WORK/arch-os-release" run_installer "$home" env >/dev/null
grep -Fq 'pacman -U --noconfirm ' "$home/pacman.log"
grep -Fqx 'path setup' "$home/binary.log"
test ! -e "$home/.local/lib/tasks-pending"
