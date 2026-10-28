#!/usr/bin/env bash
# Exercises the public release installer against local download fixtures:
# no network, no changes outside a temporary directory.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
FIXTURES="$WORK/fixtures"
FAKE_BIN="$WORK/fake/bin"
VERSION=0.6.0
TARGET=x86_64-unknown-linux-gnu
ASSET="tasks-pending-$TARGET.tar.gz"
trap 'rm -rf "${WORK:?}"' EXIT

fail() {
  printf '%s\n' "$*" >&2
  exit 1
}

# expect_failure <name> <message> <command...>: the command fails and says
# why with <message>.
expect_failure() {
  local name=$1 message=$2 output
  shift 2
  if output="$("$@" 2>&1)"; then
    fail "$name: the installer did not fail"
  fi
  [[ $output == *"$message"* ]] || fail "$name: expected '$message' in: $output"
}

mkdir -p "$FIXTURES" "$FAKE_BIN"

# A release bundle whose executable and installer record how they are run.
# The installer fails when FAIL_INSTALL is set, like one whose `setup`
# could not start the daemon.
make_bundle() {
  local bundle="$WORK/build/tasks-pending-$VERSION-$TARGET"

  mkdir -p "$bundle/bin" "$bundle/scripts"
  cat >"$bundle/bin/tasks-pending" <<'EOF2'
#!/bin/sh
printf '%s\n' "$*" >>"$BINARY_LOG"
[ "$1" = --version ] && echo "tasks-pending 0.6.0"
exit 0
EOF2
  # Like the real installer, it links BIN_DIR to the bundle's executable.
  cat >"$bundle/scripts/install.sh" <<'EOF2'
#!/bin/sh
printf 'install.sh %s PREFIX=%s BIN_DIR=%s INSTALL_LINK=%s RELEASE_BIN=%s\n' "$*" "$PREFIX" "$BIN_DIR" "$INSTALL_LINK" "$RELEASE_BIN" >>"$BINARY_LOG"
[ -z "${FAIL_INSTALL:-}" ] || exit 7
mkdir -p "$BIN_DIR"
ln -sfn "$RELEASE_BIN/tasks-pending" "$BIN_DIR/tasks-pending"
EOF2
  chmod +x "$bundle/bin/tasks-pending" "$bundle/scripts/install.sh"
  tar -C "$WORK/build" -czf "$FIXTURES/$ASSET" "${bundle##*/}"
  (
    cd "$FIXTURES"
    sha256sum "$ASSET" >"$ASSET.sha256"
    # The same archive published with the wrong hash, and with a
    # checksum file that names another asset.
    sed 's/^./0/' "$ASSET.sha256" >mismatch.sha256
    sed "s/ $ASSET\$/ other.tar.gz/" "$ASSET.sha256" >wrong-asset.sha256
  )
}
make_bundle

# A bundle whose executable cannot run here (a newer glibc, say).
make_broken_bundle() {
  local bundle="$WORK/broken/tasks-pending-$VERSION-$TARGET"
  mkdir -p "$bundle/bin" "$bundle/scripts"
  printf '#!/bin/sh\necho "version GLIBC_9.9 not found" >&2\nexit 127\n' >"$bundle/bin/tasks-pending"
  printf '#!/bin/sh\nexit 0\n' >"$bundle/scripts/install.sh"
  chmod +x "$bundle/bin/tasks-pending" "$bundle/scripts/install.sh"
  tar -C "$WORK/broken" -czf "$FIXTURES/broken.tar.gz" "${bundle##*/}"
}
make_broken_bundle

# Archives that look like a release but hold a member that must be
# refused: one that would write outside the install directory, one with
# an absolute path, a symbolic link and a hard link.
make_bad_bundles() {
  local dir="$WORK/bad" bundle="tasks-pending-$VERSION-$TARGET"
  mkdir -p "$dir/$bundle/bin" "$dir/outside"
  : >"$dir/$bundle/bin/tasks-pending"
  : >"$dir/outside/file"
  # GNU tar strips a leading ../ or / unless told otherwise, so these
  # members are written with -P to keep them.
  tar -C "$dir" -P -czf "$FIXTURES/escaping.tar.gz" "$bundle" "$bundle/../outside/file"
  tar -C "$dir" -P -czf "$FIXTURES/absolute.tar.gz" "$bundle" "$dir/outside/file"
  ln -s /etc/passwd "$dir/$bundle/link"
  tar -C "$dir" -czf "$FIXTURES/symlink.tar.gz" "$bundle"
  rm "$dir/$bundle/link"
  ln "$dir/$bundle/bin/tasks-pending" "$dir/$bundle/hard"
  tar -C "$dir" -czf "$FIXTURES/hardlink.tar.gz" "$bundle"
}
make_bad_bundles

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
cat >"$FAKE_BIN/id" <<'EOF2'
#!/bin/sh
printf '%s\n' "${MOCK_UID:-1000}"
EOF2
# curl copies the fixture named by the URL's last part and logs the call.
# ARCHIVE_FIXTURE serves another archive as the bundle, CHECKSUM_FIXTURE
# another checksum file for it.
cat >"$FAKE_BIN/curl" <<'EOF2'
#!/bin/sh
printf 'curl %s\n' "$*" >>"$CURL_LOG"
output=""
url=""
while [ "$#" -gt 0 ]; do
  case $1 in
  -o|--output)
    output=$2
    shift 2
    ;;
  --retry|--retry-delay|--proto|--proto-redir|--connect-timeout|--max-time)
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
asset=${url##*/}
case $asset in
"$BUNDLE_ASSET")
  cp "$FIXTURES/${ARCHIVE_FIXTURE:-$BUNDLE_ASSET}" "$output"
  ;;
"$BUNDLE_ASSET.sha256")
  if [ -n "${CHECKSUM_FIXTURE:-}" ]; then
    cp "$FIXTURES/$CHECKSUM_FIXTURE" "$output"
  elif [ -n "${ARCHIVE_FIXTURE:-}" ]; then
    (cd "$FIXTURES" && sha256sum "$ARCHIVE_FIXTURE" | sed "s/ $ARCHIVE_FIXTURE\$/ $BUNDLE_ASSET/") >"$output"
  else
    cp "$FIXTURES/$asset" "$output"
  fi
  ;;
*)
  cp "$FIXTURES/$asset" "$output" 2>/dev/null || exit 22
  ;;
esac
EOF2
cat >"$FAKE_BIN/sudo" <<'EOF2'
#!/bin/sh
printf 'sudo %s\n' "$*" >>"$PACMAN_LOG"
EOF2
# pacman installs nothing; asked where the package's files are, it names
# the stand-in executable.
cat >"$FAKE_BIN/pacman" <<'EOF2'
#!/bin/sh
case $1 in
-Qlq) printf '%s\n' /usr/share/doc/tasks-pending/README.md "$FAKE_BIN/tasks-pending" ;;
*) printf 'pacman %s\n' "$*" >>"$PACMAN_LOG" ;;
esac
EOF2
cat >"$FAKE_BIN/tasks-pending" <<'EOF2'
#!/bin/sh
printf 'package %s\n' "$*" >>"$BINARY_LOG"
EOF2
chmod +x "$FAKE_BIN"/*

# run_installer <home> <env...>: runs the installer with the fakes on PATH
# and the downloads pointed at the fixtures. A script of its own, so it
# can also run without a terminal (setsid) or under one (script).
export ROOT FIXTURES FAKE_BIN ASSET WORK
cat >"$WORK/run-installer.sh" <<'EOF2'
#!/usr/bin/env bash
set -euo pipefail
home=$1
shift
mkdir -p "$home"
HOME="$home" \
  PATH="$FAKE_BIN:$PATH" \
  BUNDLE_ASSET="$ASSET" \
  BINARY_LOG="$home/binary.log" \
  CURL_LOG="$home/curl.log" \
  PACMAN_LOG="$home/pacman.log" \
  TASKS_PENDING_RELEASE_BASE_URL="${RELEASE_BASE_URL:-https://example.test/releases}" \
  TASKS_PENDING_OS_RELEASE="${OS_RELEASE:-$WORK/non-arch-os-release}" \
  MOCK_UNAME_S="${MOCK_UNAME_S:-Linux}" \
  MOCK_UNAME_M="${MOCK_UNAME_M:-x86_64}" \
  "$@" \
  sh "$ROOT/scripts/install-release.sh"
EOF2
chmod +x "$WORK/run-installer.sh"
run_installer() {
  "$WORK/run-installer.sh" "$@"
}

# A plain Linux install unpacks the release, links it and runs the bundle's
# installer with --start, which hands over to `tasks-pending setup`. Every
# download is https only, redirects included, and bounded in time.
home="$WORK/linux"
output="$(run_installer "$home" env)"
lib="$home/.local/lib/tasks-pending"
test -x "$lib/bin/tasks-pending"
test -x "$lib/scripts/install.sh"
test ! -e "$lib.staged"
test ! -e "$lib.previous"
grep -Fq "install.sh --start PREFIX=$home/.local BIN_DIR=$home/.local/bin INSTALL_LINK=1 RELEASE_BIN=$lib/bin" "$home/binary.log"
grep -Fqx -- "--version" "$home/binary.log"
grep -Fq "https://example.test/releases/latest/download/$ASSET" "$home/curl.log"
grep -Fq "https://example.test/releases/latest/download/$ASSET.sha256" "$home/curl.log"
grep -Fq -- "--proto =https --proto-redir =https" "$home/curl.log"
grep -Fq -- "--connect-timeout 15 --max-time 900" "$home/curl.log"
[[ $output == *"installed tasks-pending 0.6.0 to $lib"* ]] || fail "unexpected output: $output"

# Running it again replaces the release and leaves no staging directories.
: >"$lib/marker"
run_installer "$home" env >/dev/null
test ! -e "$lib/marker"
test ! -e "$lib.staged"
test ! -e "$lib.previous"

# When the bundle's installer fails, the previous release comes back and
# the failure is reported.
: >"$lib/marker"
expect_failure "installer failure" "the previous release is back in $lib" \
  run_installer "$home" env FAIL_INSTALL=1
test -e "$lib/marker"
test ! -e "$lib.staged"
test ! -e "$lib.previous"

# Without a previous release the new one stays, and the message says what
# to run.
home="$WORK/first-failure"
expect_failure "first install failure" "run $home/.local/lib/tasks-pending/scripts/install.sh --start" \
  run_installer "$home" env FAIL_INSTALL=1
test -x "$home/.local/lib/tasks-pending/bin/tasks-pending"

# A pinned version downloads from that release's directory; a trailing
# slash on the releases location is not doubled.
home="$WORK/pinned"
run_installer "$home" env TASKS_PENDING_VERSION=0.6.0 >/dev/null
grep -Fq "https://example.test/releases/download/v0.6.0/$ASSET" "$home/curl.log"
run_installer "$home" env TASKS_PENDING_VERSION=v0.6.0 >/dev/null
grep -Fq "https://example.test/releases/download/v0.6.0/$ASSET.sha256" "$home/curl.log"
RELEASE_BASE_URL="https://example.test/releases/" run_installer "$home" env >/dev/null
grep -Fq " https://example.test/releases/latest/download/$ASSET" "$home/curl.log"
! grep -Fq "releases//latest" "$home/curl.log"

# A plain-text releases location is refused before anything is fetched.
home="$WORK/plain"
RELEASE_BASE_URL="http://example.test/releases" \
  expect_failure "plain-text location" "must start with https://" run_installer "$home" env
test ! -e "$home/curl.log"

# Custom directories are honoured, and the executable's directory is
# passed on as it is, not rebuilt from the prefix.
home="$WORK/dirs"
run_installer "$home" env TASKS_PENDING_LIB_DIR="$home/opt/tp" TASKS_PENDING_BIN_DIR="$home/opt/bin" >/dev/null
test -x "$home/opt/tp/bin/tasks-pending"
test -L "$home/opt/bin/tasks-pending"
grep -Fq "PREFIX=$home/opt BIN_DIR=$home/opt/bin INSTALL_LINK=1 RELEASE_BIN=$home/opt/tp/bin" "$home/binary.log"

# A release directory that holds something else is not replaced.
home="$WORK/occupied"
mkdir -p "$home/.local/lib/tasks-pending/photos"
expect_failure "occupied directory" "does not hold a TasksPending release" run_installer "$home" env
test -d "$home/.local/lib/tasks-pending/photos"

# A checksum that does not match, or that names another asset, stops the
# install before anything is unpacked.
home="$WORK/mismatch"
expect_failure "checksum mismatch" "checksum mismatch for $ASSET" \
  run_installer "$home" env CHECKSUM_FIXTURE=mismatch.sha256
test ! -e "$home/.local/lib/tasks-pending"
test ! -e "$home/binary.log"
expect_failure "wrong asset" "does not name $ASSET" \
  run_installer "$home" env CHECKSUM_FIXTURE=wrong-asset.sha256
test ! -e "$home/.local/lib/tasks-pending"

# Archives with a member that escapes the directory, an absolute path, a
# symbolic link or a hard link are refused by name, before unpacking.
home="$WORK/bad-members"
expect_failure "escaping member" "member tasks-pending-$VERSION-$TARGET/../outside/file escapes the archive" \
  run_installer "$home" env ARCHIVE_FIXTURE=escaping.tar.gz
expect_failure "absolute member" "member $WORK/bad/outside/file escapes the archive" \
  run_installer "$home" env ARCHIVE_FIXTURE=absolute.tar.gz
expect_failure "symbolic link" "member tasks-pending-$VERSION-$TARGET/link is not a plain file or directory" \
  run_installer "$home" env ARCHIVE_FIXTURE=symlink.tar.gz
# (tar stores whichever of the two linked names it meets second as the
# link, so only the kind of the refused member is certain.)
expect_failure "hard link" "is not a plain file or directory" \
  run_installer "$home" env ARCHIVE_FIXTURE=hardlink.tar.gz
test ! -e "$home/.local/lib/tasks-pending"
test ! -e "$home/binary.log"

# A release that cannot run here replaces nothing.
home="$WORK/broken"
run_installer "$home" env >/dev/null
: >"$home/.local/lib/tasks-pending/marker"
expect_failure "broken release" "does not run here (version GLIBC_9.9 not found)" \
  run_installer "$home" env ARCHIVE_FIXTURE=broken.tar.gz
test -e "$home/.local/lib/tasks-pending/marker"
test ! -e "$home/.local/lib/tasks-pending.staged"

# Other platforms are refused; `amd64` is not a name uname gives on Linux.
for platform in "Linux aarch64" "Linux amd64" "Darwin arm64" "Linux s390x"; do
  read -r os machine <<<"$platform"
  home="$WORK/platform-$machine"
  MOCK_UNAME_S="$os" MOCK_UNAME_M="$machine" \
    expect_failure "$platform" "no TasksPending release is available" run_installer "$home" env
done

# The bundle is for a user, not root.
home="$WORK/root"
expect_failure "root" "not as root" run_installer "$home" env MOCK_UID=0
test ! -e "$home/.local/lib/tasks-pending"

# os-release is read, never executed.
malicious_os_release="$WORK/malicious-os-release"
marker="$WORK/os-release-executed"
printf 'ID=arch; touch %s\n' "$marker" >"$malicious_os_release"
home="$WORK/os-release"
OS_RELEASE="$malicious_os_release" run_installer "$home" env >/dev/null
test ! -e "$marker" || fail "the installer executed os-release contents"

# On Arch Linux the package is installed with pacman and set up through
# the executable the package installed. Without a terminal to answer
# pacman's questions it runs with --noconfirm.
printf 'ID="arch"\n' >"$WORK/arch-os-release"
home="$WORK/arch"
output="$(OS_RELEASE="$WORK/arch-os-release" setsid -w "$WORK/run-installer.sh" "$home" env </dev/null)"
grep -Eq '^sudo pacman -U --noconfirm .*/tasks-pending-bin-x86_64\.pkg\.tar\.zst$' "$home/pacman.log"
grep -Fqx 'package setup' "$home/binary.log"
[[ $output == *"running: sudo pacman -U --noconfirm "* ]] || fail "the command was not shown: $output"
test ! -e "$home/.local/lib/tasks-pending"

# With a terminal, pacman asks as usual.
if command -v script >/dev/null 2>&1; then
  home="$WORK/arch-tty"
  OS_RELEASE="$WORK/arch-os-release" script -qec "$WORK/run-installer.sh '$home' env" /dev/null >/dev/null 2>&1 ||
    fail "the installer failed under a terminal"
  grep -Eq '^sudo pacman -U .*/tasks-pending-bin-x86_64\.pkg\.tar\.zst$' "$home/pacman.log"
  ! grep -Fq -- '--noconfirm' "$home/pacman.log"
fi

# A distribution built on Arch (ID_LIKE) gets the package too; as root,
# pacman runs without sudo.
printf 'ID=omarchy\nID_LIKE=arch\n' >"$WORK/omarchy-os-release"
home="$WORK/omarchy"
OS_RELEASE="$WORK/omarchy-os-release" setsid -w "$WORK/run-installer.sh" "$home" env MOCK_UID=0 </dev/null >/dev/null
grep -Eq '^pacman -U --noconfirm .*/tasks-pending-bin-x86_64\.pkg\.tar\.zst$' "$home/pacman.log"
grep -Fqx 'package setup' "$home/binary.log"
