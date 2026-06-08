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

This returns `{ ready: [...], parked: [{spec, reason}] }`. **Only `ready` is
fan-out-able.** Report the parked count + reasons once (so the operator sees
what's held + why) but never act on parked specs. If `ready` is empty, stop —
the backlog is drained for this selector.

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

For up to **N** of the remaining ready specs (a bounded wave; `N ≈ 4`, or scale
to budget), spawn one **worktree-isolated implementer subagent per spec**:

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
summary: specs completed, any parked-with-reason, and what's left needing the
operator.

## Guardrails

- **The gate is law.** Never fan out a spec `aida burndown plan` put in `parked`
  — it's parked because it needs a human (epic to decompose, pending decision,
  unsatisfied blocker, or a parking tag).
- **CI gates `main`.** A bad change parks (CI red → no merge); it never reaches
  `main`. That's what lets the integrator merge greens without re-reviewing each.
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
