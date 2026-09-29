#!/usr/bin/env bash
# Applies the master protection after this repository becomes public on GitHub.
set -euo pipefail

repo="${1:-$(gh repo view --json nameWithOwner --jq '.nameWithOwner')}"
visibility="$(gh api "repos/$repo" --jq '.visibility')"

if [[ $visibility != public ]]; then
  printf '%s must be public before GitHub Free can protect its branches.\n' "$repo" >&2
  exit 1
fi

gh api --method PUT \
  -H 'Accept: application/vnd.github+json' \
  "repos/$repo/branches/master/protection" \
  --input - <<'JSON'
{
  "required_status_checks": {
    "strict": true,
    "contexts": ["ci", "bundle", "macos", "scripts"]
  },
  "enforce_admins": true,
  "required_pull_request_reviews": null,
  "restrictions": null,
  "required_linear_history": true,
  "allow_force_pushes": false,
  "allow_deletions": false,
  "block_creations": false,
  "required_conversation_resolution": true,
  "lock_branch": false,
  "allow_fork_syncing": false
}
JSON

printf 'Protected master in %s.\n' "$repo"
