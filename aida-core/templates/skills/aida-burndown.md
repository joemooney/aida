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

For up to **N** of the remaining ready specs (a bounded wave), spawn one
**worktree-isolated implementer subagent per spec**. **N = the `--concurrency`
value from `$ARGUMENTS` if provided** (so `aida burndown run --concurrency 6`
flows through), else `N ≈ 4` — or scale to budget:

> `Agent(subagent_type: "general-purpose", isolation: "worktree")` — each gets
> ONE ready spec and a self-contained prompt: read it (`aida show <SPEC> -c`),
> implement to acceptance, add `// trace:<SPEC>` (plain `//`, never `///`),
> `cargo build` + `cargo test` + `cargo fmt --all -- --check` (check the exit
> code), commit `[AI:claude] type(scope): … (<SPEC>)` + the co-author trailer,
> push, open a PR, and reply ONLY the PR URL or `BLOCKED: <reason>`.

Worktree isolation means parallel agents never collide on files.

### 3. Integrate (you are the integrator — do NOT implement)

For each returned PR: if all checks pass and it's mergeable + clean, merge it
(`--squash --delete-branch`), `aida db reconcile-status --spec <SPEC>` to bump
it Completed, and pull. **Hold (do not merge) any PR whose spec is
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
blockers clear), launch the next wave, and repeat until `ready` is empty. For an
unattended drain, schedule the next wave via a wake-up rather than blocking.

### 6. Report

When the ready set is empty (or `--max` reached), send a `PushNotification`
summary: specs completed, any parked-with-reason, any worktrees that couldn't be
pruned (flagged + skipped in step 3), and what's left needing the operator.

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

trace:STORY-527 | ai:claude
