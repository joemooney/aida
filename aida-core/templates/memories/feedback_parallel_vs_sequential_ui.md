---
name: parallel-vs-sequential-ui
description: Next-steps UI splits into parallel choices (pick ONE — table) vs sequential steps (do ALL — numbered list).
propagation: scaffolding-pack
metadata:
  type: feedback
---

"Next steps" / "what to do next" UI surfaces split into two problem shapes that need different formats. Conflating them produces self-contradictory specs.

| Shape | The user… | Right format |
|-------|-----------|--------------|
| Parallel choices | picks ONE of N complete next-actions | a Path / What / Why **table** |
| Sequential steps | does ALL of them, in order | a **numbered list** with flow arrows |

**Why:** A "next steps" prompt *feels* like one pattern, so it is easy to cross-reference one spec's format from another whose shape is actually different — and the contradiction only surfaces at implementation time.

**How to apply:** Ask: *"if the user does nothing, does the workflow still progress?"*

- Yes (passive flow) → sequential steps; numbered list with flow arrows.
- No (the user must choose) → parallel choices; Path / What / Why table.

Then pick the matching format, and don't cross-reference one spec's format from another unless the shapes genuinely match.

Composes with [[run-help-before-suggesting-flags]] and [[precise-lifecycle-vocabulary]] — same family: verify the actual shape before specifying its UX.
