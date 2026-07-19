---
name: aida-solo
description: The WARM interactive solo-driver — put THIS Claude session in the solo advisor+integrator seat and run the supervised loop with live judgment (groom → implement → integrate → repeat) until the safe backlog is drained. The better-judgment counterpart to the headless cold-boot `aida solo run`. Use when the user says "drive the backlog solo", "work the safe backlog end-to-end", or "be the advisor and integrator". Keystone work (security, architecture, the autonomy machinery) is PARKED for the human, never shipped unattended.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Agent
  - Read
  - Grep
  - Glob
  - PushNotification
  - AskUserQuestion
---

<!-- trace:STORY-668 -->

# AIDA Solo Skill — the warm interactive solo-driver

## Purpose

Put the **current (warm) Claude session** in the solo advisor+integrator seat
and run the supervised loop end-to-end **with live judgment** — groom →
implement → integrate → loop — until the safe backlog is drained. This is the
**better-judgment counterpart** to the headless cold-boot `aida solo run`: same
loop shape, but a session that holds the project context makes the
worth-doing / dedup / park / keystone calls instead of a fresh `claude -p` per
tick.

It **composes the existing verbs** — `aida intake --apply` / `aida backlog
groom`, the worktree-isolated implementer fan-out, `aida queue integrate` — and
adds the one thing the CLI deliberately does not: **the judgment.** No new
approval path, no new queue path, no new merge path.

The safety floor (drain lock, keystone exclusion, escalation mailbox) and the
full runbook live in `docs/solo-mode.md` — read it before the first run.

## When to use

- "Drive the backlog solo", "work the safe backlog end-to-end", "be the advisor
  and integrator for a while", "groom-implement-integrate on a loop".
- A supervised at-the-keyboard session where you want the whole loop, not just
  one phase — and you want a session with real context making the calls.

## Skip if

- You want it **fully hands-off / cold-boot** → `aida solo run` (the headless
  engine; see "How this differs" below).
- You only want to **fan out implementers** over a ready set → `/aida-burndown`.
- You only want to **drain a role's queue** (implement-only) → `/aida-drain-queue`.
- There's one specific spec to do → `/aida-pickup`.

## Procedure

### 1. Enter solo mode

```bash
aida solo            # marks this session solo: ~/.aida/solo.toml, 24h safety TTL
```

This lights the statusline solo marker so any peer session can see the seat is
taken, and arms the solo posture (drains honor it — keystone parks, safe work
proceeds on the defensible default). Note the exit verb up front: **`aida solo
stop`** when you're done (step 5).

### 2. The loop (with judgment), repeat until the safe backlog is drained

#### a. GROOM — the advisor pass (you make the worth-doing call)

A cross-spec pass over open specs: **worth-doing judgment, dedup, dispose**,
then bless + queue the pickable approved set. The *mechanism* is the existing
verb; the *judgment* is you.

```bash
aida intake --apply        # cold-boot advisor proposes approve/reject/park/queue, then executes
# or, for the guided per-item version with file-overlap heuristics:
#   follow /aida-backlog-groom  (aida backlog list → analyze → groom --batch …)
```

Use your context to override the propose-by-default where you know better:
reject duplicates, park drafts that need a human decision (route them to `aida
questions`, do NOT approve), and queue only what's genuinely ready. Queue
membership IS the advisor sign-off (STORY-546) — only what you queue here is
drain-eligible.

#### b. IMPLEMENT — fan worktree-isolated implementers (PARK keystone)

For each bounded ready spec, spawn **one worktree-isolated implementer
subagent** — each takes ONE spec end-to-end to a PR:

> `Agent(subagent_type: "general-purpose", isolation: "worktree")` — read it
> (`aida show <SPEC> -c`), implement to acceptance, add `// trace:<SPEC>` (plain
> `//`, never `///`), `cargo build` + `cargo test` + `cargo fmt --all -- --check`
> (check the exit code), commit `[AI:claude] type(scope): … (<SPEC>)` + the
> co-author trailer, push, open a PR, `aida queue done <SPEC>`, then reply ONLY
> the PR URL or `BLOCKED: <reason>`.

**PARK keystone / `supervised` specs — never ship them unattended.** Anything
security-, architecture-, or autonomy-machinery-related — `supervised`,
`keystone`, `architecture`, `security`, `blast-radius:high`, `risk:high`, or an
epic type — does **not** get fanned out. Surface it for the human (`aida human`
/ a `PushNotification`) and move on. `aida burndown plan` already excludes the
`supervised` set, so prefer it to enumerate the fan-out-able ready set:

```bash
aida burndown plan --status approved --json   # {ready, awaiting_signoff, parked}
```

Fan out only `ready`. Drop any spec already in flight (open PR / active lease)
and include AT MOST ONE spec per `serialize:<group>` tag per wave — see
`/aida-burndown` for the full fan-out discipline (it is the engine for this
phase).

#### c. INTEGRATE — you are the serial merge authority

One merge driver at a time. For each returned PR: **wait for CI to reach a
TERMINAL state** (`gh pr checks <n>` until conclusive — never treat PENDING as
skip, BUG-541), then if green + mergeable + clean, merge (`--squash` only — NO
`--delete-branch`: the implementer worktree still holds the branch, so the
local delete is refused and an `&&` chain silently drops the pull leg) and run
**`aida pull`** as its own step (the Done→Completed auto-bump rides
`aida pull`, not raw `git pull`; branch deletion happens at worktree pruning).
HOLD any `review:draft-only` / `supervised` PR as a draft for the operator.

Between waves, **rebuild/test the combined main** — each PR's CI ran against the
old base, so two greens can break `main` together (BUG-496):

```bash
git checkout main && git pull --ff-only && cargo build -p aida-cli
```

If it fails, **HALT** — fix-forward if mechanical, else park + alert. Never loop
over a red `main`.

`aida queue integrate --watch` is the headless serial-integrator if you'd rather
let it drive the merges while you stay on groom + judgment; either way, only one
thing drives `main` (the `.aida/drain.lock` enforces this).

**Punt-and-continue (non-negotiable):** a blocker parks **one** spec
(`NeedsAttention` + a note) and the loop rolls on. One spec's failure must
never halt the wave or the loop. A real design fork → file it as an `aida
questions` DecisionRequest (don't guess) and continue.

#### d. Loop

Re-run the groom pass (the ready set shrinks as specs land and may grow as
blockers clear), fan the next wave, integrate, repeat — until the safe ready
set is empty.

### 3. Report + exit

When the safe backlog is drained, summarize on two channels — a
`PushNotification` (completed / parked-with-reason / what's left for the human)
and a mailbox handoff to the advisor seat so the caveats land without the
operator relaying them:

```bash
aida mailbox send --to advisor --intent handoff "Solo loop complete. Completed: <SPEC+PR each>. Parked for human (keystone/decision): <spec+why>. Left for manual merge: <none|#N url>."
```

Then leave the seat:

```bash
aida solo stop
```

## Guardrails (state them, hold them)

- **Never ship keystone unattended.** Security / architecture / the autonomy
  machinery itself is **surfaced for review**, never auto-merged — even a green
  PR. A false-positive park (a safe spec held for review) is cheap; shipping
  keystone unattended is the expensive error this guards against.
- **Destructive ops need sign-off.** Anything hard-to-reverse — branch/worktree
  deletion, a `aida doctor --heal` destructive fix — must be human-gated, not
  run blind. `aida doctor --heal` already routes its destructive tier to
  sign-off in autonomous contexts and lets safe reversible fixes proceed
  (STORY-666); honor the same line — gate by *consequence*, not by category.
- **Combined-main green between batches.** Per-PR CI green ≠ `main` green for a
  parallel wave (BUG-496). Rebuild/test integrated `main` before looping; HALT
  on red.
- **Supervise it first.** The end-to-end composition is newer than its parts —
  run it supervised for a cycle or two before trusting it to run long.

## How this differs (so it's not redundant surface)

This skill is the **only** surface that does **groom → implement → integrate
end-to-end with live judgment**, at the keyboard. Reach elsewhere for a slice of
it:

| Use | When |
|-----|------|
| **`/aida-solo`** (this) | the **WARM**, full-judgment, at-keyboard, end-to-end loop — a session with real context drives groom + integrate |
| `aida solo run` | the **HEADLESS cold-boot engine** — fully hands-off, a fresh `claude -p` per tick; reach for it to leave it running unattended |
| `/aida-drain-queue` | **implement-only** — drain a role's queue, no groom or integrate authority |
| `/aida-burndown` | **fan-only** — fan implementers over a ready set + integrate, no groom pass |
| `/aida-backlog-groom` | **groom-only** — the guided approve→queue prep pass that this skill's step 2a reuses |

## Related

- `docs/solo-mode.md` — the runbook: the loop, the solo posture, the safety
  floor, the honest caveats. **Read it before the first run.**
- `aida solo --help` — the typed command (`run` / `stop` / `status`, `--ttl`).
- `docs/autonomous-drain.md` — the drain user guide.
- `docs/architecture/autonomy-and-escalation.md` — the escalation cascade.

<!-- trace:STORY-668 trace:EPIC-43 | ai:claude -->
