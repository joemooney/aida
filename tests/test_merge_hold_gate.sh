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

echo "merge-hold gate fixtures passed (${#cases[@]} label states x 2 shipped gates)"
