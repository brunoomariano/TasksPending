#!/usr/bin/env bash
# Exercises scripts/check-changelog.sh against small changelogs: each rule
# it states must be able to fail.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK:?}"' EXIT

fail() {
  printf '%s\n' "$*" >&2
  exit 1
}

# check <name> <expected message, empty to pass> <changelog lines...>
check() {
  local name=$1 message=$2 output
  shift 2
  printf '%s\n' '# Changelog' '' "$@" >"$WORK/$name.md"
  if output="$("$ROOT/scripts/check-changelog.sh" "$WORK/$name.md" 2>&1)"; then
    [[ -z $message ]] || fail "$name: the check passed; expected '$message'"
  else
    [[ -n $message ]] || fail "$name: the check failed: $output"
    [[ $output == *"$message"* ]] || fail "$name: expected '$message' in: $output"
  fi
}

check current "" "## $VERSION" '- a change' '## 0.0.2' '## 0.0.1'
check unreleased "" '## Unreleased' '- a change' "## $VERSION" '## 0.0.1'
check stale "expected '$VERSION'" '## 0.0.2' '## 0.0.1'
check empty-unreleased "the 'Unreleased' section is empty" '## Unreleased' '' "## $VERSION"
check second-unreleased "'Unreleased' must be the first section" \
  '## Unreleased' '- a change' "## $VERSION" '## Unreleased' '- another' '## 0.0.1'
check late-unreleased "'Unreleased' must be the first section" "## $VERSION" '## Unreleased' '- a change'
check not-a-version "'next' is not a version" "## $VERSION" '## next'
check twice "version 0.0.1 appears twice" "## $VERSION" '## 0.0.1' '## 0.0.1'
check out-of-order "0.0.2 comes after 0.0.1" "## $VERSION" '## 0.0.1' '## 0.0.2'
check numeric-order "" "## $VERSION" '## 0.0.10' '## 0.0.9'
echo "check-changelog.sh: all cases pass"
