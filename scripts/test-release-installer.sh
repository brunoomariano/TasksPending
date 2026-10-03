#!/usr/bin/env bash
# Exercises the public release bootstrap against local download fixtures.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
FIXTURES="$WORK/fixtures"
FAKE_BIN="$WORK/fake-bin"
trap 'rm -rf "$WORK"' EXIT

assert_contains() {
  local text=$1 expected=$2
  [[ $text == *"$expected"* ]] || {
    printf 'expected installer output to contain: %s\n' "$expected" >&2
    exit 1
  }
}

assert_not_contains() {
  local text=$1 unexpected=$2
  [[ $text != *"$unexpected"* ]] || {
    printf 'installer output must not contain: %s\n' "$unexpected" >&2
    exit 1
  }
}

mkdir -p "$FIXTURES" "$FAKE_BIN"

make_bundle() {
  local target=$1
  local asset="tasks-pending-$target.tar.gz"
  local bundle="$WORK/tasks-pending-0.5.0-$target"

  mkdir -p "$bundle/scripts"
  cat >"$bundle/scripts/install.sh" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >"$INSTALL_LOG"
EOF
  chmod +x "$bundle/scripts/install.sh"
  tar -C "$WORK" -czf "$FIXTURES/$asset" "${bundle##*/}"
  (
    cd "$FIXTURES"
    sha256sum "$asset" >"$asset.sha256"
  )
  rm -rf "$bundle"
}

make_bundle x86_64-unknown-linux-gnu
make_bundle aarch64-unknown-linux-gnu
make_bundle aarch64-apple-darwin
make_bundle x86_64-apple-darwin

touch "$FIXTURES/tasks-pending-bin-x86_64.pkg.tar.zst"
(
  cd "$FIXTURES"
  sha256sum tasks-pending-bin-x86_64.pkg.tar.zst >tasks-pending-bin-x86_64.pkg.tar.zst.sha256
)

cat >"$FAKE_BIN/uname" <<'EOF'
#!/bin/sh
case $1 in
-s) printf '%s\n' "$MOCK_UNAME_S" ;;
-m) printf '%s\n' "$MOCK_UNAME_M" ;;
*) exit 2 ;;
esac
EOF
cat >"$FAKE_BIN/curl" <<'EOF'
#!/bin/sh
output=""
url=""
while [ "$#" -gt 0 ]; do
  case $1 in
  -o|--output)
    output=$2
    shift 2
    ;;
  *)
    url=$1
    shift
    ;;
  esac
done
asset=${url##*/}
if [ "${CORRUPT_ASSET:-}" = "$asset" ]; then
  printf 'corrupt' >"$output"
else
  cp "$FIXTURES/$asset" "$output"
fi
EOF
cat >"$FAKE_BIN/systemctl" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$SYSTEMCTL_LOG"
if [ "$*" = "--user is-active --quiet tasks-pending" ]; then
  exit "${SYSTEMCTL_ACTIVE:-1}"
fi
EOF
cat >"$FAKE_BIN/sudo" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$PACMAN_LOG"
EOF
cat >"$FAKE_BIN/pacman" <<'EOF'
#!/bin/sh
exit 0
EOF
cat >"$FAKE_BIN/shasum" <<'EOF'
#!/bin/sh
[ "$1" = "-a" ] && shift 2
exec sha256sum "$@"
EOF
chmod +x "$FAKE_BIN"/*

run_bundle_case() {
  local os=$1 machine=$2 target=$3 os_release=${4:-"$WORK/non-arch-os-release"}
  local install_log="$WORK/$target.install.log"
  local output

  output="$(
    MOCK_UNAME_S="$os" \
      MOCK_UNAME_M="$machine" \
      PATH="$FAKE_BIN:$PATH" \
      FIXTURES="$FIXTURES" \
      INSTALL_LOG="$install_log" \
      TASKS_PENDING_DOWNLOAD_BASE="https://example.test/download" \
      TASKS_PENDING_OS_RELEASE="$os_release" \
      sh "$ROOT/scripts/install-release.sh"
  )"

  grep -Fqx -- '--start' "$install_log"
  [[ $output == *"TasksPending is running at http://127.0.0.1:61000"* ]]
}

run_bundle_case Linux x86_64 x86_64-unknown-linux-gnu
run_bundle_case Linux aarch64 aarch64-unknown-linux-gnu
run_bundle_case Darwin arm64 aarch64-apple-darwin
run_bundle_case Darwin x86_64 x86_64-apple-darwin

malicious_os_release="$WORK/malicious-os-release"
marker="$WORK/os-release-executed"
printf 'ID=arch; touch %s\n' "$marker" >"$malicious_os_release"
run_bundle_case Linux x86_64 x86_64-unknown-linux-gnu "$malicious_os_release"
test ! -e "$marker" || {
  echo "the release bootstrap executed os-release contents" >&2
  exit 1
}

printf 'ID="arch"\n' >"$WORK/arch-os-release"
for state in 1 0; do
  systemctl_log="$WORK/arch-$state.systemctl.log"
  pacman_log="$WORK/arch-$state.pacman.log"
  MOCK_UNAME_S=Linux \
    MOCK_UNAME_M=x86_64 \
    PATH="$FAKE_BIN:$PATH" \
    FIXTURES="$FIXTURES" \
    SYSTEMCTL_ACTIVE="$state" \
    SYSTEMCTL_LOG="$systemctl_log" \
    PACMAN_LOG="$pacman_log" \
    TASKS_PENDING_DOWNLOAD_BASE="https://example.test/download" \
    TASKS_PENDING_OS_RELEASE="$WORK/arch-os-release" \
    sh "$ROOT/scripts/install-release.sh"
  grep -Fq 'pacman -U ' "$pacman_log"
  if [[ $state == 1 ]]; then
    grep -Fqx -- '--user enable --now tasks-pending' "$systemctl_log"
  else
    grep -Fqx -- '--user restart tasks-pending' "$systemctl_log"
  fi
done

missing_env_home="$WORK/arch-missing-env-home"
arch_output="$(
  HOME="$missing_env_home" \
    MOCK_UNAME_S=Linux \
    MOCK_UNAME_M=x86_64 \
    PATH="$FAKE_BIN:$PATH" \
    FIXTURES="$FIXTURES" \
    SYSTEMCTL_ACTIVE=0 \
    SYSTEMCTL_LOG="$WORK/arch-missing-env.systemctl.log" \
    PACMAN_LOG="$WORK/arch-missing-env.pacman.log" \
    TASKS_PENDING_DOWNLOAD_BASE="https://example.test/download" \
    TASKS_PENDING_OS_RELEASE="$WORK/arch-os-release" \
    sh "$ROOT/scripts/install-release.sh"
)"
assert_contains "$arch_output" "Warning: token file is missing; authenticated sources will be unavailable."

existing_env_home="$WORK/arch-existing-env-home"
install -Dm 0600 /dev/stdin "$existing_env_home/.config/tasks-pending/env" <<'EOF'
TODOIST_API_TOKEN=preserve-this-token
EOF
arch_output="$(
  HOME="$existing_env_home" \
    MOCK_UNAME_S=Linux \
    MOCK_UNAME_M=x86_64 \
    PATH="$FAKE_BIN:$PATH" \
    FIXTURES="$FIXTURES" \
    SYSTEMCTL_ACTIVE=0 \
    SYSTEMCTL_LOG="$WORK/arch-existing-env.systemctl.log" \
    PACMAN_LOG="$WORK/arch-existing-env.pacman.log" \
    TASKS_PENDING_DOWNLOAD_BASE="https://example.test/download" \
    TASKS_PENDING_OS_RELEASE="$WORK/arch-os-release" \
    sh "$ROOT/scripts/install-release.sh"
)"
assert_not_contains "$arch_output" "Warning: token file is missing; authenticated sources will be unavailable."
grep -Fqx 'TODOIST_API_TOKEN=preserve-this-token' "$existing_env_home/.config/tasks-pending/env"
test "$(stat -c %a "$existing_env_home/.config/tasks-pending/env")" = 600

if MOCK_UNAME_S=Linux \
  MOCK_UNAME_M=x86_64 \
  PATH="$FAKE_BIN:$PATH" \
  FIXTURES="$FIXTURES" \
  INSTALL_LOG="$WORK/corrupt.install.log" \
  CORRUPT_ASSET=tasks-pending-x86_64-unknown-linux-gnu.tar.gz \
  TASKS_PENDING_DOWNLOAD_BASE="https://example.test/download" \
  TASKS_PENDING_OS_RELEASE="$WORK/non-arch-os-release" \
  sh "$ROOT/scripts/install-release.sh" >/dev/null 2>&1; then
  echo "the release bootstrap accepted a corrupt archive" >&2
  exit 1
fi
test ! -e "$WORK/corrupt.install.log"

if MOCK_UNAME_S=Linux \
  MOCK_UNAME_M=s390x \
  PATH="$FAKE_BIN:$PATH" \
  FIXTURES="$FIXTURES" \
  TASKS_PENDING_DOWNLOAD_BASE="https://example.test/download" \
  TASKS_PENDING_OS_RELEASE="$WORK/non-arch-os-release" \
  sh "$ROOT/scripts/install-release.sh" >/dev/null 2>&1; then
  echo "the release bootstrap accepted an unsupported platform" >&2
  exit 1
fi
