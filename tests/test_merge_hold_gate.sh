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
  exit 1
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

extract_block "$github_workflow" 'run: |' >"$work/github-gate.sh"
extract_block "$gitlab_template" '- |' >"$work/gitlab-gate.sh"

# Guard against a vacuous extraction: an empty or truncated script would exit 0
# for every fixture below and report a green that means nothing.
for gate in "$work/github-gate.sh" "$work/gitlab-gate.sh"; do
  for label in aida:merge-hold aida:merge-hold-recorded aida:merge-hold-cleared; do
    grep -Fq "$label" "$gate" || fail "extracted gate $(basename "$gate") does not mention $label"
  done
done

# Run a shipped gate with the label set a forge would hand it.
# $1 = gate script, $2 = env var name the gate reads, $3 = comma-joined labels.
gate() {
  env "$2=$3" bash -e -o pipefail "$1" >/dev/null 2>&1
}

# Each case is "labels|expected exit|description".
cases=(
  'aida:merge-hold,aida:merge-hold-recorded|1|deleting the local marker leaves the active label, so the PR stays held'
  'aida:merge-hold|1|an active hold fails even before the recorded label exists'
  'aida:merge-hold-recorded|1|dropping the active label by hand leaves history without clearance'
  'aida:merge-hold-recorded,aida:merge-hold-cleared|0|a recorded clearance releases a previously held PR'
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
    gate "$script" "$var" "$labels" || got=$?
    [ "$got" = "$want" ] || fail "${target%%:*} with labels '$labels': expected exit $want, got $got ($desc)"
  done
done

# STORY-1690 train mode (GitHub gate): a `train/*` PR passes only when its
# body names its members and every member passes the same label rules. A fake
# `gh` answers `gh api repos/<repo>/issues/<n>/labels` from MEMBER_LABELS_<n>
# (unset = the API call fails).
# trace:STORY-1690 | ai:claude
mkdir -p "$work/bin"
cat >"$work/bin/gh" <<'GH'
#!/usr/bin/env bash
[ "$1" = api ] || exit 2
n=$(printf '%s' "$2" | sed -n 's#^repos/[^/]*/[^/]*/issues/\([0-9][0-9]*\)/labels$#\1#p')
[ -n "$n" ] || exit 2
var="MEMBER_LABELS_$n"
[ "${!var+set}" = set ] || exit 1
printf '%s\n' "${!var}"
GH
chmod +x "$work/bin/gh"
grep -Fq 'aida-train-members:' "$work/github-gate.sh" || fail "extracted GitHub gate has no train mode"

# $1 = head ref, $2 = PR body, then MEMBER_LABELS_<n>=... assignments.
train_gate() {
  local ref=$1 body=$2
  shift 2
  env PATH="$work/bin:$PATH" LABELS= HEAD_REF="$ref" PR_BODY="$body" REPO=o/r "$@" \
    bash -e -o pipefail "$work/github-gate.sh" >/dev/null 2>&1
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

echo "merge-hold gate fixtures passed (${#cases[@]} label states x 2 shipped gates, $train_cases train-mode cases)"
