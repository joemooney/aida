#!/usr/bin/env bash
# Operator-only: one fresh JIT runner, no registration credential kept on disk.
# trace:STORY-2672 | ai:codex
set -euo pipefail
if [ "$EUID" -ne 0 ] || [ "$#" -ne 0 ]; then
    echo 'Usage: operator-generated JIT config on stdin | sudo one-job.sh' >&2
    exit 2
fi
exec 9>/run/aida-ci-runner.lock
flock -n 9 || { echo "A dell1 runner is already active" >&2; exit 1; }
refuse() { echo "Runner launch refused: $*" >&2; exit 1; }
runner_user=aida-ci
runner_source=/opt/aida-actions-runner
[ "$(id -u "$runner_user")" -ne 0 ] || refuse "runner account must be non-root"
# Fail if the account has any supplementary group (especially sudo/docker).
[ "$(id -G "$runner_user" | wc -w)" -eq 1 ] || refuse "runner account has supplementary groups"
[ -x "$runner_source/run.sh" ] || refuse "missing executable runner distribution"
# The operator installs and verifies the distribution; jobs cannot replace it.
unsafe_entry=$(find "$runner_source" \( ! -user root -o \( ! -type l -a -perm /022 \) \) -print -quit) || refuse "cannot inspect runner distribution"
[ -z "$unsafe_entry" ] || refuse "distribution must be root-owned and not writable"
IFS= read -r jit || refuse "missing JIT configuration"
[ -n "$jit" ] || refuse "empty JIT configuration"
# Only one invocation may run; the user must traverse the root-owned parent.
install -d -m 0711 /var/lib/aida-ci-jobs
state=$(mktemp -d /var/lib/aida-ci-jobs/job.XXXXXXXX)
unit="aida-ci-${state##*.}"
cleanup() {
    systemctl stop "$unit.service" >/dev/null 2>&1 || true
    rm -rf -- "$state"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
cp -a "$runner_source/." "$state/"
mkdir "$state/home"
chown -R "$runner_user:$runner_user" "$state"
# Every invocation shares an eight-core quota. The service also has a fixed
# cpuset, protecting the other twelve dell1 cores even during compiler bursts.
systemctl start aida-ci.slice
systemctl set-property --runtime aida-ci.slice CPUQuota=800% AllowedCPUs=0-7
# Idle listening has no wall-clock limit. GitHub bounds execution from job
# pickup via ci.yml timeout-minutes, independently of time spent waiting.
printf '%s\n' "$jit" | systemd-run --quiet --wait --pipe --collect \
    --unit="$unit" --slice=aida-ci.slice \
    -p User="$runner_user" -p Group="$runner_user" \
    -p WorkingDirectory="$state" -p ProtectHome=yes -p ProtectSystem=strict \
    -p ReadWritePaths="$state" -p PrivateTmp=yes -p PrivateIPC=yes \
    -p PrivateDevices=yes -p NoNewPrivileges=yes -p RestrictSUIDSGID=yes \
    -p ProtectKernelModules=yes \
    -p ProtectControlGroups=yes -p RemoveIPC=yes \
    -p 'InaccessiblePaths=-/run/docker.sock -/run/podman -/run/dbus' \
    -p KillMode=control-group -p TimeoutStopSec=30 \
    /usr/bin/env -i HOME="$state/home" PATH=/usr/local/bin:/usr/bin:/bin \
    /bin/bash -c 'set -eu; IFS= read -r config; exec ./run.sh --jitconfig "$config"'
