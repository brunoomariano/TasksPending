#!/bin/sh
# Installs or updates TasksPending from a GitHub release, for this user.
#
#   curl --proto '=https' --tlsv1.2 -fsSL https://github.com/brunoomariano/TasksPending/releases/latest/download/tasks-pending-install.sh | sh
#
# On Arch Linux (or a distribution derived from it, per os-release's ID_LIKE)
# it installs the release's pacman package. Elsewhere it downloads
# tasks-pending-x86_64-unknown-linux-gnu.tar.gz, checks it against the
# release's published .sha256, unpacks it under ~/.local/lib/tasks-pending and
# links ~/.local/bin/tasks-pending to it. A missing or mismatched checksum
# stops the install before anything is unpacked. Either way it ends with
# `tasks-pending setup`, which installs the desktop entry and the user service
# and starts (or restarts) the daemon.
#
# Environment (all optional):
#   TASKS_PENDING_VERSION           release to install, e.g. 0.6.0 (default: latest)
#   TASKS_PENDING_BIN_DIR           where the `tasks-pending` link goes (default: ~/.local/bin)
#   TASKS_PENDING_LIB_DIR           where the release is unpacked (default: ~/.local/lib/tasks-pending)
#   TASKS_PENDING_RELEASE_BASE_URL  releases URL prefix, https only, the one `tasks-pending
#                                   upgrade` reads too (default: GitHub). The checksum comes
#                                   from the same place as the archive, so it proves the
#                                   download arrived whole, not who published it: point it
#                                   only at an origin you trust.
#   TASKS_PENDING_REPOSITORY        GitHub repository (default: brunoomariano/TasksPending)
#   TASKS_PENDING_OS_RELEASE        os-release file to read the distribution from
#
# Everything runs inside main(), called on the last line, so a download cut
# short runs nothing.
set -eu
umask 022

say() {
  printf '%s\n' "$*"
}

fail() {
  printf 'tasks-pending-install: %s\n' "$*" >&2
  exit 1
}

# download <url> <dest>: fails on any HTTP error, so a missing file never
# leaves a page of HTML behind; https only, redirects included.
download() {
  curl --proto '=https' --proto-redir '=https' --tlsv1.2 \
    --fail --location --silent --show-error \
    --connect-timeout 15 --max-time 900 --retry 3 --retry-delay 1 \
    --output "$2" "$1"
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    fail "no SHA-256 tool found (sha256sum or shasum)"
  fi
}

# verify <file> <checksum-file> <asset-name>: the checksum file names the
# asset and holds its hash; both must match.
verify() {
  expected="$(awk -v asset="$3" '$2 == asset || $2 == "*" asset {print $1}' "$2" | head -n 1)"
  [ -n "$expected" ] || fail "$2 does not name $3"
  actual="$(sha256_of "$1")"
  [ "$actual" = "$expected" ] || fail "checksum mismatch for $3: expected $expected, got $actual"
}

# check_archive <archive> <asset-name> <work-dir>: refuses an archive with
# an absolute or parent-relative member, or with any member that is not a
# plain file or directory, before tar writes anything. The names come from
# one listing and the types from another, paired line by line, so a name
# with spaces or a link's target cannot confuse the check. The listings go
# to files and are read back here, not in a pipeline's subshell, so `fail`
# ends the install.
check_archive() {
  tar -tzf "$1" >"$3/names" || fail "cannot list $2"
  tar -tzvf "$1" >"$3/verbose" || fail "cannot list $2"
  cut -c1 "$3/verbose" >"$3/kinds"
  [ -s "$3/names" ] || fail "refusing $2: the archive is empty"
  [ "$(wc -l <"$3/names")" -eq "$(wc -l <"$3/kinds")" ] ||
    fail "refusing $2: the two listings of the archive do not agree"
  exec 3<"$3/kinds"
  while IFS= read -r name; do
    IFS= read -r kind <&3 || kind=""
    case "$kind" in
    -|d) ;;
    *) fail "refusing $2: member $name is not a plain file or directory" ;;
    esac
    case "/$name/" in
    //*|*/../*) fail "refusing $2: member $name escapes the archive" ;;
    esac
  done <"$3/names"
  exec 3<&-
}

# download_checked <base-url> <asset> <work-dir>: downloads the asset and
# its .sha256 and prints the verified file's path.
download_checked() {
  file="$3/$2"
  download "$1/$2" "$file"
  download "$1/$2.sha256" "$file.sha256"
  verify "$file" "$file.sha256" "$2"
  printf '%s\n' "$file"
}

install_bundle() {
  base="$1" work="$2" bin_dir="$3" lib_dir="$4"
  [ "$(id -u)" -ne 0 ] ||
    fail "run this as the user who will use TasksPending, not as root: it installs under your home and starts a user service"
  if [ -e "$lib_dir" ] && [ ! -x "$lib_dir/bin/tasks-pending" ]; then
    fail "refusing to replace $lib_dir: it does not hold a TasksPending release (TASKS_PENDING_LIB_DIR names where the release is unpacked)"
  fi
  asset="tasks-pending-x86_64-unknown-linux-gnu.tar.gz"
  archive="$(download_checked "$base" "$asset" "$work")"
  check_archive "$archive" "$asset" "$work"

  mkdir -p "$work/unpack"
  tar -xzf "$archive" -C "$work/unpack"
  bundle="$(find "$work/unpack" -mindepth 1 -maxdepth 1 -type d -name 'tasks-pending-*' | head -n 1)"
  [ -n "$bundle" ] || fail "the archive holds no tasks-pending-* directory"
  [ -x "$bundle/bin/tasks-pending" ] || fail "the archive holds no executable"
  [ -x "$bundle/scripts/install.sh" ] || fail "the archive holds no installer"
  # The release must run on this machine (its glibc and GTK) before
  # anything is replaced.
  "$bundle/bin/tasks-pending" --version >/dev/null 2>"$work/probe" ||
    fail "the new release does not run here ($(cat "$work/probe")); nothing was replaced"

  # Swap the unpacked release in. The previous one is kept aside until the
  # new one is in place and set up, and comes back if either step fails.
  mkdir -p "$(dirname "$lib_dir")" "$bin_dir"
  staged="$lib_dir.staged"
  previous="$lib_dir.previous"
  rm -rf "${staged:?}" "${previous:?}"
  mv "$bundle" "$staged"
  had_previous=0
  if [ -e "$lib_dir" ]; then
    mv "$lib_dir" "$previous"
    had_previous=1
  fi
  if ! mv "$staged" "$lib_dir"; then
    [ "$had_previous" -eq 0 ] || mv "$previous" "$lib_dir"
    fail "could not put the new release in $lib_dir"
  fi

  # The bundle's installer links the executable and installs the desktop
  # entry, the icon and the user service, then runs `tasks-pending setup`.
  # It is part of the release whose checksum was just verified, and runs
  # with the same trust as the executable it installs.
  if ! PREFIX="$(dirname "$bin_dir")" BIN_DIR="$bin_dir" INSTALL_LINK=1 RELEASE_BIN="$lib_dir/bin" SKIP_BUILD=1 \
    "$lib_dir/scripts/install.sh" --start; then
    if [ "$had_previous" -eq 1 ]; then
      mv "$lib_dir" "$staged"
      mv "$previous" "$lib_dir"
      rm -rf "${staged:?}"
      fail "the bundle's installer failed; the previous release is back in $lib_dir"
    fi
    fail "the bundle's installer failed; the new release is in $lib_dir, run $lib_dir/scripts/install.sh --start"
  fi
  rm -rf "${previous:?}"
  say "installed $("$bin_dir/tasks-pending" --version 2>/dev/null || echo tasks-pending) to $lib_dir"
}

install_arch_package() {
  base="$1" work="$2"
  command -v pacman >/dev/null 2>&1 || fail "pacman is required to install the Arch package"
  package="$(download_checked "$base" "tasks-pending-bin-x86_64.pkg.tar.zst" "$work")"
  # pacman asks before installing when a terminal is there to answer (the
  # script's own stdin is the download); without one it must not wait.
  if (exec </dev/tty) 2>/dev/null; then
    confirm=""
    input=/dev/tty
  else
    confirm="--noconfirm"
    input=/dev/null
  fi
  if [ "$(id -u)" -eq 0 ]; then
    say "running: pacman -U $confirm $package"
    pacman -U $confirm "$package" <"$input"
  elif command -v sudo >/dev/null 2>&1; then
    say "running: sudo pacman -U $confirm $package"
    # The user's shell opens the terminal (or /dev/null) for pacman; root
    # never needs to.
    # shellcheck disable=SC2024
    sudo pacman -U $confirm "$package" <"$input"
  else
    fail "sudo is required to install the Arch package"
  fi
  # The package's own executable, wherever pacman put it, not whatever
  # `tasks-pending` on PATH points at.
  executable="$(pacman -Qlq tasks-pending-bin | grep -m 1 '/bin/tasks-pending$' || true)"
  [ -n "$executable" ] && [ -x "$executable" ] || fail "the package installed no tasks-pending executable"
  "$executable" setup
}

main() {
  repository="${TASKS_PENDING_REPOSITORY:-brunoomariano/TasksPending}"
  release_base="${TASKS_PENDING_RELEASE_BASE_URL:-https://github.com/$repository/releases}"
  release_base="${release_base%/}"
  case "$release_base" in
  https://*) ;;
  *) fail "TASKS_PENDING_RELEASE_BASE_URL must start with https:// (it is $release_base)" ;;
  esac
  version="${TASKS_PENDING_VERSION:-latest}"
  bin_dir="${TASKS_PENDING_BIN_DIR:-$HOME/.local/bin}"
  lib_dir="${TASKS_PENDING_LIB_DIR:-$HOME/.local/lib/tasks-pending}"
  os_release="${TASKS_PENDING_OS_RELEASE:-/etc/os-release}"

  case "$version" in
  latest) base="$release_base/latest/download" ;;
  v*) base="$release_base/download/$version" ;;
  *) base="$release_base/download/v$version" ;;
  esac

  os="$(uname -s)"
  machine="$(uname -m)"
  case "$os:$machine" in
  Linux:x86_64) ;;
  *) fail "no TasksPending release is available for $os on $machine (Linux x86_64 only)" ;;
  esac

  # Arch itself, or a distribution built on it (Omarchy, Manjaro, ...):
  # os-release is read, never run.
  release_id=""
  release_like=""
  if [ -r "$os_release" ]; then
    release_id="$(sed -n 's/^ID=//p' "$os_release" | head -n 1 | tr -d '"')"
    release_like="$(sed -n 's/^ID_LIKE=//p' "$os_release" | head -n 1 | tr -d '"')"
  fi
  arch_based=0
  case " $release_id $release_like " in
  *" arch "*) arch_based=1 ;;
  esac

  work="$(mktemp -d "${TMPDIR:-/tmp}/tasks-pending.XXXXXX")"
  trap 'rm -rf "${work:?}"' EXIT INT TERM

  say "fetching TasksPending ($version) from $base"
  if [ "$arch_based" -eq 1 ]; then
    install_arch_package "$base" "$work"
  else
    install_bundle "$base" "$work" "$bin_dir" "$lib_dir"
  fi
}

main "$@"
