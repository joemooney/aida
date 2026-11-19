# Required Actions check never starts

<!-- trace:STORY-2856 | ai:codex -->

A workflow run can finish with `startup_failure` and zero jobs, or remain queued
without ever starting a job. This can leave required checks absent and prevent
approved PRs from landing. The failure may be external; do not assume a cause
from an empty jobs list.

The supervised auto-train checks on each discovery pass. `aida notify check`
also performs the check; a host without auto-train should schedule that command
at least every three minutes. Passive notification hooks do not query Actions.
Only GitHub is supported by this monitor.

Configure the reviewed default-branch `.aida/config.toml`:

```toml
[ci_startup]
consecutive_runs = 3
queued_timeout_secs = 600
```

The run threshold accepts 1–20; the queue timeout accepts 1–86400 seconds.
The monitor reads the default branch and its branch protection plus effective
rulesets. It then reads each workflow's latest 100 runs that report on that
branch. Push-style runs must name the default branch. `pull_request_target`
and `pull_request` runs carry the PR's source branch, and Actions omits their
PR link on startup failures. Those runs count unless they name a different
target branch. A run that ended `startup_failure`, or is still `queued` past
the timeout, never started a job. Runs that are still queued under the
timeout, or waiting for approval, neither count nor break the streak.

Required check names map to job names from the newest started run, cached per
run. When none of the latest 100 runs started and nothing is cached yet, the
monitor reads older pages (up to 1,000 runs) to find one. For custom status publishers such as `merge-hold-gate`, an exact workflow
display-name match also identifies the required context. The alert's first
failure time is the oldest run in the current streak. When the streak fills
all 100 visible runs, the alert says the start may be earlier.

The scan has a 40-second observation budget. It visits the least recently attempted workflows
first, giving open incidents priority on ties. A blocked-PR scan saves its
progress when the budget runs out and yields to workflows that have waited
longer. API errors or exhausted budgets report unavailable and preserve
previous episodes. Missing evidence never counts as recovery.

Alerts go to the trusted `[notify].command` and to every
`[seat_health].push_commands` channel, each with the message on stdin; either
setting alone is enough. A push command identical to `[notify].command` runs
once. A notice counts as delivered when at least one channel accepts it, so a
retry never repeats it on channels that already succeeded; failures are logged
to `.aida/notify.log`. Test `[notify].command` with `aida notify test`. Without
any configured transport, no delivery or backoff is recorded. Quiet hours queue
`[notify].command` notices for `aida notify check` to deliver; push channels
bypass quiet hours, as seat-health alerts do. Repeat intervals are 5, 10, 20, 40, 80, 160 minutes, then four
hours, capped at four hours. A successful run newer than the alerted failures
sends one recovery notice, even if newer runs are already queued or running.
A successful rerun of the alerted run (same run id, later attempt) also counts. A
later outage starts a new episode with its own first failure time and backoff.
State lives in `.aida/ci-startup.json` under a separate lock; failed delivery
retains the episode for retry. Transport acceptance is not operator receipt.

The alert includes the workflow, required job contexts, latest run URL, first
observed failure time and the count of non-draft, approved open PRs whose head
lacks successful matching required checks or statuses. Each notice uses an
open-PR inventory with pinned head SHAs. Check, status and review observations
may span several passes. If a review observation reports a different head,
the monitor discards that scan and takes a fresh inventory on a later pass.
Truncated inventories prevent delivery of a count. PRs may have other blockers.

1. Open the **run URL**, not only the PR checks summary. Read the run page's
   annotations and actual startup error. `gh run view <run-id>` and
   `gh api repos/{owner}/{repo}/actions/runs/<run-id>/jobs` help distinguish
   zero jobs from jobs that started and failed. Preserve the error and times.
2. The operator should inspect repository **Settings → Actions → General**:
   Actions enablement, allowed actions/reusable workflows, workflow permissions,
   and approval policies. Also check organization policy, runner availability,
   billing/usage limits and GitHub's status page. Inspect only; changes require
   the operator's deliberate decision.
3. For `merge-hold-gate`, editing the PR body triggers a new evaluation through
   the `pull_request_target: edited` event. The operator can make a harmless body edit,
   then follow the new run. Re-running a failed run may also expose the real
   error. Do not repeatedly change code to repair an external startup incident.
4. While the gate is down, the auto-train worker can stay in its **600-second
   refusal loop** waiting for the required check. This does not establish a
   code failure or justify bypassing the gate. Inspect train status and the
   required run; leave its journal and leases intact.
5. Seats must **never post the required status by hand or change repository
   settings**. A synthetic successful status would bypass the protection this
   workflow supplies. Page the operator and retain the refusal until a real
   workflow run succeeds. Recovery alerts are informational and grant no merge
   or completion authority.
