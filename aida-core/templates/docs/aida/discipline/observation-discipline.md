# Observation discipline — when to file an observation vs an immediate BUG / TASK

The advisor seat sees a lot. Across a session you spot patterns — a CLI
that prompts twice, a memory file that contradicts code, a friction point
that *might* be a bug but you're not sure yet. The temptation is binary:
file a BUG / TASK immediately, or do nothing and trust your memory to
hold onto it.

Both extremes lose. File too aggressively and the backlog fills with
not-yet-confirmed patterns the team has to triage; trust your memory and
the observation decays with context — by next session you've forgotten
why it mattered.

The middle ground is the **observation**: an advisor-filed finding that
sits in the same triage substrate as drain-filed findings, can be
re-sighted (`aida findings recur`), promoted (`aida findings promote`),
or dismissed (`aida findings dismiss`).

## What an observation is

An `aida findings add --kind observation` entry. It carries:

- `from-advisor:<linked-spec-or-general>` — the source tag that makes it
  a finding and groups it under "From advisor" in the triage view
- `kind:observation` — the category (parallel to implementer findings'
  `kind:bug-spotted`, `kind:deviation`, …)
- `severity:<major|minor|cosmetic>` — optional, sorts the triage view
- `linked:<spec>` — optional, one per additional spec the observation
  references; the first linked spec doubles as the `from-advisor:`
  origin
- `recurrence:N` — implicit `1` on filing; `aida findings recur <ID>`
  increments it

Stored as a draft TASK in the same store as every other finding. Audit
trail lives in the requirement's comments.

## When to file an observation (rather than an immediate BUG / TASK)

File an **observation** when the symptom is real but:

1. You don't yet know if it's a bug, a UX problem, a documentation gap,
   or a misuse — the *kind* is uncertain
2. The pattern has only happened once and you want to see if it recurs
3. The fix-shape is unclear — naming the right TASK requires more
   evidence than one sighting gives
4. The cost of being wrong (filed prematurely → bad spec text →
   implementer goes down a dead-end) is higher than the cost of waiting
   for a second sighting

File a **BUG / TASK directly** (skip the observation step) when:

1. The symptom is reproducible *now* and you can describe the fix in
   one sentence
2. There's a regression suite the bug should land in immediately
3. The work fits in one PR and you can scope it without further evidence

## The recurrence-as-promotion signal

The promotion signal is the **recurrence counter**. Re-sight an existing
observation with `aida findings recur <ID> --note "what you saw"` rather
than filing a new one for the same pattern. The counter survives in the
`recurrence:N` tag; comments record the audit trail.

- **`×1` (just filed)** — keep watching; don't promote yet
- **`×2` (one re-sighting)** — the pattern is real but you may not yet
  know its shape
- **`×3` or more** — the pattern is recurring; promote it
  (`aida findings promote <ID>`) to a TASK / BUG / STORY so the team
  acts on it

`aida findings recur` prints the promote-it nudge automatically once the
counter crosses 3. The threshold isn't load-bearing — promote earlier if
the third sighting clarifies the fix-shape, or wait longer if you still
don't know what to ask for.

## Dismissal

Observations decay. Dismiss (`aida findings dismiss <ID> --reason
"…"`) when:

- The underlying capability landed and the observation no longer applies
- A second look revealed the symptom was misread (operator error,
  config drift, expected behaviour)
- The pattern hasn't recurred in 30+ days — the original sighting may
  have been a one-off

The reason becomes an audit comment so the *why* survives.

## Worked examples

### File an observation

```
aida findings add \
  --note "Recurrence threshold of 3 is hard-coded in the recur handler;
          consider making it configurable via [findings] promote_threshold
          in .aida/config.toml so projects with different signal-to-noise
          tolerances can tune it." \
  --severity minor \
  --linked-specs STORY-467 \
  --tags "aida:findings"
```

### Re-sight when the pattern shows up again

```
aida findings recur TASK-1-086 \
  --note "Third sighting — small project wants threshold=2; AIDA's own
          high-volume tracker wants threshold=5."
```

### Promote when the fix-shape is clear

```
aida findings promote TASK-1-086 \
  --reason "Three independent users hit this — converting to TASK so
            it lands in the next round of config-surface work."
```

### Dismiss when the symptom was misread

```
aida findings dismiss TASK-1-087 \
  --reason "Re-read the docs — the behaviour I flagged is documented in
            session-discipline.md and matches the stated design."
```

## Why this exists

The advisor seat captures friction the rest of the team doesn't see —
*because* they're heads-down implementing while the advisor is the one
spotting cross-cutting patterns. Without a substrate-resident capture
surface, every observation either decays with context or gets prematurely
filed as a BUG/TASK that the implementer can't act on because the fix
shape is still ambiguous. Observations close that gap: substrate-resident
so context decay can't erase them, but distinct from approved work so
the backlog stays clean.

trace:STORY-467 | ai:claude
