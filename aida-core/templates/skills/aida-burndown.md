---
name: aida-burndown
description: Autonomously burn down a backlog — fan out worktree-isolated implementer subagents over the ready set, integrate their PRs, and loop until drained. Wraps the empirically-working autonomous-drain pattern so the "never stop to ask" rules are structural. Use when the user asks to "burn down the backlog", "drain the approved work", or run a hands-off multi-spec session. Reads the ready set from `aida burndown plan`.
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
`docs/aida/discipline/autonomous-burndown.md`) as one command, so the rules
that keep it from stalling are **structural**, not something the driving agent
must remember. The engine is Claude-Code-native and only the harness can run it:
fan out implementer subagents in isolated worktrees, integrate their PRs, loop.
The subagent fan-out is the harness's native `Agent(isolation: "worktree")`
primitive, so this is **Claude-harness-only** — a non-Claude vendor (Codex,
Cursor, Amp) has no equivalent and drains the same ready set the serial way
instead: `aida queue work --auto-complete` per spec (the vendor-agnostic
orchestrator engine). SPIKE-74 tracks closing that gap.

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

This returns `{ ready: [...], awaiting_signoff: [...], parked: [{spec, reason}] }`.
**Only `ready` is fan-out-able.** `ready` is the advisor-blessed drain set:
queued + bounded + unblocked + decision-free — queue membership IS the advisor
sign-off (STORY-546), so the runner can never drain a spec the advisor didn't
deliberately queue. `awaiting_signoff` is pickable-but-unqueued (the advisor
hasn't blessed it yet) — report it but never act on it. Report the parked count
+ reasons once (so the operator sees what's held + why) but never act on parked
specs. If `ready` is empty, stop — nothing blessed to drain for this selector.

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

For up to **N** of the remaining ready specs (a bounded wave), spawn one
**worktree-isolated implementer subagent per spec**. **N = the `--concurrency`
value from `$ARGUMENTS` if provided** (so `aida burndown run --concurrency 6`
flows through), else `N ≈ 4` — or scale to budget:

> `Agent(subagent_type: "general-purpose", isolation: "worktree")` — each gets
> ONE ready spec and a self-contained prompt: read it (`aida show <SPEC> -c`),
> implement to acceptance, add `// trace:<SPEC>` (plain `//`, never `///`),
> `cargo build` + `cargo test` + `cargo fmt --all -- --check` (check the exit
> code), commit `[AI:claude] type(scope): … (<SPEC>)` + the co-author trailer,
> push, open a PR, then mark the spec done (`aida queue done <SPEC>`) so it
> reaches **Done** and the normal merge-driven Done→Completed auto-bump fires on
> integration, and reply ONLY the PR URL or `BLOCKED: <reason>`.

Worktree isolation means parallel agents never collide on files *mid-flight* —
but two specs that touch the SAME files still **conflict at MERGE time** (the
stacked / duplicate-edit hazard).

**Sequential work in one worktree stacks branches (BUG-554).** The fan-out above
is safe because each subagent gets a FRESH worktree off `origin/main`. The hazard
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
  each member is its OWN PR off freshly-pulled main, one member at a time
  (concurrency 1). A member failure **shelves** that member and the drain
  **continues** with the rest. Use when the members are coupled but each
  increment is independently shippable + reviewable.
- **`aida queue work --batch NAME --auto-complete --single-branch`** — all
  members accumulate on ONE shared branch in one worktree, no per-member
  merge-to-main, ONE cluster PR at the end. A member failure **halts** the drain
  (later increments build on earlier commits, so it stops rather than build on
  broken code). Use when the members must ship together.

The one-line rule of thumb: **`--sequential` shelves-and-continues (independent
PRs); `--single-branch` halts (one accumulating branch).** Both replace the old
manual `git reset --hard origin/main` between members — let the mode base each
member correctly instead of driving `git` by hand. trace:BUG-554 trace:TASK-1005 | ai:claude

**Never co-fan a serialize-group (STORY-614).** Specs that must not run
concurrently carry a shared `serialize:<group>` tag (e.g. `serialize:docs`,
`serialize:burndown-display`). When selecting the N specs for a wave, check each
ready spec's tags and include **AT MOST ONE** spec per `serialize:<group>` value;
the rest of that group drain in **successive** waves, after the first one lands
and merges. Independent specs still fan out in parallel — only the tagged
collision-set is serialized. This is the operator marking known file-overlap
("these touch the same code") so the drain enforces the ordering instead of
relying on someone remembering `--concurrency 1`. (A typed `ConflictsWith` edge
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
recovery, not license to end the turn early.) A
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

For each returned PR: if all checks pass and it's mergeable + clean, merge it
(`--squash --delete-branch`) and pull — the spec was marked **Done** by its
implementer (step 2), so the merge-driven auto-bump promotes it to **Completed**
on pull. As a fallback (e.g. the implementer skipped `queue done`, or the
auto-bump missed), run `aida db reconcile-status --spec <SPEC>` to force the
Completed bump. **Hold (do not merge) any PR whose spec is
`review:draft-only`** — leave it a draft for the operator. On a merge conflict,
have the agent rebase (`git merge origin/main`, resolve, push).

**Prune the merged implementer worktree + branch (per merged spec).** After a
PR's merge succeeds **and** its spec auto-bumped to **Completed** (confirm with
`aida show <SPEC>` — status `Completed`), reclaim that implementer's
worktree-isolated branch and worktree. They are NOT cleaned up automatically:
`Agent(isolation: "worktree")` auto-cleans only **unchanged** worktrees, and a
committed + merged implementer worktree HAS changes, so it persists by design —
the integrator must prune it explicitly, or a long unattended drain accumulates
stale `.claude/worktrees/agent-*` worktrees that also block `git branch -d`.

For each just-merged Completed spec:

1. Identify the implementer's branch (the spec's own branch, e.g.
   `task-NNN-…` — the `headRefName` from the `gh pr list` entry) and its
   worktree path (the `.claude/worktrees/agent-*` or `~/ai/aida-<spec>` dir).
   `git worktree list --porcelain` maps branches → worktree paths.
2. Remove the worktree, then delete the local branch:

   ```
   git worktree remove --force <that worktree path>
   git branch -d <that branch>            # -d = safe: refuses if not merged
   git push origin --delete <that branch> # only if --delete-branch above didn't already
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
events). Keep a **long-interval** `ScheduleWakeup` (e.g. 30–60 min) as the
documented degenerate fallback: if no event stream is live, or the watcher
wedges, the timer still resurfaces the loop — correctness never depends on the
event path.

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
  is what catches that. Both gates together are what let the integrator merge
  greens without re-reviewing each.
- **Keep at the keyboard, not the drain:** releases/tags and changes to the
  autonomy machinery itself (the orchestrator, this runner) ship supervised — a
  fix riding through a broken drain gets caught in the breakage.

## Relationship to the orchestrator drain

This is the **recommended** hands-off backlog-drain path. It deliberately uses
the harness's native subagent fan-out rather than `aida queue work
--auto-complete` (the orchestrator-spawns-agent path), which is hardened in
parallel. They are **not** competitors: reach for `/aida-burndown` to drain a
*ready set*; reach for the orchestrator drain when its single-spec lifecycle is
what you want. Don't run both against the same set.

trace:STORY-527 trace:TASK-792 trace:TASK-992 | ai:claude
