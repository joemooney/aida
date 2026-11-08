#!/usr/bin/env bash
# tests/test_merge_hold_gate.sh — BUG-1693 regression test.
#
# The merge-hold gate is the server-side half of the integrity floor: an agent
# that deletes the local marker, or drops the `aida:merge-hold` label, must
# still not be able to release a PR. This asserts that against the SHIPPED
# gate scripts, not against a copy of them: it extracts the `run:` block out of
# .github/workflows/merge-hold-gate.yml and the `script:` block out of
# aida-core/templates/gitlab-ci-merge-hold-gate.yml and executes those bytes.
# Deleting or weakening either artifact fails this test.
#
# The local-marker half of the same floor (AIDA recording that a marker
# disappeared without a clearance record) is covered by the `merge_hold` unit
# tests in aida-cli-lib/src/merge_hold.rs; this file covers the forge gate.
#
# trace:BUG-1693 | ai:claude
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
github_workflow="$repo_root/.github/workflows/merge-hold-gate.yml"
gitlab_template="$repo_root/aida-core/templates/gitlab-ci-merge-hold-gate.yml"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 99
}

# Extract the first literal block scalar that follows a key on/after $2 out of
# a YAML file, dedented, so the fixtures run the bytes CI runs.
extract_block() {
  python3 - "$1" "$2" <<'PY'
import sys

path, anchor = sys.argv[1], sys.argv[2]
lines = open(path, encoding="utf-8").read().splitlines()
start = next((i for i, l in enumerate(lines) if anchor in l), None)
if start is None:
    sys.exit(f"anchor {anchor!r} not found in {path}")
head = lines[start]
indent = len(head) - len(head.lstrip())
body = []
for line in lines[start + 1:]:
    if not line.strip():
        body.append("")
        continue
    if len(line) - len(line.lstrip()) <= indent:
        break
    body.append(line)
if not body:
    sys.exit(f"literal block after {anchor!r} in {path} is empty")
strip = min(len(l) - len(l.lstrip()) for l in body if l.strip())
print("\n".join(l[strip:] if l.strip() else "" for l in body))
PY
}

[ -f "$github_workflow" ] || fail "missing $github_workflow"
[ -f "$gitlab_template" ] || fail "missing $gitlab_template"

# The publisher must not emit a successful check with the required status name,
# and fork events must execute only trusted base workflow code.
grep -Fq '  pull_request_target:' "$github_workflow" || fail 'fork status token unavailable'
grep -Fq '  statuses: write' "$github_workflow" || fail 'status permission missing'
grep -Fq '  cancel-in-progress: false' "$github_workflow" || fail 'publisher can be cancelled mid-write'
if grep -Eq '^  merge-hold-gate:|uses:.*checkout' "$github_workflow"; then
  fail 'publisher shadows the required context or checks out PR code'
fi

extract_block "$github_workflow" 'run: |' >"$work/github-gate.sh"
extract_block "$gitlab_template" '- |' >"$work/gitlab-gate.sh"

# Guard against a vacuous extraction: an empty or truncated script would exit 0
# for every fixture below and report a green that means nothing.
for gate in "$work/github-gate.sh" "$work/gitlab-gate.sh"; do
  for label in aida:merge-hold aida:merge-hold-recorded aida:merge-hold-cleared; do
    grep -Fq "$label" "$gate" || fail "extracted gate $(basename "$gate") does not mention $label"
  done
done

# trace:BUG-2492 | ai:codex
# Fake only the API boundary; execute the shipped evaluator and publisher.
mkdir -p "$work/bin"
cat >"$work/bin/gh" <<'GH'
#!/usr/bin/env bash
set -eu
[ "$1" = api ] || exit 2
shift
if [ "$1" = --method ]; then
  [ "${STATUS_FAIL:-0}" = 0 ] || exit 1
  printf '%s\n' "$*" >>"$STATUS_LOG"
  exit 0
fi
case "$1" in
  repos/o/r/pulls/1)
    jq -n --arg labels "${LABELS:-}" --arg ref "${HEAD_REF:-feature/x}" --arg body "${PR_BODY:-}" \
      '{head:{sha:"abc123456789",ref:$ref},labels:($labels|split(",")|map(select(length>0)|{name:.})),body:$body}'
    exit ;;
esac
n=$(printf '%s' "$1" | sed -n 's#^repos/[^/]*/[^/]*/issues/\([0-9][0-9]*\)/labels$#\1#p')
[ -n "$n" ] || exit 2
var="MEMBER_LABELS_$n"
[ "${!var+set}" = set ] || exit 1
printf '%s\n' "${!var}"
GH
chmod +x "$work/bin/gh"
export PATH="$work/bin:$PATH" REPO=o/r PR_NUMBER=1
export HEAD_SHA=abc123456789
export GITHUB_SERVER_URL=https://github.com GITHUB_RUN_ID=123
export STATUS_LOG="$work/statuses"

# Pending holds succeed as Actions jobs but continue to block required status.
# Translate the final commit status into the existing pass/block fixture result.
github_gate() {
  : >"$STATUS_LOG"
  local result=0 state
  bash -e -o pipefail "$work/github-gate.sh" >/dev/null 2>&1 || result=$?
  state=$(tail -n 1 "$STATUS_LOG")
  [[ "$state" == *'repos/o/r/statuses/abc123456789'* ]] || fail 'status must target PR head SHA'
  [[ "$state" == *'context=merge-hold-gate'* ]] || fail 'required context changed'
  if [ -n "${WANT_STATE:-}" ]; then
    [[ "$state" == *"state=$WANT_STATE "* ]] || fail "expected $WANT_STATE status, got $state"
  fi
  if [[ "$state" == *'state=pending'* ]]; then
    [ "$result" = 0 ] || fail 'ordinary hold failed the Actions job'
    return 1
  elif [[ "$state" == *'state=failure'* ]]; then
    [ "$result" != 0 ] || fail 'invalid evidence did not fail the Actions job'
    return 1
  fi
  [[ "$state" == *'state=success'* ]] || fail 'missing final status'
  [ "$result" = 0 ] || fail 'passing labels failed the Actions job'
}

# Run a shipped gate with the label set a forge would hand it.
# $1 = gate script, $2 = env var name the gate reads, $3 = comma-joined labels.
gate() {
  if [ "$2" = LABELS ]; then
    LABELS="$3" github_gate
  else
    env "$2=$3" bash -e -o pipefail "$1" >/dev/null 2>&1
  fi
}

# Each case is "labels|expected exit|description".
cases=(
  'aida:merge-hold,aida:merge-hold-recorded|1|deleting the local marker leaves the active label, so the PR stays held'
  'aida:merge-hold|1|an active hold fails even before the recorded label exists'
  'aida:merge-hold-recorded|1|dropping the active label by hand leaves history without clearance'
  'aida:merge-hold-recorded,aida:merge-hold-cleared|0|a recorded clearance releases a previously held PR'
  'aida:merge-hold,aida:merge-hold-cleared|1|active hold with orphan clearance blocks'
  '|0|a PR that was never held passes'
  'aida:merge-hold-cleared|1|a clearance label with no recorded hold is forged and fails'
  'aida:merge-hold,aida:merge-hold-recorded,aida:merge-hold-cleared|1|a clearance label cannot release a still-active hold'
)

for spec in "${cases[@]}"; do
  labels=${spec%%|*}
  rest=${spec#*|}
  want=${rest%%|*}
  desc=${rest#*|}
  for target in "github-gate.sh:LABELS" "gitlab-gate.sh:CI_MERGE_REQUEST_LABELS"; do
    script=$work/${target%%:*}
    var=${target#*:}
    got=0
    WANT_STATE=success
    if [ "$want" = 1 ]; then
      WANT_STATE=pending
      if [[ ",$labels," == *',aida:merge-hold-cleared,'* && ",$labels," != *',aida:merge-hold-recorded,'* ]]; then
        WANT_STATE=failure
      fi
    fi
    gate "$script" "$var" "$labels" || got=$?
    unset WANT_STATE
    [ "$got" = "$want" ] || fail "${target%%:*} with labels '$labels': expected exit $want, got $got ($desc)"
  done
done

# A status API error must fail the job instead of reporting successful publication.
if STATUS_FAIL=1 LABELS= bash -e -o pipefail "$work/github-gate.sh" >/dev/null 2>&1; then
  fail 'status API refusal was ignored'
fi

# STORY-1690 train mode (GitHub gate): a `train/*` PR passes only when its
# body names its members and every member passes the same label rules. A fake
# `gh` answers `gh api repos/<repo>/issues/<n>/labels` from MEMBER_LABELS_<n>
# (unset = the API call fails).
# trace:STORY-1690 | ai:claude
grep -Fq 'aida-train-members:' "$work/github-gate.sh" || fail "extracted GitHub gate has no train mode"

# $1 = head ref, $2 = PR body, then MEMBER_LABELS_<n>=... assignments.
train_gate() {
  local ref=$1 body=$2
  shift 2
  (
    export LABELS= HEAD_REF="$ref" PR_BODY="$body"
    for assignment in "$@"; do export "$assignment"; done
    github_gate
  )
}

a40=$(printf 'a%.0s' $(seq 40))
b40=$(printf 'b%.0s' $(seq 40))
two="Train.

aida-train-members: 11@$a40,12@$b40"
train_cases=0
expect_train() {
  local want=$1 desc=$2
  shift 2
  local got=0
  train_gate "$@" || got=$?
  [ "$got" = "$want" ] || fail "train gate: expected exit $want, got $got ($desc)"
  train_cases=$((train_cases + 1))
}
expect_train 0 'two clear members pass' train/x "$two" MEMBER_LABELS_11= MEMBER_LABELS_12=
expect_train 0 'a member with a recorded clearance passes' train/x "$two" \
  MEMBER_LABELS_11=aida:merge-hold-recorded,aida:merge-hold-cleared MEMBER_LABELS_12=
expect_train 1 'a held member fails the train' train/x "$two" \
  MEMBER_LABELS_11= MEMBER_LABELS_12=aida:merge-hold,aida:merge-hold-recorded
expect_train 1 'a member held before without clearance fails the train' train/x "$two" \
  MEMBER_LABELS_11=aida:merge-hold-recorded MEMBER_LABELS_12=
expect_train 1 'a member with a forged clearance fails the train' train/x "$two" \
  MEMBER_LABELS_11=aida:merge-hold-cleared MEMBER_LABELS_12=
expect_train 1 'an unreadable member fails closed' train/x "$two" MEMBER_LABELS_11=
expect_train 1 'a train PR with no members line fails' train/x 'no members here'
expect_train 1 'a malformed member entry fails' train/x "aida-train-members: x@$a40"
expect_train 1 'a member entry with no @sha fails' train/x "aida-train-members: 11" MEMBER_LABELS_11=
expect_train 1 'a member entry with a non-hex sha fails' train/x "aida-train-members: 11@zzzzzzz" MEMBER_LABELS_11=
expect_train 1 'a member entry with a sha under 7 chars fails' train/x "aida-train-members: 11@abc12" MEMBER_LABELS_11=
expect_train 0 'a 7-char hex sha passes' train/x "aida-train-members: 11@abc1234" MEMBER_LABELS_11=
expect_train 0 'a non-train PR ignores a members line' feature/x "$two"
expect_train 0 'a CRLF body (a web edit) still parses' train/x \
  "$(printf 'Train.\r\n\r\naida-train-members: 11@%s\r\n' "$a40")" MEMBER_LABELS_11=

# The members-line cases shared with the Rust parser (pr_train
# parse_members_line), so the binding bash parser and it cannot drift.
members_fixture="$repo_root/tests/fixtures/train-members-lines.txt"
shared_cases=0
while IFS=$'\t' read -r want value; do
  case "$want" in '' | '#'*) continue ;; esac
  value=${value//<CR>/$'\r'}
  expect_train "$want" "shared members line '$value'" train/x "Train.

aida-train-members: $value" MEMBER_LABELS_11= MEMBER_LABELS_12=
  shared_cases=$((shared_cases + 1))
done <"$members_fixture"
[ "$shared_cases" -ge 10 ] || fail "shared members-line fixture read only $shared_cases cases"

echo "merge-hold gate fixtures passed (${#cases[@]} label states x 2 shipped gates, $train_cases train-mode cases)"
