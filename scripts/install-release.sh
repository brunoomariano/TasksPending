#!/bin/sh
# Installs or updates TasksPending from a GitHub release, for this user.
#
#   curl --proto '=https' --tlsv1.2 -fsSL https://github.com/brunoomariano/TasksPending/releases/latest/download/tasks-pending-install.sh | sh
#
# On Arch Linux it installs the release's pacman package. Elsewhere it
# downloads tasks-pending-x86_64-unknown-linux-gnu.tar.gz, checks it against
# the release's published .sha256, unpacks it under ~/.local/lib/tasks-pending
# and links ~/.local/bin/tasks-pending to it. A missing or mismatched checksum
# stops the install before anything is unpacked. Either way it ends with
# `tasks-pending setup`, which installs the desktop entry and the user service
# and starts (or restarts) the daemon.
#
# Environment (all optional):
#   TASKS_PENDING_VERSION           release to install, e.g. 0.6.0 (default: latest)
#   TASKS_PENDING_BIN_DIR           where the `tasks-pending` link goes (default: ~/.local/bin)
#   TASKS_PENDING_LIB_DIR           where the release is unpacked (default: ~/.local/lib/tasks-pending)
#   TASKS_PENDING_RELEASE_BASE_URL  releases URL prefix, the one `tasks-pending upgrade`
#                                   reads too (default: GitHub). The checksum comes from
#                                   the same place as the archive, so point it only at an
#                                   origin you trust.
#   TASKS_PENDING_REPOSITORY        GitHub repository (default: brunoomariano/TasksPending)
#   TASKS_PENDING_OS_RELEASE        os-release file to read the distribution from
#
# Everything runs inside main(), called on the last line, so a download cut
# short runs nothing.
set -eu

say() {
  printf '%s\n' "$*"
}

fail() {
  printf 'tasks-pending-install: %s\n' "$*" >&2
  exit 1
}

# download <url> <dest>: fails on any HTTP error, so a missing file never
# leaves a page of HTML behind.
download() {
  curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
    --retry 3 --retry-delay 1 --output "$2" "$1"
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

# Refuses an archive with an absolute or parent-relative member, or with
# any member that is not a plain file or directory, before tar writes
# anything.
check_archive() {
  tar -tzvf "$1" | while IFS= read -r line; do
    kind="$(printf '%s' "$line" | cut -c1)"
    name="$(printf '%s' "$line" | awk '{print $NF}')"
    case "$kind" in
    -|d) ;;
    *) fail "refusing $2: member $name is not a plain file or directory" ;;
    esac
    case "/$name/" in
    //*|*/../*) fail "refusing $2: member $name escapes the archive" ;;
    esac
  done
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
  asset="tasks-pending-x86_64-unknown-linux-gnu.tar.gz"
  archive="$(download_checked "$base" "$asset" "$work")"
  check_archive "$archive" "$asset"

  mkdir -p "$work/unpack"
  tar -xzf "$archive" -C "$work/unpack"
  bundle="$(find "$work/unpack" -mindepth 1 -maxdepth 1 -type d -name 'tasks-pending-*' | head -n 1)"
  [ -n "$bundle" ] || fail "the archive holds no tasks-pending-* directory"
  [ -x "$bundle/bin/tasks-pending" ] || fail "the archive holds no executable"
  [ -x "$bundle/scripts/install.sh" ] || fail "the archive holds no installer"

  # Swap the unpacked release in; keep the previous one until the new one
  # is in place, and put it back if the swap fails halfway.
  mkdir -p "$(dirname "$lib_dir")" "$bin_dir"
  staged="$lib_dir.staged"
  previous="$lib_dir.previous"
  rm -rf "${staged:?}" "${previous:?}"
  mv "$bundle" "$staged"
  if [ -e "$lib_dir" ]; then
    mv "$lib_dir" "$previous"
    if ! mv "$staged" "$lib_dir"; then
      mv "$previous" "$lib_dir"
      fail "could not put the new release in $lib_dir"
    fi
    rm -rf "${previous:?}"
  else
    mv "$staged" "$lib_dir"
  fi

  # The bundle's installer links the executable and installs the desktop
  # entry, the icon and the user service, then runs `tasks-pending setup`.
  PREFIX="$(dirname "$bin_dir")" INSTALL_LINK=1 RELEASE_BIN="$lib_dir/bin" \
    "$lib_dir/scripts/install.sh" --start
  say "installed $("$bin_dir/tasks-pending" --version 2>/dev/null || echo tasks-pending) to $lib_dir"
}

install_arch_package() {
  base="$1" work="$2"
  package="$(download_checked "$base" "tasks-pending-bin-x86_64.pkg.tar.zst" "$work")"
  command -v pacman >/dev/null 2>&1 || fail "pacman is required to install the Arch package"
  if [ "$(id -u)" -eq 0 ]; then
    pacman -U --noconfirm "$package"
  elif command -v sudo >/dev/null 2>&1; then
    sudo pacman -U --noconfirm "$package"
  else
    fail "sudo is required to install the Arch package"
  fi
  tasks-pending setup
}

main() {
  repository="${TASKS_PENDING_REPOSITORY:-brunoomariano/TasksPending}"
  release_base="${TASKS_PENDING_RELEASE_BASE_URL:-https://github.com/$repository/releases}"
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
  Linux:x86_64 | Linux:amd64) ;;
  *) fail "no TasksPending release is available for $os on $machine (Linux x86_64 only)" ;;
  esac

  release_id=""
  if [ -r "$os_release" ]; then
    release_id="$(sed -n 's/^ID=//p' "$os_release" | head -n 1)"
    release_id="${release_id#\"}"
    release_id="${release_id%\"}"
  fi

  work="$(mktemp -d "${TMPDIR:-/tmp}/tasks-pending.XXXXXX")"
  trap 'rm -rf "${work:?}"' EXIT INT TERM

  say "fetching TasksPending ($version) from $base"
  if [ "$release_id" = arch ]; then
    install_arch_package "$base" "$work"
  else
    install_bundle "$base" "$work" "$bin_dir" "$lib_dir"
  fi
}

main "$@"
