#!/usr/bin/env bash
# Keeps one issue per package with open Dependabot alerts, labelled
# `dependabot`: it lists the package's alerts (severity, advisory, the
# release that fixes it), is edited in place as they change, and is closed
# once the package has none. Dependabot opens no pull requests here (its
# security updates are off), so these issues are how an alert reaches the
# backlog. Run by .github/workflows/dependabot-issues.yml, with a token
# that reads Dependabot alerts and writes issues.
#
#   scripts/dependabot-issues.sh <owner/repo>
#
# The issues it keeps carry a marker naming the package; anything else
# with the label is left alone.
set -euo pipefail

LABEL=dependabot

fail() {
  printf 'dependabot-issues: %s\n' "$*" >&2
  exit 1
}

say() {
  printf '%s\n' "$*"
}

repo="${1:-}"
[[ $repo == ?*/?* ]] || fail "usage: $0 <owner/repo>"
work="$(mktemp -d)"
trap 'rm -rf "${work:?}"' EXIT

# Every open alert, one JSON object a line. A failed read stops here,
# before any issue is touched: read as "no alerts", it would close them all.
gh api --paginate "repos/$repo/dependabot/alerts?state=open&per_page=100" --jq '.[]' >"$work/alerts"

# The issue each package should have, keyed by ecosystem, package and
# manifest.
jq -s '
  def rank: {critical: 4, high: 3, medium: 2, low: 1}[.] // 0;
  def plural($n; $word): "\($n) \($word)\(if $n == 1 then "" else "s" end)";
  group_by([.dependency.package.ecosystem, .dependency.package.name, .dependency.manifest_path])
  | map(
      .[0].dependency as $dep
      | (sort_by(.number)) as $alerts
      | {
          key: "\($dep.package.ecosystem)/\($dep.package.name)/\($dep.manifest_path)",
          title: "Dependabot: update \($dep.package.name) in \($dep.manifest_path)",
          body: ([
            "\(plural($alerts | length; "open Dependabot alert")) for `\($dep.package.name)` (\($dep.package.ecosystem)) in `\($dep.manifest_path)`, highest severity **\($alerts | map(.security_advisory.severity) | max_by(rank))**:",
            "",
            ($alerts[] | "- [#\(.number)](\(.html_url)) **\(.security_advisory.severity)** \(.security_advisory.ghsa_id): \(.security_advisory.summary) (affects \(.security_vulnerability.vulnerable_version_range), \(.security_vulnerability.first_patched_version.identifier // null | if . then "fixed in \(.)" else "no fixed release yet" end))"),
            "",
            "Kept by `.github/workflows/dependabot-issues.yml`: edited as the alerts change, and closed once none is open. To drop an alert, dismiss it in Dependabot: an issue closed by hand comes back on the next run while its alerts are open.",
            "",
            "<!-- dependabot-issues: \($dep.package.ecosystem)/\($dep.package.name)/\($dep.manifest_path) -->"
          ] | join("\n"))
        }
    )
' "$work/alerts" >"$work/wanted"

# GitHub takes label names regardless of case. grep reads the whole list
# (no -q), so gh never writes into a closed pipe.
if ! gh label list --repo "$repo" --search "$LABEL" --json name --jq '.[].name' | grep -Fxi "$LABEL" >/dev/null; then
  gh label create "$LABEL" --repo "$repo" --color 0366d6 --description "A package with open Dependabot alerts"
  say "created the $LABEL label"
fi

# The open issues this script keeps, with the key from their marker; an
# issue without one yields no capture, so it is left out, and so are pull
# requests, which the REST API lists with the issues. Read through REST:
# `gh issue list --label` goes through search, which may not have an issue
# opened a moment ago yet.
gh api --paginate "repos/$repo/issues?state=open&labels=$LABEL&per_page=100" \
  --jq '.[] | select(.pull_request | not) | {number, title, body: (.body // "")}' |
  jq -s '[.[] | (.body | capture("<!-- dependabot-issues: (?<key>.+?) -->")) as $marker | . + {key: $marker.key}]' \
    >"$work/kept"

# Compared without carriage returns or trailing newlines, which GitHub may
# add or drop.
normal() {
  tr -d '\r' | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}'
}

changed=0
while IFS= read -r wanted; do
  key="$(jq -r .key <<<"$wanted")"
  title="$(jq -r .title <<<"$wanted")"
  jq -r .body <<<"$wanted" >"$work/body"
  kept="$(jq -c --arg key "$key" 'map(select(.key == $key)) | first // empty' "$work/kept")"
  if [[ -z $kept ]]; then
    url="$(gh issue create --repo "$repo" --title "$title" --label "$LABEL" --body-file "$work/body")"
    say "opened $url: $title"
    changed=1
    continue
  fi
  number="$(jq -r .number <<<"$kept")"
  if [[ $(jq -r .title <<<"$kept") != "$title" ]] ||
    [[ $(jq -r .body <<<"$kept" | normal) != "$(normal <"$work/body")" ]]; then
    gh issue edit "$number" --repo "$repo" --title "$title" --body-file "$work/body" >/dev/null
    say "updated #$number: $title"
    changed=1
  fi
done < <(jq -c '.[]' "$work/wanted")

while IFS= read -r number; do
  gh issue close "$number" --repo "$repo" \
    --comment "The package has no open Dependabot alert any more." >/dev/null
  say "closed #$number"
  changed=1
done < <(jq -r --slurpfile wanted "$work/wanted" '($wanted[0] | map(.key)) as $keys | .[] | select(.key as $k | $keys | index($k) | not) | .number' "$work/kept")

[[ $changed -eq 1 ]] || say "the issues already match the open alerts"
