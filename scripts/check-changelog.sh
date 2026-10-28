#!/usr/bin/env bash
# Checks CHANGELOG.md: it starts with a section for the workspace version or
# "Unreleased", every other section is "## <version>", versions are
# well-formed, newest first, and no version appears twice.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHANGELOG="${1:-$ROOT/CHANGELOG.md}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"

fail() {
  printf 'CHANGELOG.md: %s\n' "$*" >&2
  exit 1
}

# above <a> <b>: version <a> is higher than version <b>.
above() {
  local -a a b
  local i
  IFS=. read -r -a a <<<"$1"
  IFS=. read -r -a b <<<"$2"
  for i in 0 1 2; do
    if (( 10#${a[i]} != 10#${b[i]} )); then
      (( 10#${a[i]} > 10#${b[i]} ))
      return
    fi
  done
  return 1
}

test -f "$CHANGELOG" || fail "missing at $CHANGELOG"
mapfile -t sections < <(sed -n 's/^## //p' "$CHANGELOG")
(( ${#sections[@]} > 0 )) || fail "no '## <version>' section"

first="${sections[0]}"
if [[ $first != Unreleased && $first != "$VERSION" ]]; then
  fail "the first section is '$first'; expected '$VERSION' (Cargo.toml) or 'Unreleased'"
fi

seen=""
previous=""
for index in "${!sections[@]}"; do
  section="${sections[index]}"
  if [[ $section == Unreleased ]]; then
    (( index == 0 )) || fail "'Unreleased' must be the first section"
    continue
  fi
  [[ $section =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "'$section' is not a version"
  case " $seen " in
  *" $section "*) fail "version $section appears twice" ;;
  esac
  if [[ -n $previous ]] && ! above "$previous" "$section"; then
    fail "$section comes after $previous; the newest version goes first"
  fi
  seen="$seen $section"
  previous=$section
done

if [[ $first == Unreleased ]]; then
  body="$(awk '/^## Unreleased$/{on=1;next} /^## /{on=0} on' "$CHANGELOG" | sed '/^\s*$/d')"
  [[ -n $body ]] || fail "the 'Unreleased' section is empty; remove it or describe the changes"
fi
