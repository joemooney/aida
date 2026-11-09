#!/usr/bin/env bash
# Run in the operator's foreground session, never as the runner user/root.
# trace:STORY-2672 | ai:codex
set -euo pipefail
if [ "$EUID" -eq 0 ] || [ "$#" -ne 0 ]; then
    echo 'Usage: rearm.sh (as the credentialed operator, not root)' >&2
    exit 2
fi
# Keep gh credentials outside the job sandbox. Failure stops re-registration;
# Ctrl-C stops this supervisor and the current one-job launcher.
# Requires the reviewed command-only NOPASSWD rule from the runbook.
# Do not rely on a sudo timestamp, which expires while a runner is idle.
while true; do
    started=$SECONDS
    gh api --method POST repos/joemooney/aida/actions/runners/generate-jitconfig \
        -f name="dell1-jit-$(date +%s)" -F runner_group_id=1 \
        -f 'labels[]=self-hosted' -f 'labels[]=Linux' -f 'labels[]=X64' \
        -f 'labels[]=aida-dell1' -f work_folder=_work --jq .encoded_jit_config \
        | sudo -n /usr/local/sbin/aida-ci-one-job
    # Even a zero exit can mean startup failure; never churn JIT registrations.
    # trace:BUG-2506 | ai:codex
    elapsed=$((SECONDS - started))
    if [ "$elapsed" -lt 10 ]; then
        echo "Runner exited after ${elapsed}s (under 10s); stopping re-registration. Diagnose startup and remove orphan registrations before restarting." >&2
        exit 1
    fi
    echo 'One-job runner exited; registering its fresh replacement.'
done
