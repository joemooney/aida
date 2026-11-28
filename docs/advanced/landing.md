# Landing approved work

<!-- trace:STORY-2809 | ai:claude -->
<!-- trace:STORY-b6b1278890774ef993d2ebc327862470-732.Docs | ai:claude -->

How a pull request reaches the default branch depends on the landing mode.
In `ci-gated` mode (the default) it lands through a merge train
(`aida pr train`), which tests the combined result before it merges. In
`review-gated` mode it merges on an independent approval at its exact head,
and CI runs after the merge. This page describes both modes, the CI a pull
request must pass before a train admits it in `ci-gated` mode, and what
happens when main goes red. The train itself is described in the
[git lifecycle reference](../cli/04-git-lifecycle.md).

## Landing modes

<!-- trace:STORY-2897 trace:TASK-3818 | ai:claude -->

`[landing] mode` in `.aida/config.toml` chooses what a merge into the default branch waits for: `ci-gated` (the default) or `review-gated`. AIDA reads it from the default branch only, so a pull request cannot relax it with its own copy. A value it does not recognise falls back to `ci-gated`, with a warning.

In `ci-gated` mode, `aida pr ship` waits for CI on the head, and the train admits only a green full-tier head. Choose it (the default) when a broken main is expensive: many changes land at once, a revert is slow, or nobody watches main after a merge. The CI sections below describe this mode.

In `review-gated` mode, a change merges as soon as it has an independent approval at its exact head and no hold. Nothing waits for CI; CI runs on the default branch after the merge. Choose it when review rounds are slow because they queue behind CI, and a broken main can be fixed or reverted quickly. Nothing else relaxes: the approval must be at the exact head and from someone who did not author the change, every hold binds, and mandate path coverage still applies. In exchange, the advisor seat (or the operator at a terminal) performs the merge, and only for a change it authored none of.

There is no train in `review-gated` mode. A head behind its base merges as it is when none of the base's new commits touched a file it changes (an edit later reverted still counts) and it changes no protected (gate or authority) path. Otherwise it is refused until the branch is updated and the update is reviewed. Every merging command judges this at the moment it merges, against the base as the forge has it then.

<!-- trace:STORY-2903 | ai:claude -->

Every AIDA command that merges follows the mode, and each re-checks it immediately before the merge: if the mode on the default branch changed while it waited, or the caller's authority lapsed, nothing merges.

| Command | `ci-gated` | `review-gated` |
|---|---|---|
| `aida pr ship` | waits for CI on the head | merges on the approval; the advisor or the operator only |
| `aida queue work` (the drain's merge phase) | waits for CI on the head | merges on the approval; the advisor or the operator only |
| `aida burndown` (the wave merge) | waits for CI on the head | merges on the approval; the advisor or the operator only |
| `aida release --after-pr` | waits for CI on the head | waits for CI on the head (a release must build) |
| `aida land run` | boards ready changes onto the train | ships each ready change once when the run may merge it; otherwise reports it |
| `aida pr train` | as described below | the advisor or the operator only; each car's authority is judged before the merge |

<!-- trace:STORY-2905 | ai:claude -->

One merge hand per pull request: before any of these commands merges a pull request, it claims it on this host, and a pull request another command is already merging (or a train has boarded) is refused rather than merged twice; the refusal for a boarded pull request names how to wait for the train or release it. Claims are local to the host, so only the primary host merges; every merge also names the exact head it merges (`--match-head-commit`), which stops a second host's stale merge at the forge. A claim left by a process that has exited is taken over; one whose holder cannot be checked refuses.

A train's merge is judged again before each merge attempt, and once more as the last step before the forge call, after every wait: the landing policy must be readable and the one it was proved under, the advisor-merger rule and the author rule apply to every car, and the base must still be the commit the train was built and proved on; anything else stops the train before it merges.

In `review-gated` mode the auto-train stands down: each pass does nothing and says so (`standing down: landing.mode=review-gated`, also shown by `aida pr train --status`). `aida land run` is the merge hand. In `ci-gated` mode the auto-train runs as before.

### Changing the mode

`aida land mode status` prints the mode, the checks it requires, the checks the forge requires, and any drift between them. It also shows the last few changes, which are recorded in `.aida/land/landing-audit.jsonl`.

`aida land mode set <mode>` changes the mode:

1. It replaces the default branch's required status checks with the list for that mode.
2. It reads the checks back. If the forge reports something else, it restores the previous list and fails.
3. It records the new mode in `.aida/config.toml`. Landing reads the default branch's copy, so the edit takes effect once it lands in a reviewed pull request.

`aida land mode reconcile` re-applies the current mode's list.

Both `set` and `reconcile` change branch protection, so each needs either the operator at a terminal or the advisor seat under a mandate that includes protected paths. Both work on GitHub only.

## When main goes red

<!-- trace:STORY-2897 | ai:claude -->

In both modes, AIDA watches the default branch's CI after each merge. The check runs as its own scheduled job, `land watch` every 5 minutes (enabled in this repository's `.aida/config.toml`), and `aida land watch` runs it once on demand. No landing path runs it, so the land loop and trains never wait on it.

The watch only observes and reports: it writes nothing to the requirement store.

It judges each merged commit in order. A commit is red when a check the CI-gated mode requires concludes as a failure on that commit, counting only the result of that check's latest finished attempt. A cancelled check, a check that is still running and a failure in a check the mode does not require do not make a commit red. Those other failures are listed as diagnostics. A commit whose CI is still running holds the commits after it, so the first red commit is the one named.

When main goes red:

1. AIDA mails the advisor role at once, as urgent. The mail names the first red commit, its pull request and specs, and the failing checks with their links and the error lines of their logs. It also names the last green commit, where the live binary stays. The watch observes independently initiated re-runs.
2. If a green re-run clears the red, the episode is recorded as a flake.
3. If the red survives the re-run, or 30 minutes pass, the red is confirmed and the advisor gets a second mail.
4. Merging continues while main is red. The live binary stays on the newest green commit.

AIDA fixes forward by default. After confirmation it recommends a revert of the red commit by mail, once per reason, only when:

- main does not build: a compile step failed in the current attempt of the failing jobs;
- main is still red `[landing] red_main.fix_window` (default `2h`) after it was confirmed.

A revert is an ordinary pull request with a normal review. The first green commit ends the episode, and AIDA mails that main is green again. A mail that cannot be delivered is retried on every pass until it is. Filing a fix-forward bug and an `aida human` item for a confirmed red is a later slice.

Each watch call to git or the forge has a time limit, and so does the whole pass and the scheduled job. `aida land watch` prints the tip's CI separately from the last green commit, and the open episode if there is one.

`aida land metrics` reports, from `.aida/land/main-watch.jsonl`:

- red-main minutes;
- episodes, with flakes counted separately;
- merges that landed while red;
- fix latency, which excludes flakes;
- how many episodes ended in a revert.

## Two CI tiers on a pull request

For ordinary pull requests under the default repository configuration:

| Tier | Runs when | What it runs |
| --- | --- | --- |
| FAST | a push updates the PR head | source guards and a workspace compile check |
| FULL | `ci:full` is added, or the PR opens or reopens with it | the complete test shards, the same ones a train runs |

A push runs FAST even when the PR carries `ci:full` from an earlier approval.
A fresh approval requests FULL for the updated head.

## Full CI starts when a head is approved

<!-- trace:TASK-3282 | ai:codex -->
Manual and review-watch approvals persist the reviewed SHA as a FULL request.
The two-minute `approved-head-full-ci` job waits for that head’s FAST workflow to finish, then adds
`ci:full` (removing it first if inherited). Removal triggers no workflow. Label-triggered
FULL runs have their own per-PR concurrency group. The train can retry a missing or cancelled run.

An advisor can start the same run by hand:

```bash
gh pr edit <N> --add-label ci:full
```

FULL runs for different pull requests run at the same time, within the
repository's Actions concurrency limits. Reapplying `ci:full` can supersede an earlier label-triggered FULL run
on that PR. Its FAST run continues in a separate group.

Adding any other label (a merge hold, for example) starts a run in which
every job is skipped. That run is ignored: it does not replace the FAST or FULL
run on the head, and a head that carries only such a run has no CI result.

## The train admits only a green full-tier head

`aida pr train` (named PRs, `--all-ready`, and the advisor's automatic train)
checks each approved member's exact head commit. It admits the member only when
that commit has a FULL run, reported as `ci-tier: full`, whose
`Build (ubuntu-latest)` check passed. A FAST run on the same commit does not
count, and neither does a FULL run on an earlier head.

A member without that run is left out, and the reason names what is missing:

| Reason printed | Meaning | What to do |
| --- | --- | --- |
| `no full-tier CI run exists at its head <sha>` | no FULL run has started on this commit | none; the train requests one, and the PR rides a later train |
| `full-tier CI run <id> at its head <sha> has not finished` | the run is in progress | wait for it, then run the train again |
| `full-tier CI run <id> at its head <sha> is red` | the run failed or was cancelled | fix the branch, or rerun the failed jobs for a flake |

With `--all-ready`, a member that is left out waits for the next train. A PR
named on the command line that is left out makes the command exit non-zero.
`--dry-run` reports the same reasons and requests nothing.

The train still runs its own CI on the combined result before it merges. The
full run on each head removes the failures a single PR would cause by itself;
the train's run catches the ones that appear only when PRs are combined.

Before that run, the train regenerates the showcase search data
(`docs/showcase/search-data.js`) and the store-writer inventory
(`scripts/store-writers.toml`) at its tip when two or more of its PRs touch
their inputs. Each generator runs twice in an offline sandbox, and the second
run must change nothing. A changed output becomes one
`[AI:aida-train] chore(train): regenerate generated files` commit after the
last PR's commit. That commit changes only those files, records each file's
hashes and command in its trailers and in the train PR's
`aida-train-regenerated:` block, and completes no spec. In the inventory,
existing rows must stay as reviewed and new rows must start `blocked`.
When a check fails, the train is left as built. Commits that land
before the tip can carry stale generated files; only the tip is tested.
<!-- trace:TASK-3963 | ai:claude -->

## Overriding the check

```bash
aida pr train <N...> --skip-full-check
```

`--skip-full-check` admits members whose head has no green FULL run. Each
admission prints a warning and appends one `train-admit-skip-full-check` entry,
with the PR, head and reason, to `.aida/advisor-activity.jsonl`. The override
applies to that invocation only. Every other train gate still applies. If the
entry cannot be written, the member is not admitted. With `--dry-run` nothing
is admitted, so no entry is written.

## Repositories without tiers

The check applies to a repository whose workflow files under
`.github/workflows/` define a `ci-tier:` job. In such a repository a head with
no `ci-tier: full` run is left out, whatever other checks it carries. A
repository whose CI has a single tier is admitted as before.


## Stuck changes: the throughput guard

<!-- trace:STORY-2906 | ai:claude -->

`aida land stalls` prints which open changes are stuck and who can move them.
It runs the same pass as `aida land status` and reads the same evidence fact by
fact. The reconciler's state is one label in a fixed order (a red head is
`CI-FAILED` before its review history is read), so the guard does not classify
from it: an unreviewed head with red CI is still `STUCK-REVIEW`, and an approved
head behind a hold nothing releases is still `STUCK-MERGE`. The `STATE` column
shows the reconciler's label for reference. Use `--json` for the same report as
JSON. The command only reads and prints; it routes nothing.

| Class | Meaning | Wait counted from | Routed to |
|---|---|---|---|
| `STUCK-MERGE` | approved at the head, not landed for more than `merge_mins` | the approval (the head's commit, when an approval was carried over a newer head) | advisor |
| `STUCK-MERGE-RED` | approved at the head, CI red there, no lane on it, for more than `merge_mins` | the same | advisor |
| `STUCK-REVIEW` | no verdict at the head and no reviewer for more than `review_mins` | the head's commit | advisor |
| `STUCK-REWORK` | changes requested at the head, no lane or session on it, for more than `rework_mins` | the verdict | advisor |
| `STUCK-CONFLICT` | conflicts with its base, nobody on it, for more than `conflict_mins` | the base commit that first conflicted, or the head's commit when that is newer | advisor |
| `moving` | none of the above | - | - |

Each wait counts from the event that started it, not from when the guard first
looked, so a head pushed hours ago is stuck on the first run that sees it. The
head's time is its commit date. A change whose time or evidence cannot be read
is listed under `Not read` and is not called stuck. A train, a promotion, a
change under review and an approved change stacked on another change's branch
are moving. An unreviewed red head with a lane on it is moving too: the lane is
about to replace the head.

A `STUCK-MERGE-RED` line names the failing tests and says why CI is red. Each
failed check is judged on its own, and the line gives one reason per check:

- **probable flake**: no failing test of the check is in a file the change
  touches, and the same check passes at the base branch's tip.
- **the change plausibly caused it**: the change touches a file a failing test
  covers.
- **the base branch is red**: the same check is red on the base too.
- **undetermined**: the check's log, the changed files or the base's CI could
  not be read, or the log names no test. The line says which.

The action follows from the checks together. Any check the change plausibly
caused means rework. Otherwise any undetermined check means reading the failed
job: a red head is called a flake only when every failed check was read and is
one. Then the action is one rerun, and if the rerun fails again, a fix-forward
for the test, not rework of the change. A check that is red on the base adds a
fix-forward of the base to whatever the other checks call for; it never
excuses them.

Tests are matched to files by module path (`queue::tests::drains` matches
`src/queue.rs`), so this is a heuristic and the line always shows what it
compared. Failing test names are read on GitHub only; on another forge the line
says so. One run reads the job logs of at most `max_red_logs` red changes and
spends at most 75 seconds on CI detail. A log read once is kept, so the next
runs read the changes that are still missing; anything not read yet is
reported as undetermined, with the reason.

The report also counts landings on the default branch in the last
`landings_window_mins` and lists live lanes per host, with available memory for
the host the command runs on. Another host's memory is not measured from here;
that host's own scheduled run records it.

The scheduled `throughput-guard` job runs `aida doctor check throughput-guard`,
which computes the same report. On the scheduler's tick each stall not routed
before goes out once: one `throughput-guard-route-<seat>` due job per seat,
carrying all of that seat's current stalls, and a short mail to the seat. If
the mail cannot be written, the next run sends it again without raising the due
job a second time. A change with a new head, or a red whose remedy changed, is
a new stall. When nothing has landed for `escalate_after_mins` while something
is stuck, the operator gets `throughput-guard-route-operator` in `aida awaiting`
(at most once per window) and a push if `[notify]` is configured. No model
runs, and the guard never launches, merges or releases anything.

Every scheduled run, including one that finds nothing stuck, saves what it
observed in `.aida/throughput-guard/state.json` under `runs`: the time, the
landings in the window, each host's lane count and memory, and how many changes
were open and stuck. The last 72 runs are kept, and `aida land stalls` prints
the newest.

The guard is off unless the project sets `[throughput_guard] enabled = true`
and enables the `throughput-guard` job; see `[throughput_guard]` in
[Project setup](../cli/07-project-setup.md#throughput-guard).

## Landing metrics

<!-- trace:TASK-3813 | ai:codex -->

`aida land metrics --since 7d` reports hourly merges from local `main`'s
first-parent history, each READY hour's merge activity, READY-to-merge waits,
waits exceeding two hours, and mean/median requested-change rounds before the
last approval for landed PRs. Use `--format json` for the same report with
source labels. Windows accept hours or days; boundary hours are clipped to the
requested window. Update local main before reporting if fresh merge data is
needed. Only commits identifying a PR (`(#N)` or `Merge pull request #N`) count.

Every status/run pass, including a dry run, now appends readiness changes to
`.aida/land/readiness.jsonl`. An initial marker records when observation began,
even with no open PRs. The log records observed state, not the unobserved time
between passes. A changed head begins a new observation; a disappeared PR ends
its observed interval. A permanent nonblocking lock serializes recorders; a
busy recorder refuses the pass. The adjacent cursor is a disposable incremental
index and can be removed for recovery; keep the event log.

Before recording began, approvals supply an estimate labelled **approx (no
READY record)**. Consecutive approvals preserve the estimate's start; a
blocking verdict (including WAITING or REJECTED) ends it. READY-hour entries
keep their exact or approximate source labels. A continuous wait crossing the
recording boundary has its full duration reported under **mixed (approval
estimate + READY event log)**, including the estimated prefix for the two-hour
threshold. Other exact observations and estimates have separate wait summaries.
Legacy archives, current verdicts and ledger acts contribute to review history;
only REQUEST CHANGES counts as a requested-change round.
Missing approval records are listed rather than counted as zero review rounds.
Review acts copied under several specs are counted once. Groups use the
approving reviewer's vendor and the covered spec-ID types. A mixed change
contributes once to each type it covers; missing metadata is `unknown`. The generated recorder suffix
supplies vendor metadata for older verdicts. These observations grant no landing,
review or completion authority.
