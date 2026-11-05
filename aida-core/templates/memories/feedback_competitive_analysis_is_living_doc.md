---
name: competitive-analysis-is-living-doc
description: A competitive analysis is a living document with a refresh cadence and dated snapshots, not a one-shot exercise.
propagation: scaffolding-pack
metadata:
  type: feedback
---

Some artifacts go stale fast — a competitive analysis especially. The tooling landscape moves monthly. A one-shot analysis is obsolete within a few months. Treat it as a living document with a refresh cadence, not a single output.

**Why:** Without a durable home and a cadence, the next analysis re-does the work from scratch and loses every incremental observation made in between.

**How to apply:**

1. Each analysis session writes a **dated snapshot** (`docs/competitive-analysis/YYYY-MM-DD-snapshot.md`) — don't overwrite previous ones; retain them as a historical record.
2. Maintain category summaries that update incrementally as observations accumulate.
3. Refresh on a cadence (e.g. quarterly) OR on signals — a competitor reaches a milestone, a foundational primitive ships, a standard emerges.
4. Keep a signals-to-watch list naming specific projects and triggers.
5. Keep a durable positioning statement — updated only when the niche actually shifts.

**Pattern to avoid:** produce a great analysis → consider it done → months later the market has moved → re-do from scratch.

**Better pattern:** produce analysis → write a dated snapshot + update summaries → schedule the next refresh → later, the refresh adds a delta.

The same discipline applies to any fast-staling artifact (architecture overviews, dependency audits).

Composes with [[self-test-via-dogfood-merge]] (durable capture of evidence) and [[advisor-role-responsibilities]] (strategic gap detection produces these artifacts).
