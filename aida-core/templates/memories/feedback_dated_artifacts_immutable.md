---
name: dated-artifacts-immutable
description: When refactoring across the codebase, dated historical artifacts (SPIKE outputs, PROMPT_HISTORY, dated plans, dated competitive-analysis snapshots, commit messages, spec comments) stay frozen at the date in their filename — they record what we knew at time T.
propagation: scaffolding-pack
metadata:
  type: feedback
---

When a convention evolves — a glyph swap, a vocabulary update, a palette unification, a renamed term — a cross-cutting refactor should update *living* guidance and code to current truth, and **leave dated historical artifacts alone**. Those files are records of "what we knew at time T"; rewriting them erodes the trail of how the team's understanding evolved.

**Why:** The value of a dated SPIKE output, a session log, or a competitive-analysis snapshot is precisely that it captures a moment. Retroactively editing it makes the past look like the present — which destroys evidence of the path taken and makes it impossible to reason later about why a decision was made when only the "current" version exists. Commit messages and spec comments share the same property: they are chronological record, not living guidance.

**How to apply:** When doing a cross-cutting refactor (glyph swap, vocabulary normalization, rename, palette unification), classify each affected file:

| Artifact kind | Retroactive edit? |
|---|---|
| Living guidance (CLAUDE.md, skill templates, `docs/aida/discipline/`, README) | **YES** — update to current truth |
| Code (Rust source, configs, templates that compile/scaffold) | **YES** — update to current truth |
| Plan files in `docs/plans/` (active or recent) | YES if still load-bearing; NO once the work shipped and the plan is historical |
| Dated SPIKE outputs (`docs/spikes/YYYY-MM-DD-*.md`) | **NO** — dated record of empirical findings |
| Dated session logs (`PROMPT_HISTORY.md` entries) | **NO** — chronological record |
| Dated competitive-analysis snapshots (`docs/competitive-analysis/YYYY-MM-DD-*.md`) | **NO** — snapshot at time T |
| Spec descriptions / acceptance bullets | YES if work hasn't started; OR file follow-up + comment per [[refinements-must-be-acceptance-criteria]] |
| Spec comments | **NO** — chronological record |
| Git commit messages | **NO** — immutable record |

The discriminator: *is the filename dated, or does the artifact's value depend on being a point-in-time record?* If yes, freeze it. If it is living guidance someone reads to learn current truth, update it.

**Concrete instance (2026-05-17 BUG-116):** BUG-116 propagated the `▶ ⏵ 🚪` → `▶ ⇒ ⏸` glyph swap across skill templates. The implementer correctly left `docs/spikes/2026-05-16-claude-headless.md:87` untouched, noting: *"dated historical observation record, not living guidance."* That phrase is the convention worth codifying — and is the origin of this memory.

**Out of scope:** A lint check for retroactive edits to dated files is over-engineered; filename-date convention plus this discipline is sufficient. Similarly, no per-file "frozen" metadata field is needed.

Composes with [[classify-memory-propagation]] (this discipline itself propagates via the scaffolding pack), [[precise-lifecycle-vocabulary]] (same family — precision about state-at-time-T), and [[refinements-must-be-acceptance-criteria]] (sibling — refinements to *active* specs go in acceptance, not comments; but historical comments stay frozen).
