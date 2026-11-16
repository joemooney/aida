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
rulesets, then maps required check names to workflow jobs using a previous
started run on that branch. For custom status publishers such as
`merge-hold-gate`, an exact workflow display-name match also identifies the
required context. Other workflows need a prior started run with matching job
names within the latest 100 runs. The scan has a 40-second observation budget;
API errors or exhausted budgets report unavailable and preserve previous state.
It never treats missing evidence as recovery.

Alerts use the existing trusted `[notify].command`, with the message on stdin.
Configure that command to fan out to **every** operator push channel (for
example, desktop and phone), and report failure if any channel fails. Test the
transport with `aida notify test`. Without a configured transport, no delivery
or backoff is recorded. Existing quiet hours queue notices for `aida notify
check` to deliver. Repeat intervals are 5, 10, 20, 40, 80, 160 minutes, then four
hours, capped at four hours. A successful latest run sends a recovery notice.
State lives in `.aida/ci-startup.json` under a separate lock; failed delivery
retains the episode for retry. Transport acceptance is not operator receipt.

The alert includes the workflow, required job contexts, latest run URL, first
observed failure time and the count of non-draft, approved open PRs whose head
lacks successful matching required check runs. This is a snapshot: PRs may
also have other blockers. Truncated inventories refuse an alert rather than
supply an incomplete count.

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
