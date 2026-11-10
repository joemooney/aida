#!/usr/bin/env bash
# Credentialed operator foreground loop. Host independently verifies every claim.
# trace:STORY-2695 | ai:codex
set -euo pipefail
if [ "$EUID" -eq 0 ] || [ "$#" -ne 1 ]; then
    echo 'Usage: rearm.sh s0|s1 (credentialed operator, not root)' >&2
    exit 2
fi
case "$1" in s0|s1) slot=$1;; *) exit 2;; esac
# select-job.py proposes a queued heavy job, TRAIN first. Proposals grant no trust.
while true; do
    proposal=$(/usr/bin/python3 -I /usr/local/libexec/aida-ci-select-job.py "$slot")
    if [ "$proposal" = null ]; then
        echo "$slot: no candidate; waiting 15 seconds"
        sleep 15
        continue
    fi
    name="aida-ci-${slot}-$(/usr/bin/python3 -c 'import uuid; print(uuid.uuid4().hex)')"
    kind=$(printf '%s' "$proposal" | jq -er .class)
    started=$SECONDS
    jit=$(gh api --method POST repos/joemooney/aida/actions/runners/generate-jitconfig \
        -f name="$name" -F runner_group_id=1 \
        -f 'labels[]=self-hosted' -f 'labels[]=Linux' -f 'labels[]=X64' \
        -f 'labels[]=aida-dell1' -f "labels[]=$kind" -f "labels[]=$name" \
        -f work_folder=_work --jq .encoded_jit_config)
    # JIT is stdin data, never argv or a persistent file. No shell interpolation.
    printf '%s\n%s\n%s\n' "$proposal" "$name" "$jit" | /usr/bin/python3 -c '
import json,sys
proposal=json.loads(sys.stdin.readline()); name=sys.stdin.readline().strip(); jit=sys.stdin.readline().strip()
print(json.dumps(dict(run=proposal["run"],job=proposal["job"],runner=name,jit=jit)))
' | sudo -n /usr/local/sbin/aida-ci-one-job "$slot"
    unset jit
    if [ "$((SECONDS - started))" -lt 10 ]; then
        echo 'Short runner lifetime; stop re-registration and diagnose.' >&2
        exit 1
    fi
    echo "$slot: completed; considering the next admitted job"
done
