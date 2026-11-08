# dell1 CI runner: operator setup and recovery

<!-- trace:STORY-2672 | ai:codex -->

joemooney/aida is public. Keep `AIDA_DELL1_CI` unset until independent review
explicitly checks the event/repository condition in `ci.yml` and the operator
completes the checklist below. No runner or repository setting is changed by
merging this preparation. Registration and before/after measurements remain
operator acceptance work.

The four heavy shards use `[self-hosted, Linux, X64, aida-dell1]` only when the
variable is exactly `true` and the event is a push or a same-repository PR.
Fork PRs (even approved ones), merge groups and other events select hosted
Ubuntu. Other shards and the required aggregate check remain hosted. A missing
or offline runner does **not** trigger automatic hosted failover: unset the
variable, cancel and rerun queued workflows with the new configuration (start
a fresh run if the rerun retains old routing). One runner executes one shard
at a time; measure whether this actually improves train wait before expanding.

## Provision once, as operator

1. In repository Settings → Actions → General → Fork pull request workflows,
   select **Require approval for all outside collaborators**. Verify and record
   the setting before enabling the runner. Review every workflow change before
   approving outside contributions; YAML routing is not a security boundary
   against an approved PR that changes that YAML.
2. Create a locked account with no supplementary groups or sudo rules:
   `sudo useradd --system --user-group --home-dir /var/empty/aida-ci --shell /usr/sbin/nologin aida-ci`.
   Do not put it in docker, sudo, dirtlab or shared lane groups. Keep dirtlab's
   home mode 0700 and verify the account cannot traverse it or read any operator
   SSH keys, Codex auth or AIDA grants outside it. Never copy these into CI.
3. Install systemd, build-essential, git, curl, python3, pkg-config,
   protobuf-compiler, libxcb-render0-dev, libxcb-shape0-dev, libxcb-xfixes0-dev,
   libxkbcommon-dev, libssl-dev, libgtk-3-dev, bubblewrap, ripgrep, zstd, jq, openssh-client and zsh.
   Install the runner distribution's .NET runtime prerequisites as needed.
   CI installs Rust into the disposable job HOME. Ensure at least eight CPUs
   numbered 0–7 are available; change the reviewed cpuset if dell1 differs.
4. Download a current Linux x64 Actions runner from GitHub's runner setup page,
   verify its published SHA-256, extract into `/opt/aida-actions-runner`, and
   make the complete tree root-owned and not group/other writable. Leave it
   unconfigured. Install the reviewed `scripts/runner/one-job.sh` root-owned at
   `/usr/local/sbin/aida-ci-one-job` (mode 0755). Never execute this root script
   directly from a PR checkout. Install the reviewed `scripts/runner/aida-ci.slice`
   as `/etc/systemd/system/aida-ci.slice` (root-owned, mode 0644), then run
   `sudo systemctl daemon-reload`.
5. Verify user namespaces and nested bwrap work under dell1's existing policy.
   The script does not disable host AppArmor. If the prerequisite check refuses,
   retain hosted CI and have the operator evaluate a narrowly scoped policy or
   disposable VM; do not copy the hosted VM's policy-disabling steps onto dell1.
6. Review the systemd sandbox: ProtectHome hides `/home`, `/root`, `/run/user`;
   fresh private tmp/IPC/devices, read-only system, no privilege escalation,
   blocked common host-control sockets. Do not enable ProtectProc or
   ProtectKernelTunables: their procfs overmounts can prevent the nested proc
   mounts required by the containment tests. The prerequisite probe uses the
   same fresh `/dev` and `/proc` mounts as those tests; self-hosted jobs set
   `AIDA_REQUIRE_BWRAP_LIVE=1` so supported live-test failures cannot skip. There is
   no host credential/environment projection. Root owns the distribution and
   job parent; each job gets a wiped home/work directory at the fixed
   `/var/lib/aida-ci-jobs/current` path. The exclusive launcher lock prevents
   overlap. Stable absolute paths let main-branch pushes warm reusable dell1
   Rust caches; no previous job files survive locally. A shared slice caps
   all runners to eight cores (800% quota and CPU set 0–7). This is a host
   process sandbox, with host network access; do not give it secrets.

## Register and run one job

The operator generates a single-use JIT configuration, using their own `gh`
credentials, and passes only that configuration into the root launcher:

```bash
sudo -v
gh api --method POST repos/joemooney/aida/actions/runners/generate-jitconfig \
  -f name="dell1-jit-$(date +%s)" -F runner_group_id=1 \
  -f 'labels[]=self-hosted' -f 'labels[]=Linux' -f 'labels[]=X64' \
  -f 'labels[]=aida-dell1' -f work_folder=_work --jq .encoded_jit_config \
  | sudo /usr/local/sbin/aida-ci-one-job
```

This uses GitHub's JIT API instead of retaining a registration token. Generating
registration credentials is always an operator act; never run it from the lane.
The JIT runner takes one job, exits and deregisters. The launcher stops its whole
cgroup and deletes the disposable directory on exit. Idle listening has no
local lifetime limit: a late pickup still gets the full 40-minute job timeout
from `ci.yml`. A normal runner exit (job completion or clean idle retirement)
re-arms; nonzero runner/service exits stop supervision for diagnosis. For continuous capacity, run the reviewed `scripts/runner/rearm.sh` in a
foreground operator session (for example an operator-owned tmux session). It
uses host-side gh credentials to generate each fresh configuration and invokes
the installed root launcher through sudo. Credentials never enter the job.
Before starting supervision, use `sudo visudo -f /etc/sudoers.d/aida-ci-launcher`
to install this rule (replace `dirtlab` only if the credentialed operator differs):

```sudoers
dirtlab ALL=(root) NOPASSWD: /usr/local/sbin/aida-ci-one-job ""
```

The `""` restricts this grant to no arguments. Keep the launcher and all parent
paths root-owned and unwritable by the operator or runner. Grant this rule only
to the operator, never `aida-ci`; it does not grant general sudo or project-script
execution. Verify with `sudo -k` then `sudo -n -l /usr/local/sbin/aida-ci-one-job`
and confirm `aida-ci` has no sudo rule. The supervisor uses `sudo -n` on every
cycle and does not depend on cached credentials. Any API, sudo or runner
failure stops the loop. Monitor that session and restart after diagnosing a
failure. Ctrl-C stops it. Start this supervisor before enabling routing and keep
it active while enabled: each PR needs four successive registrations. There is
no unattended credential service installed by this change.
Use the GitHub UI to remove orphan registrations if setup fails. After a host
crash, remove inactive `aida-ci-*` services and stale `/var/lib/aida-ci-jobs/current`
directory (and legacy `job.*` directories) before registering again. Preserve relevant diagnostics first.

Enable with repository variable `AIDA_DELL1_CI=true` only after verification.
During a smoke run, record `systemctl show aida-ci.slice -p CPUQuotaPerSecUSec
-p AllowedCPUs`, confirm the job uses the `aida-ci` uid, and confirm the exact prerequisite probe (including fresh proc/dev mounts and
nested user namespaces) passes under the actual unit properties, then run a
heavy shard with `AIDA_REQUIRE_BWRAP_LIVE=1`. Do not enable routing until this
smoke test succeeds. Confirm prerequisites pass without access to `/home/dirtlab` or sudo. Check a fork PR still selects
`ubuntu-latest`; approving it must not move it to dell1. Review the default
branch workflow as well as any pending workflow modifications.

## Capacity acceptance

Before activation export job records for a fixed set of recent train PR CI runs
with `gh api --paginate repos/joemooney/aida/actions/runs/RUN_ID/jobs`.
Retain job `created_at`, `started_at`, runner name, head SHA and shard name.
For completed heavy shards compute queue seconds as `started_at - created_at`
and report the median, sample count and observation window. Repeat for a
comparable window after activation, separating hosted and dell1 jobs and cold
builds. Also compare end-to-end run duration and dell1 lane load. Do not claim
capacity improvement from registration alone. Roll back the variable if one
serial runner or cold builds increase wait.

References: [GitHub JIT security guidance](https://docs.github.com/en/actions/reference/security/secure-use)
and [runner REST API](https://docs.github.com/en/rest/actions/self-hosted-runners).
