#!/usr/bin/env bash
# Exercises scripts/dependabot-issues.sh against a stand-in `gh` that keeps
# the issues and labels in a temporary directory: no network, and no
# repository touched.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
STATE="$WORK/state"
FAKE_BIN="$WORK/bin"
REPO=owner/repo
trap 'rm -rf "${WORK:?}"' EXIT

fail() {
  printf '%s\n' "$*" >&2
  exit 1
}

mkdir -p "$STATE" "$FAKE_BIN"

# `gh`, as far as the script uses it: the alerts come from $STATE/alerts,
# the issues (pull requests among them, as the REST API has it) live in
# $STATE/issues.json and the labels in $STATE/labels. Every call is logged
# in $STATE/calls. A call for another repository, or without naming one,
# fails: the script must never act on whatever repository the working
# directory happens to be.
#
# `gh api` answers a page of one item, newest first as GitHub does: without
# --paginate only the first page comes back, and --jq runs on each page.
cat >"$FAKE_BIN/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$STATE/calls"
issues="$STATE/issues.json"
[[ -f $issues ]] || echo '[]' >"$issues"
touch "$STATE/labels"
sub=$1
shift
if [[ $sub == api ]]; then
  paginate=0
  filter=.
  path=
  while (($#)); do
    case $1 in
    --paginate) paginate=1 && shift ;;
    --jq) filter=$2 && shift 2 ;;
    -*) echo "gh: unexpected flag: $1" >&2 && exit 9 ;;
    *) path=$1 && shift ;;
    esac
  done
  case $path in
  "repos/$EXPECTED_REPO/dependabot/alerts?state=open&per_page=100")
    if [[ -n ${FAIL_ALERTS:-} ]]; then
      echo "gh: HTTP 403: Resource not accessible by integration" >&2
      exit 1
    fi
    jq -s . "$STATE/alerts" >"$STATE/response"
    ;;
  "repos/$EXPECTED_REPO/issues?state=open&labels="*"&per_page=100")
    label=${path#*labels=}
    jq --arg label "${label%%&*}" '[sort_by(-.number)[] | select(.state == "open" and (.labels | index($label)))
      | {number, title, body, labels: (.labels | map({name: .}))} + if .pull_request then {pull_request: {}} else {} end]' \
      "$issues" >"$STATE/response"
    ;;
  *)
    echo "gh: unexpected request: $path" >&2
    exit 9
    ;;
  esac
  pages=$(jq 'length' "$STATE/response")
  ((paginate && pages > 1)) || pages=1
  for ((page = 0; page < pages; page++)); do
    jq -c --argjson page "$page" ".[\$page:\$page + 1] | $filter" "$STATE/response"
  done
  exit 0
fi
action=$1
shift
declare -A opt=()
args=()
while (($#)); do
  case $1 in
  --repo | --title | --label | --body-file | --comment | --jq | --json | --search | --color | --description)
    opt[$1]=$2
    shift 2
    ;;
  *)
    args+=("$1")
    shift
    ;;
  esac
done
[[ ${opt[--repo]:-} == "$EXPECTED_REPO" ]] || { echo "gh: wrong or no --repo: $sub $action" >&2; exit 9; }
case "$sub $action" in
"label list") cat "$STATE/labels" ;;
"label create")
  if grep -Fxiq -- "${args[0]}" "$STATE/labels"; then
    echo "gh: label \"${args[0]}\" already exists" >&2
    exit 1
  fi
  printf '%s\n' "${args[0]}" >>"$STATE/labels"
  ;;
"issue create")
  body="$(cat "${opt[--body-file]}")"
  jq --arg title "${opt[--title]}" --arg body "$body" --arg label "${opt[--label]}" \
    '. + [{number: ((map(.number) | max // 0) + 1), title: $title, body: $body, state: "open", labels: [$label], comments: []}]' \
    "$issues" >"$issues.new"
  mv "$issues.new" "$issues"
  echo "https://github.com/$EXPECTED_REPO/issues/$(jq 'map(.number) | max' "$issues")"
  ;;
"issue edit")
  body="$(cat "${opt[--body-file]}")"
  jq --argjson n "${args[0]}" --arg title "${opt[--title]}" --arg body "$body" \
    'map(if .number == $n then .title = $title | .body = $body else . end)' "$issues" >"$issues.new"
  mv "$issues.new" "$issues"
  ;;
"issue close")
  jq --argjson n "${args[0]}" --arg comment "${opt[--comment]}" \
    'map(if .number == $n then .state = "closed" | .comments += [$comment] else . end)' "$issues" >"$issues.new"
  mv "$issues.new" "$issues"
  ;;
*)
  echo "gh: unexpected command: $sub $action" >&2
  exit 9
  ;;
esac
EOF
chmod +x "$FAKE_BIN/gh"

# alert <number> <ecosystem> <package> <manifest> <severity> <ghsa> <fixed in, or empty> <summary>
alert() {
  jq -cn --argjson number "$1" --arg ecosystem "$2" --arg name "$3" --arg manifest "$4" \
    --arg severity "$5" --arg ghsa "$6" --arg fixed "$7" --arg summary "$8" '{
      number: $number,
      html_url: "https://github.com/owner/repo/security/dependabot/\($number)",
      dependency: {package: {ecosystem: $ecosystem, name: $name}, manifest_path: $manifest},
      security_advisory: {severity: $severity, ghsa_id: $ghsa, summary: $summary},
      security_vulnerability: {
        vulnerable_version_range: (if $fixed == "" then ">= 7.0.0" else "< \($fixed)" end),
        first_patched_version: (if $fixed == "" then null else {identifier: $fixed} end)
      }
    }'
}
RUSTLS_8="$(alert 8 rust rustls Cargo.lock low GHSA-0008 0.23.46 'Brotli decompression can allocate 1 GiB')"
RUSTLS_9="$(alert 9 rust rustls Cargo.lock medium GHSA-0009 0.23.46 'Session Debug formatting exposes secrets')"
RUSTLS_10="$(alert 10 rust rustls Cargo.lock high GHSA-0010 0.23.46 'Hybrid key exchange can be downgraded')"
# A manifest path with a space, which the issue's marker must still name.
VITE_2="$(alert 2 npm vite 'web app/package-lock.json' high GHSA-0002 '' 'launch-editor check can be bypassed')"

# run [env...]: runs the script for $REPO against the stand-in, after
# clearing the call log.
run() {
  : >"$STATE/calls"
  env PATH="$FAKE_BIN:$PATH" STATE="$STATE" EXPECTED_REPO="$REPO" "$@" \
    "$ROOT/scripts/dependabot-issues.sh" "$REPO"
}
issue() {
  jq -r --argjson n "$1" --arg field "$2" '.[] | select(.number == $n) | .[$field] | if type == "array" then join("\n") else . end' \
    "$STATE/issues.json"
}
writes() {
  grep -E '^(issue (create|edit|close)|label create)' "$STATE/calls" || true
}

# What carries the label but is not the script's to change: someone's own
# issue, one without a body, and a pull request quoting a marker.
jq -n '[
  {number: 1, title: "Should we pin the toolchain?", body: "Asked in passing.", state: "open", labels: ["dependabot"], comments: []},
  {number: 2, title: "Look at the lockfile", body: null, state: "open", labels: ["dependabot"], comments: []},
  {number: 3, title: "Pin left-pad", body: "Quoting <!-- dependabot-issues: npm/left-pad/package-lock.json -->",
   state: "open", labels: ["dependabot"], comments: [], pull_request: true}
]' >"$STATE/issues.json"

# One issue per package and manifest, with each alert's severity,
# advisory and fix, oldest alert first; the label is created when the
# repository lacks it. GitHub lists the alerts newest first.
printf '%s\n' "$RUSTLS_10" "$VITE_2" "$RUSTLS_9" "$RUSTLS_8" >"$STATE/alerts"
run >/dev/null
[[ $(grep -c '^label create dependabot' "$STATE/calls") == 1 ]] || fail "the label was not created once: $(cat "$STATE/calls")"
[[ $(jq '[.[] | select(.state == "open" and .number > 3)] | length' "$STATE/issues.json") == 2 ]] ||
  fail "expected two issues: $(cat "$STATE/issues.json")"
rustls=$(jq '.[] | select(.title | contains("rustls")) | .number' "$STATE/issues.json")
vite=$(jq '.[] | select(.title | contains("vite")) | .number' "$STATE/issues.json")
[[ $(issue "$rustls" title) == "Dependabot: update rustls in Cargo.lock" ]] || fail "rustls title: $(issue "$rustls" title)"
[[ $(issue "$vite" title) == "Dependabot: update vite in web app/package-lock.json" ]] || fail "vite title: $(issue "$vite" title)"
body=$(issue "$rustls" body)
for expected in "3 open Dependabot alerts" "highest severity **high**" \
  "[#8](https://github.com/owner/repo/security/dependabot/8) **low** GHSA-0008: Brotli decompression can allocate 1 GiB" \
  "[#9](https://github.com/owner/repo/security/dependabot/9) **medium** GHSA-0009" \
  "[#10](https://github.com/owner/repo/security/dependabot/10) **high** GHSA-0010" "fixed in 0.23.46"; do
  [[ $body == *"$expected"* ]] || fail "the rustls issue lacks '$expected': $body"
done
[[ $body == *"[#8]"*"[#9]"*"[#10]"* ]] || fail "the alerts are not oldest first: $body"
[[ $(issue "$vite" body) == *"1 open Dependabot alert "*"no fixed release yet"* ]] ||
  fail "the vite issue does not say no fix exists: $(issue "$vite" body)"

# Run again with the same alerts: nothing changes, and a label that differs
# only in case is the same label.
echo Dependabot >"$STATE/labels"
run >/dev/null
[[ -z $(writes) ]] || fail "a run with nothing new wrote: $(writes)"

# An alert fixed: its package's issue is edited in place, and only it.
printf '%s\n' "$RUSTLS_10" "$VITE_2" "$RUSTLS_9" >"$STATE/alerts"
run >/dev/null
[[ $(writes) == "issue edit $rustls "* ]] || fail "expected one edit of #$rustls: $(writes)"
[[ $(writes | wc -l) == 1 ]] || fail "more than one write: $(writes)"
[[ $(issue "$rustls" body) == *"2 open Dependabot alerts"* && $(issue "$rustls" body) != *"[#8]"* ]] ||
  fail "the fixed alert is still listed: $(issue "$rustls" body)"

# A package left without alerts: its issue is closed, saying why.
printf '%s\n' "$RUSTLS_10" "$RUSTLS_9" >"$STATE/alerts"
run >/dev/null
[[ $(writes) == "issue close $vite "* && $(writes | wc -l) == 1 ]] || fail "expected #$vite closed alone: $(writes)"
[[ $(issue "$vite" state) == closed && $(issue "$vite" comments) == *"no open Dependabot alert"* ]] ||
  fail "#$vite was not closed with a comment: $(cat "$STATE/issues.json")"

# An issue closed by hand while its alerts are open comes back as a new
# one: the alerts decide, and dismissing them is how one is dropped.
jq --argjson n "$rustls" 'map(if .number == $n then .state = "closed" else . end)' "$STATE/issues.json" >"$STATE/issues.new"
mv "$STATE/issues.new" "$STATE/issues.json"
run >/dev/null
[[ $(writes) == "issue create --repo $REPO --title Dependabot: update rustls in Cargo.lock "* && $(writes | wc -l) == 1 ]] ||
  fail "expected a new rustls issue alone: $(writes)"
[[ $(issue "$rustls" state) == closed ]] || fail "#$rustls, closed by hand, was reopened"
rustls=$(jq '[.[] | select((.title | contains("rustls")) and .state == "open") | .number] | first' "$STATE/issues.json")

# A failed read of the alerts changes nothing: an empty list read as "no
# alerts" would close every issue.
if output="$(run FAIL_ALERTS=1 2>&1)"; then
  fail "the script went on after the alerts could not be read"
fi
[[ $output == *"HTTP 403"* ]] || fail "the failure was not shown: $output"
[[ -z $(writes) ]] || fail "a failed read wrote: $(writes)"
[[ $(issue "$rustls" state) == open ]] || fail "a failed read closed #$rustls"

[[ $(issue 1 state) == open && $(issue 1 body) == "Asked in passing." ]] || fail "someone's own issue was changed"
[[ $(issue 2 state) == open && $(issue 3 state) == open ]] || fail "an issue without a body or the pull request was closed"

# Without a repository it says how to run it, and calls nothing.
: >"$STATE/calls"
if output="$(PATH="$FAKE_BIN:$PATH" STATE="$STATE" EXPECTED_REPO="$REPO" "$ROOT/scripts/dependabot-issues.sh" 2>&1)"; then
  fail "the script ran without a repository"
fi
[[ $output == *"usage:"* ]] || fail "no usage without a repository: $output"
if [[ -s $STATE/calls ]]; then
  fail "the script called gh without a repository: $(cat "$STATE/calls")"
fi

echo "dependabot-issues.sh: all cases pass"
