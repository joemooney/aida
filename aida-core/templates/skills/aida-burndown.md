---
name: aida-burndown
description: Autonomously burn down a backlog — launch one standalone implementer process per ready spec, integrate their PRs, and loop until drained.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Agent
  - PushNotification
---

# AIDA Burn-Down Skill

## Purpose

Run the autonomous backlog burn-down that empirically works (memory
`feedback_parallel_implementer_fanout_burndown`; discipline
`.aida/discipline/autonomous-burndown.md`) as one command, so the rules
that keep it from stalling are **structural**, not something the driving agent
must remember. The engine launches one standalone implementer process per
spec (`aida queue work <SPEC> --role implementer --no-human`), each in its own
worktree and implementer seat, integrates their PRs, and loops. A harness that
cannot run background shell jobs drains the same ready set the serial way
instead: `aida queue work --auto-complete` per spec (the vendor-agnostic
orchestrator engine).
<!-- trace:STORY-1662 | ai:claude -->

The safety property is delegated to `aida burndown plan` (STORY-527 slice 1):
**only bounded + unblocked + decision-free specs are ever fanned out.** That is
what makes "never stop to ask" safe — the runner can't drag in work that needs a
human, because the gate already excluded it.

## When to use

- "Burn down the backlog", "drain the approved work", "clear the ready set".
- A hands-off multi-spec session while the operator is away.

## Skip if

- There's one specific spec to do → use `/aida-pickup`.
- You want a single-spec lifecycle with the orchestrator → `aida queue work <id> --auto-complete` (see "Relationship to the orchestrator" below).

## Procedure

### 1. Resolve the ready set (the gate)

Parse the selector from `$ARGUMENTS` (default `--status approved`):

```
aida burndown plan --status approved --json     # or --tag <T> / --batch <B>
```

This returns
`{ ready: [...], awaiting_signoff: [...], serialize_held: [{spec, held_by, group}], supervised: [...], parked: [{spec, reason}] }`.

**Only `ready` is fan-out-able.** `ready` is the advisor-blessed drain set:
queued + bounded + unblocked + decision-free — queue membership IS the advisor
sign-off (STORY-546), so the runner can never drain a spec the advisor didn't
deliberately queue.

The non-ready buckets are three DIFFERENT states — do not conflate them:

- `awaiting_signoff` — pickable but **not queued**: the advisor has not blessed
  it yet. Report it, never act on it. Clearing it needs a human/advisor
  (`aida queue add <id>`).
- `serialize_held` — **queued and blessed**, held out of *this* wave only
  because a sibling already claimed its `serialize:<group>` (see "Never co-fan a
  serialize-group" below). Nothing is awaiting a human. Report it as
  "held behind `held_by` in serialize group `group` — drains in a later wave",
  and never act on it *this* wave; it becomes `ready` on the next resolve once
  the claiming spec lands.
- `parked` — mechanically blocked (blocker, pending decision, deferred tag).
  Report the count + reasons once so the operator sees what's held + why, but
  never act on parked specs.

If `ready` is empty, stop — nothing blessed to drain for this selector.

### 2. Fan out a wave (the engine)

**First, drop any ready spec that's already in flight.** The gate is
pure/store-only — it can't see transient forge or session state, so a spec
mid-merge (open PR) or actively being worked (live session lease) can still come
back `ready`. Spawning a second implementer for it duplicates effort and races.
Before fanning out, filter the ready set against both:

```
gh pr list --state open --json headRefName,title    # specs with an open PR
aida session leases                                  # specs with an active lease
```

Skip any ready spec whose SPEC-ID matches an open PR branch/title or an active
lease scope; carry only the genuinely-idle remainder into the wave below.

> **Do NOT invent a "competing drain" check beyond the two filters above.**
> When `AIDA_BURNDOWN_LOCK_HELD` is set in your environment, the launcher
> (`aida burndown run`) is already holding the **exclusive** drain lock — so by
> construction there is **no** other live drain or orchestrator racing this set.
> A `drain-state.json` you may see is either yours or a stale tombstone; do not
> read it and do not "hold to avoid double-driving." If a spec passes the
> open-PR + active-lease filters, **fan it out.** Treating the launcher's own
> lock/state as a competitor is the BUG-607 self-deadlock (the drain reports a
> live `pid …` that is actually its own parent and then refuses to do anything).

For up to **N** of the remaining ready specs (a bounded wave), launch one
**standalone implementer process per spec** from Bash, as a
`run_in_background` job:

```bash
aida queue work <SPEC> --role implementer --no-human --vendor claude
```

Each gets its own worktree, lease and implementer seat (never your grant),
opens its own PR and stops, and ends with one `implementer:` line whose exit
code you branch on: 0 PR opened, 31 no PR, 32 punted, 33 held, 34 the
concurrency cap refused it (nothing set up; launch it in a later wave).
**N = the `--concurrency` value from `$ARGUMENTS` if provided** (so
`aida burndown run --concurrency 6` flows through), else 3, and never more
than `[queue] standalone_implementer_cap`.
<!-- trace:STORY-1662 | ai:claude -->

Never fan implementers out through the Agent tool. An in-session subagent
shares your session and seat: it trips the advisor code guard, and you become
an author of the PR it opens. The pickup moves each spec to In Progress and
sets its owner, so queue and status views agree it is being worked (BUG-754).
If the standalone launch is unavailable (an older `aida`), drain the set
serially with `aida queue work <SPEC> --auto-complete` instead.

Worktree isolation means parallel agents never collide on files *mid-flight* —
but two specs that touch the SAME files still **conflict at MERGE time** (the
stacked / duplicate-edit hazard).

**Sequential work in one worktree stacks branches (BUG-554).** The fan-out above
is safe because each implementer gets a FRESH worktree off `origin/main`. The hazard
is *reuse*: if a single agent (or you) works MORE THAN ONE spec in the SAME
worktree, branch each spec off `origin/main`, **never** off the current HEAD —
otherwise spec B's branch stacks on spec A's unmerged commit, which (1) pollutes
B's PR with A's commit and (2) makes A's commit reachable from two branches, so
`aida human` / `aida queue list` mis-attribute A to B's branch. Before each next
spec in a reused worktree: `git reset --hard origin/main` (or a fresh `git
worktree add -b <branch> <path> origin/main`). `aida session start` already bases
on `origin/main` and warns when cwd is on a feature branch (BUG-76); raw `git`
does not — so this rule is on you when you drive `git` directly.

**Route coupled file-sharing sets to a drain mode — do NOT fan them out in
parallel and do NOT hand-drive the reset.** The parallel fan-out (one worktree
per spec) is for INDEPENDENT specs. For a set that shares files / must land in
order, tag the members `batch:NAME` and drain the batch instead of fanning out:

- **`aida queue work --batch NAME --auto-complete --sequential`** — ordered,
  each member is its OWN PR off freshly-pulled main. The flag governs ORDER and
  the per-member-PR shape, not concurrency: members run strictly one at a time
  at the default `[drain] pipeline_depth = 1`, and raising that (max 3) lets a
  later member's implementer/CI leg overlap an earlier member's wait while
  merges stay serial. That depth applies to a **single-batch** drain only — a
  `--batches A,B,C` chain is serial at any depth. A member failure **shelves**
  that member and the drain **continues** with the rest. Use when the members
  are coupled but each increment is independently shippable + reviewable.
- **`aida queue work --batch NAME --auto-complete --single-branch`** — all
  members accumulate on ONE shared branch in one worktree, no per-member
  merge-to-main, ONE cluster PR at the end. A member failure **halts** the drain
  (later increments build on earlier commits, so it stops rather than build on
  broken code). Use when the members must ship together.

The one-line rule of thumb: **`--sequential` shelves-and-continues (independent
PRs); `--single-branch` halts (one accumulating branch).** Both replace the old
manual `git reset --hard origin/main` between members — let the mode base each
member correctly instead of driving `git` by hand. trace:BUG-554 trace:TASK-1005 trace:TASK-185 | ai:claude

**Never co-fan a serialize-group (STORY-614).** Specs that must not run
concurrently carry a shared `serialize:<group>` tag (e.g. `serialize:docs`,
`serialize:burndown-display`). When selecting the N specs for a wave, check each
ready spec's tags and include **AT MOST ONE** spec per `serialize:<group>` value;
the rest of that group drain in **successive** waves, after the first one lands
and merges. Independent specs still fan out in parallel — only the tagged
collision-set is serialized. This is the operator marking known file-overlap
("these touch the same code") so the drain enforces the ordering instead of
relying on someone remembering `--concurrency 1`.

The gate already enforces this for you: `burndown plan` collapses the groups
before it reports, so the held members arrive in the **`serialize_held`** key
(each naming its `held_by` claimer + `group`), not in `ready` and not in
`awaiting_signoff`. Treat them as blessed work scheduled for a later wave — do
not tell the operator they need sign-off. (A typed `ConflictsWith` edge
enforced in `resolve_burndown_sets` is the substrate-v2 follow-up — STORY-614;
the tag is the working slice.)

### 3. Integrate (you are the integrator — do NOT implement)

**Wait for each fanned-out PR's CI to reach a TERMINAL state before deciding its
fate — and never exit the drain while any PR is still pending (BUG-541).**
**Background watches do not exist in a headless session (BUG-755): the session
terminates when your turn ends, so a promised "background watch" dies with it
and the PR is silently orphaned. Never promise one — poll in the FOREGROUND
(`gh pr checks <n>`) and never end your turn while a wave PR is non-terminal or
blessed specs remain unstarted.** (The `aida burndown run` launcher relaunches a
continuation turn if a session exits with residual work — but that safety net is
recovery, not license to end the turn early.)

**A wait longer than your foreground budget is the LAUNCHER's job, not yours
(TASK-1169).** A foreground poll is capped at about ten minutes per call, so a
slow CI run cannot be covered by one. Keep polling in the
foreground while you are working the wave — that is still the fast path — but
if a PR's CI genuinely outlasts your turn, **do not background it and do not
claim a watch**: end the turn having said plainly which PRs are still
non-terminal. The launcher then blocks on their CI in Rust (no ten-minute cap,
no turn-end reaper), merges the clean ones, holds supervised ones, and parks
anything else `NeedsAttention` with a finding. Never invent a background watch
to bridge the gap — the launcher already covers it, truthfully. A
PR's CI lags the implementer's push by minutes, so the LAST wave's PRs are
routinely still running when their implementer returns; treating "checks
pending" as "skip it" silently orphans the final PR (observed 2×: #852, #864
were stranded open+unmerged for the advisor to back-merge). For each returned
PR, poll `gh pr checks <n>` (or `gh pr view <n> --json statusCheckRollup`) until
it is **conclusive** — every required check SUCCESS or any FAILURE, not PENDING
— *before* merging or holding it. Do **not** declare the drain complete until
every PR it opened has reached a terminal state and been merged, held, or
explicitly listed as a straggler (step 6). The last PR is the one this bug
strands — give it the same wait every other PR got.

**Review every PR in a separate reviewer process before merging it.** The
merge gate accepts only an approval recorded at the PR's head by a reviewer
process with its own identity: an approval you record, or one recorded by an
Agent-tool reviewer inside your session, is refused at merge. Launch
each reviewer from Bash as its own `run_in_background` job, one job per PR,
and wait for the job to exit. The job's exit status is the signal, not the
verdict file:

```bash
aida queue work PR-<n> --role reviewer --no-human --vendor claude \
  > .aida/burndown-review-PR-<n>.log 2>&1
echo "exit=$?" >> .aida/burndown-review-PR-<n>.log
```

When the job's completion arrives, read the `exit=` line and the summary's
`verdict:` line from that log. A shell `&` plus `wait` works only inside a
single Bash call, so don't split the launch and the wait across calls.

- **Exit 0 and a `verdict:` line** (not `UNKNOWN`): the launcher checked the
  verdict against the PR head and this launch's reviewer grant. Only now
  read `.aida/review-verdicts/PR-<n>.json`.
- **Exit 30, or any other non-zero exit**: no verified verdict. Read the
  log and launch a new reviewer; never record one yourself.

Never poll for the verdict file or treat its existence as completion. The
reviewer writes that file in place while it is still running, a file that
fails the launcher's checks stays on disk, and in a rework round the file
from the previous round is there until the new launcher replaces it.

- Never use the Agent tool for review, never write the reviewer's prompt, and
  never run `aida review record` yourself. The reviewer process reads the spec
  and any operator checklist comments itself.
- At most 3 reviewer processes at once, one per PR. Don't pass `--sandbox`.
- Always pass `--vendor claude`: under `AIDA_CONTAINED` the codex vendor
  refuses.
- If the launch refuses with "branch pr-<n> is held by worktree ..." (a previous
  reviewer died or ran without `--no-human`), run
  `aida session end --spec PR-<n> --remove --skip-ci --yes` and
  `git branch -D pr-<n>`, then relaunch. If it refuses with "base is N commits
  behind ... overlaps", have the author merge main into the branch first.
- The burndown seat must have been entered with
  `aida role enter <seat> --delegate-seat reviewer`; without it the launch
  fails closed. Report that as a blocker; don't work around it.
- `RequestChanges` → route the findings back to the implementer: relaunch
  `aida queue work <SPEC> --role implementer --no-human` on the existing
  branch, or mail an interactive lane (`aida mailbox send --to <lane> ...`).
  Then launch a **new** reviewer at the new head.
  <!-- trace:STORY-1662 | ai:claude --> A stale approval
  also means a new reviewer; never use `--override-stale-approval`.
<!-- trace:STORY-1654 | ai:claude -->

For each returned PR: if all checks pass, it's mergeable + clean, and a
reviewer process approved its current head, merge it with
`aida pr ship <n> --no-cleanup`. It re-checks the approval, squash-merges,
leaves a branch that a sibling worktree still holds, and pulls. If it refuses
the approval, don't merge another way: launch a new reviewer. The spec was
marked **Done** by its implementer (step 2), so the merge-driven auto-bump
promotes it to **Completed** on pull (run `aida pull` yourself if the ship
reported the pull failed). Branch deletion happens in the prune step
below, after the worktree is removed. As a fallback (e.g. the implementer skipped `queue done`, or the
auto-bump missed), run `aida db reconcile-status --spec <SPEC>` to force the
Completed bump. **Hold (do not merge) any PR whose spec is
`review:draft-only`** — leave it a draft for the operator. On a merge conflict,
have the agent rebase (`git merge origin/main`, resolve, push).

**Prune the merged implementer worktree + branch (per merged spec).** After a
PR's merge succeeds **and** its spec auto-bumped to **Completed** (confirm with
`aida show <SPEC>` — status `Completed`), reclaim that implementer's
branch and worktree. They are NOT cleaned up automatically: a committed and
merged implementer worktree persists by design, so the integrator must prune
it explicitly, or a long unattended drain accumulates stale worktrees that
also block `git branch -d`.

For each just-merged Completed spec:

1. Identify the implementer's branch (the spec's own branch, e.g.
   `task-NNN-…` — the `headRefName` from the `gh pr list` entry) and its
   worktree path.
   `git worktree list --porcelain` maps branches → worktree paths.
2. Remove the worktree, then delete the local branch:

   ```
   git worktree remove --force <that worktree path>
   git branch -d <that branch>            # -d = safe: refuses if not merged
   git push origin --delete <that branch> # remote cleanup (the merge deliberately skipped branch deletion)
   ```

**Prune guards (follow these exactly — they bound the blast radius):**

- **Completed-only.** ONLY prune a worktree whose spec reached **Completed**
  (i.e. its PR merged and the auto-bump landed). NEVER prune a worktree for a
  spec that is still held / `review:draft-only` / in-flight / `NeedsAttention` —
  that work is unmerged and the worktree is live.
- **Never the integrator's own tree.** NEVER `git worktree remove` the
  integrator's main worktree or the repo root — prune ONLY the implementer
  worktree that produced the just-merged PR.
- **Best-effort + non-fatal.** A worktree that won't remove (e.g. uncommitted
  unrelated changes from another agent) is **flagged and SKIPPED** — note it for
  the operator and continue. A prune failure must NEVER abort the wave or the
  loop; pruning is reclamation, not an integrity gate.

**Verify the integrated `main` before looping (BUG-496).** After a wave's PRs
are merged, the merges are *integrated but un-tested together* — each PR's CI ran
against the **old** base, not the post-merge result, so two PRs that were green
alone can break `main` **together** (the squash-merge parallel-integration
hazard). So once a wave's PRs are merged: `git checkout main && git pull
--ff-only && cargo build -p aida-cli` (a quick compile is enough to catch the
usual integration breaks — a signature/import/type mismatch). **If it fails,
HALT the drain** — do **not** launch the next wave, and do **not** report
success. Fix-forward the break if it's mechanical, else park it and alert the
operator with the build error. **Never loop, and never declare "complete," over
a red `main`** — "every PR was green" is not "main is green" for a parallel wave.

### 4. Punt-and-continue (non-negotiable)

A blocker parks **one** spec — tag it + leave a note — and the pipeline rolls
on. One spec's failure must **never** halt the wave or the loop. **Never stop to
ask; never down tools.** A fork → make the defensible call or park that one
spec, then move to the next.

### 5. Loop until drained

Re-run `aida burndown plan` (the ready set shrinks as specs land + may grow as
blockers clear), launch the next wave, and repeat until `ready` is empty.
**Looping is not optional (BUG-755): stopping after wave 1 with blessed specs
unstarted is an incomplete drain.** Only a shelve/cap stop (`--max` reached, or
a shelved failure you are exiting non-zero over) may end the run with blessed
specs remaining — and then the report (step 6) must name them explicitly.

For an **unattended** drain, wait on the wave **event-driven**, not on a blind
timer. Launch the harness `Monitor` tool over the drain's wake feed:

```
Monitor(command: "aida watch --emit-wakes", persistent: true)
```

`aida watch` tails the drain's event stream and prints a line **only** on an
actionable verb — a PR shipped or merged, a CI verdict, a punt, a shelve, the
queue drained — staying silent through the benign phase churn. The session burns
**zero tokens while the wave runs** and wakes exactly when there is something to
integrate, so supervision cost drops from O(time-elapsed) to O(actionable
events). If the watcher is unavailable, fail visibly and restart that
shell-side wait; never substitute model-side `CronCreate`, `/loop`, or
`ScheduleWakeup`. Those mechanisms reload the full model context on every
quiet tick. Correctness depends on the structural termination check after an
actionable event, never on elapsed time. <!-- trace:BUG-1589 | ai:codex -->

### 6. Report

When the ready set is empty (or `--max` reached), report the drain on **two
channels** — they reach different audiences and neither replaces the other:

1. **`PushNotification`** — the ambient at-the-keyboard ping (specs completed,
   parked-with-reason, worktrees that couldn't be pruned, what's left).

2. **Post the completion summary to the advisor mailbox** so the *narrative* —
   the caveats and reasoning a `PushNotification` can't carry — lands in the
   advisor session's unread notice / inbox **without the operator relaying it.**
   Outcomes already reach the advisor automatically (merged PRs via the monitor,
   completed specs via the cache); what *doesn't* is the
   reasoning/caveats/escalations, so the operator has been cut-n-pasting every
   drain report. This step closes that paste loop — the burn-down analog of
   STORY-569's `--zen` clean-finish → review brief; same substrate-handoff
   principle, applied to the drain's completion report:

   ```
   aida mailbox send --to advisor --intent handoff "Burn-down complete (<selector>).

   Completed: <SPEC + PR url per landed spec>
   Caveats / verifications worth noting: <e.g. 'TASK-X passed CI but the integrated-main build was only a compile check'>
   Held / awaiting sign-off: <review:draft-only PRs left for the operator>
   Left for manual merge: <un-merged PRs — CI not yet terminal at exit, or an unresolved conflict; 'none' if the step-3 wait drained them all (BUG-541)>
   Parked (with reason): <spec + why, from the gate's parked set + any punt-and-continue parks>
   Worktrees not pruned: <flagged + skipped paths from step 3, if any>"
   ```

   **Any un-merged PR MUST appear on the `Left for manual merge` line in BOTH
   channels (BUG-541).** If step 3 waited correctly this is normally `none`; but
   if a PR's CI was still pending at `--max`, or a conflict blocked the merge,
   name it explicitly (`#N <url>`) — silence reads as "all merged" and the PR is
   silently orphaned.

   - Use `--intent handoff` for a normal completion report. Use
     `--intent request` instead if the summary contains something that needs an
     advisor **decision** (e.g. a parked spec the advisor must triage). Add
     `--urgent` only if the drain hit a `main`-breaking HALT (step 3) the advisor
     must see out-of-band.
   - Keep the body to the lines above — specs+PRs, caveats, held,
     parked-with-reason, unpruned worktrees. Omit a heading that has nothing
     under it (don't post empty lines). One message per completed drain.
   - **Best-effort:** a failed mailbox send must **never** retroactively fail an
     otherwise-complete drain — note it and continue.

## Guardrails

- **The gate is law.** Never fan out a spec `aida burndown plan` put in `parked`
  — it's parked because it needs a human (epic to decompose, pending decision,
  unsatisfied blocker, or a parking tag).
- **CI gates each PR — the integrated-`main` verify gates the wave.** A bad
  change parks (CI red → no merge), so no *single* bad PR reaches `main`. But CI
  ran each PR against the *old* base, so a parallel wave can still break `main`
  *together* (BUG-496) — the per-wave `cargo build` on integrated `main` (step 3)
  is what catches that. Both gates catch build breaks; neither replaces the
  per-PR reviewer process, whose approval the merge gate requires.
- **Keep at the keyboard, not the drain:** releases/tags and changes to the
  autonomy machinery itself (the orchestrator, this runner) ship supervised — a
  fix riding through a broken drain gets caught in the breakage.

## Relationship to the orchestrator drain

This is the **recommended** hands-off backlog-drain path. It deliberately
launches standalone implementers in parallel rather than `aida queue work
--auto-complete` (the orchestrator drain, one per repository at a time). They are **not** competitors: reach for `/aida-burndown` to drain a
*ready set*; reach for the orchestrator drain when its single-spec lifecycle is
what you want. Don't run both against the same set.

trace:STORY-527 trace:TASK-792 trace:TASK-992 trace:BUG-754 | ai:claude
