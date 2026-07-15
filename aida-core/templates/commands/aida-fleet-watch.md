---
description: "Substrate-first fleet monitor — census every agent session (managed or not), classify each, and give ONE recommended next action per non-nominal item. Read + route only; never starts work, takes a lease, or merges. Pair with /loop for a live dashboard."
---
# Watch the Fleet

One continuously-runnable view of every agent session on the machine — managed by
AIDA or hand-started — with a plain-language state and ONE recommended next action
per non-nominal item. **Read + route only:** it may file findings / ack briefs /
escalate into `aida awaiting`, but it NEVER starts work, takes a lease, or merges.

## Instructions

Follow the workflow in `.claude/skills/aida-fleet-watch.md`. Each tick:

1. **Substrate sweep (authoritative):** `aida ps --json` (live/STALE/orphaned),
   `aida awaiting --json` (operator gates), the new `.aida/events.jsonl` lines
   since the last tick (classify like `aida watch` — actionable verbs only), and
   optionally `aida integrate --json` for throughput context.
2. **Terminal census (fallback):** `tmux list-panes -a` (+ `wezterm cli list` if
   a GUI instance is present), subtract panes already backed by a lease, and
   cheaply classify the remainder (`capture-pane -p`) to surface **unmanaged**
   sessions and **wedges** (pid alive + output quiescent). Degrade gracefully —
   with neither tmux nor wezterm, the substrate sweep alone still yields the
   managed-fleet digest.
3. **Diff + classify** each session vs the previous tick:
   completed / progressing / stalled / needs-you / unmanaged / new.
4. **Digest:** plain-language fleet state + ONE recommended next action per
   non-nominal item (name the spec + the concrete verb).
5. **Persist** the last event offset + census snapshot under `.aida/` so the next
   tick can diff.

## Design constraints (honor these, they are the contract)

- **Substrate-first.** Completion is STRUCTURAL ONLY — lease released / spec Done
  / PR merged / drain exited. Never inferred from scrollback text. The census is
  a fallback; when it disagrees with the substrate about a managed session, the
  substrate wins.
- **Decision envelope — route, never dispatch.** Default mode writes NOTHING.
  Routing verbs (file finding / ack brief / escalate) fire only on an explicit
  flag/answer. NEVER start work, take a lease, merge, or send keys into a pane —
  no new-work dispatch path exists (single-drain-lock lesson: no second decider).
  Any nudge/auto-action tier is a separate explicit opt-in per
  `docs/architecture/autonomy-and-escalation.md`.
