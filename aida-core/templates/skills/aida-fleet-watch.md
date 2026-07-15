---
name: aida-fleet-watch
description: Substrate-first fleet monitor — one continuously-runnable census of every agent session (managed or not), with plain-language state and ONE recommended next action per item. Read + route only: it may file findings / ack briefs / escalate into `aida awaiting`, but NEVER starts work, takes a lease, or merges. Composes existing read-only CLI surfaces (`aida ps`, `aida awaiting`, `aida watch`, `aida integrate`) plus a tmux/wezterm terminal census fallback. Pair with `/loop` for a live dashboard.
allowed-tools:
  - Bash
  - Read
  - Grep
  - Glob
---

# AIDA Fleet-Watch Skill

## Purpose

Give the operator **one continuously-runnable view of the whole fleet** — every
agent session on the machine, managed by AIDA or hand-started — with a
plain-language state per session and exactly **one recommended next action** for
each non-nominal item. This is the SPIKE-77 driving use case: a monitor you can
leave running (one-shot, or under `/loop`) that tells you *what is genuinely
running, what is stuck, and what needs you* without you having to poll six
different commands.

Skill-only v1: **no new Rust**. Every input is an existing read-only CLI surface
or a cheap terminal scrape. If this pass gets used weekly+ or its shell grows
past ~a screen of bash, that is the revisit trigger to promote the mechanics
into a native `aida fleet` subcommand (STORY-766).

## The two design constraints (not implementation details)

These are the contract of the skill. State them; honor them.

### 1. Substrate-first

The **authoritative** picture of the fleet is AIDA's own substrate — leases,
spec status, the event feed, the coordination inbox. The terminal census
(tmux / wezterm) is a **fallback**, used only to (a) find *unmanaged* sessions
AIDA never launched and (b) detect a **wedge** in a managed session (pid alive
but output quiescent) that pid-liveness alone cannot see.

**Completion detection is STRUCTURAL ONLY** — a session is "done" because its
lease released, the spec went Done, its PR merged, or the drain exited — **never
because scrollback text looks like it finished.** Output phrasing varies
run-to-run; structural signals do not. Never classify from captured pane text
beyond the coarse "agent-shaped / waiting-at-prompt / quiescent" buckets.

### 2. Decision envelope — route, never dispatch

Default mode writes **nothing**. This pass may, and only via an explicit
flag/answer, **route**: file a finding, ack a brief, escalate an item into
`aida awaiting`. It may **NOT** start work, take a lease, merge, or send keys /
text into a pane. There is **no new-work dispatch path at all** — by design.

This is the single-drain-lock lesson (BUG-538): a monitor that could start work
would be a second decider racing the orchestrator. Any nudge (send-keys /
send-text) or auto-action beyond read+route is a separate, explicit opt-in tier,
aligned with `docs/architecture/autonomy-and-escalation.md` — out of scope for
default fleet-watch.

## Instructions

Each tick (one-shot, or `/loop`-driven):

### 1. Substrate sweep (authoritative)

- `aida ps --json` — every active session/agent: spec, role, worktree, pid,
  started, elapsed, and the live / dormant / STALE verdict, plus **orphaned**
  In-Progress specs (a status flag with no live lease behind it).
- `aida awaiting --json` — the operator gates: mergeable PRs, unacked briefs,
  findings, reviewer verdicts, NeedsAttention escalations, unread mail.
- Event tail since the **last tick's offset**: read the new lines of
  `.aida/events.jsonl` and classify them the same cheap way `aida watch` does —
  surface only actionable verbs (CI terminal, PR shipped/merged, punt, shelve,
  escalation, drain finished); ignore benign phase churn.
- `aida integrate --json` (optional) — throughput context: time since last merge
  to the default branch and whether main is idle.

### 2. Terminal census (fallback — the only legitimate scrape)

Run only to find what the substrate cannot see (unmanaged sessions + wedges):

- `tmux list-panes -a -F '#{pane_id} #{pane_pid} #{pane_current_command} #{window_name}'`
  — tmux is the operator daily driver (2026-07-11) and the TASK-1120 pane host.
- `wezterm cli list` — **only if** a wezterm GUI instance is present; skip
  silently if the binary or a running instance is absent.
- **Subtract** panes matching known lease pids / pane-ids from step 1 — those are
  already accounted for by the substrate.
- For the **remainder**, `tmux capture-pane -p -t <id>` (or wezterm
  `get-text`) and classify cheaply: agent-shaped (a coding-agent prompt) /
  waiting-at-prompt / quiescent (>N min no change). Also use this to flag a
  **wedge** on a *managed* pane (pid alive + output quiescent).

Degrade gracefully: wezterm absent → tmux-only census; tmux absent →
wezterm-only; **neither present → the substrate sweep alone still yields the
managed-fleet digest.**

### 3. Diff + classify vs previous tick

Compare each session against the persisted previous-tick snapshot and bucket it:

| Class          | Meaning                                                        |
| -------------- | ------------------------------------------------------------- |
| `completed`    | lease released / spec Done / PR merged / drain exited (structural) |
| `progressing`  | live, elapsed advancing, recent actionable events             |
| `stalled`      | lease live but STALE / wedged (pid alive, output quiescent)   |
| `needs-you`    | an `aida awaiting` gate points at it (verdict / escalation / mergeable PR) |
| `unmanaged`    | a terminal session AIDA never launched (no lease)             |
| `new`          | first seen this tick                                          |

### 4. Digest

Plain-language fleet state, then **one recommended next action per non-nominal
item** (skip the nominal `progressing` rows or list them compactly). Name the
spec and the concrete verb, e.g.:

- `STORY-812 — STALE lease 41m (drain wedged). → aida why STORY-812; if truly dead: aida session leases, release + re-queue.`
- `Unmanaged claude in tmux window "hotfix". → not tracked by AIDA; if it's real work, file a spec + take a lease so it's visible.`
- `BUG-901 — reviewer RequestChanges awaiting you. → aida review BUG-901.`

### 5. Persist tick state

Write the last event offset and the census snapshot under `.aida/` runtime
state (e.g. `.aida/fleet-watch/state.json`) so the **next** invocation can diff.
This is gitignored per-clone runtime state — `.aida/*` is deny-by-default; do not
track it.

### 6. Route (only on explicit flag / answer)

Default is read + report. If the operator asks (or an explicit routing flag is
set), you MAY: file a finding (`aida findings add …`), ack a brief
(`aida brief ack …`), or escalate an item so it lands in `aida awaiting`. You
may **NOT** start work, take a lease, merge, or send keys into a pane.

## Guardrails

- **Zero writes in default mode.** The digest is read-only. Routing verbs fire
  only on an explicit flag/answer; no new-work dispatch path exists.
- **Structural completion only.** Never call a session done from scrollback
  text — only from a released lease / Done spec / merged PR / drain exit.
- **Never a second decider.** No `queue work`, no `pr merge`, no `send-keys`.
  A nudge tier is a separate explicit opt-in, not this pass.
- **Census is fallback, not source of truth.** When substrate and scrape
  disagree about a managed session, the substrate wins — the scrape only *adds*
  unmanaged sessions and wedge signals.

## Delineation

- **`/aida-fleet-watch`** — *is anything running / stuck across the whole
  machine, and what's the one next action per item?* Read + route, continuous.
- **`aida ps`** — the raw running-work table this pass builds on (one command,
  no digest, no census, no recommendations).
- **`/aida-human-audit`** — reconciles the *coordination inbox* so its report is
  true from the graph (may edit spec state). Fleet-watch **observes**; it does
  not fix state.
- **`/aida-integrate`** / **`/aida-solo`** — the *acting* seats (merge, drain).
  Fleet-watch watches them run; it never takes their actions.

Related: **SPIKE-77** (parent — cmux UX signals), the driving research doc
`docs/research/2026-07-07-spike-77-cmux-ux-signals-fleet-watch.md`, and the
autonomy/escalation contract `docs/architecture/autonomy-and-escalation.md`
(where any future nudge/auto-action tier is specified).
