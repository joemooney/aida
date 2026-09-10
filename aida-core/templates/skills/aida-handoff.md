---
name: aida-handoff
description: Capture conversation residue, write a durable session handoff brief, and recommend whether to start fresh or compact.
allowed-tools:
  - Bash
  - Read
  - Grep
  - Edit
  - Write
---

# AIDA Handoff Skill

## Purpose

Prepare this AIDA session to survive a context boundary. The durable substrate
should carry the work; this skill closes gaps that live only in conversation,
writes a pickup brief for the next session, then recommends whether a fresh
session is safe or compaction is required.

## Workflow

### Step 1: Capture Residue

Run the `/aida-capture` sweep mentally, using the same rules:

- unfiled observations, bugs, or ideas
- decisions made in conversation but not reflected in acceptance criteria,
  comments, docs, or code
- promised follow-ups that are not represented by a queued spec, finding, or
  brief

Under `AIDA_HEADLESS=1`, never ask the user. File clear conversation flags as
draft requirements or findings for later triage; skip ambiguous residue rather
than blocking.

### Step 2: Inspect Session Pins

```bash
aida session handoff --check
aida drain status 2>/dev/null || true
aida session leases 2>/dev/null || true
aida brief list --for-agent "${AIDA_AGENT_TYPE:-codex}" 2>/dev/null || true
```

The CLI probe sees live drain and lease pins. It cannot see conversation-only
residue; Step 1 is the skill half.

### Step 3: Write The Handoff

Create a concise durable handoff note in the substrate. Prefer an agent brief
for the likely next agent; also append the project session log if the project
still uses one.

Minimum content:

- active role and current spec or queue focus
- running drains, live leases, watchers, and background processes
- open PRs or reviews awaiting action
- next 3 actions in order
- session-bound state that will not survive a fresh start, named explicitly
- residue captured in Step 1 and where it was filed

Use AIDA commands where possible:

```bash
aida brief codex <SPEC-ID> --note -
```

Session narrative lives in the substrate, not a log file: the brief above
plus the specs and PR trail are the record (`aida digest` renders the
narrative on demand). Never write a session-log markdown file.

### Step 4: Recommend

Print exactly one decision line:

- `Recommendation: START FRESH` when the capture sweep is complete and
  `aida session handoff --check` reports no live drain or current-session lease
  pins. A fresh session should rebuild from `aida status`, queue state, briefs,
  leases, and the handoff note.
- `Recommendation: COMPACT` when live session-bound state must survive. List
  each pin: live drain, live lease, watcher, monitor, background process, or
  unresolved conversation residue.

Do not recommend compaction merely because context is large. In AIDA work,
fresh is preferred once the substrate boundary is clean.

