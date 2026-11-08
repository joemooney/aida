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
    gh api --method POST repos/joemooney/aida/actions/runners/generate-jitconfig \
        -f name="dell1-jit-$(date +%s)" -F runner_group_id=1 \
        -f 'labels[]=self-hosted' -f 'labels[]=Linux' -f 'labels[]=X64' \
        -f 'labels[]=aida-dell1' -f work_folder=_work --jq .encoded_jit_config \
        | sudo -n /usr/local/sbin/aida-ci-one-job
    echo 'One-job runner exited; registering its fresh replacement.'
done
