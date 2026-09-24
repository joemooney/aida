---
name: well-formed
version: 1
summary: Is this a well-formed requirement? Deterministic EARS lint first, then an agent checklist for what lint cannot decide.
binds_to: groom
deterministic: ears-lint
---
# Gate: well-formed

Tier 1 (deterministic) has already run: the EARS lint findings above are
reproducible and came from no model. Do not re-litigate them; fix or accept them.

Tier 2 (heuristic, rung 4). Answer each item for THIS spec only. A tier-2
verdict is advisory: record it, never let it silently block.

1. One reader, one meaning: could two implementers read the title and
   description and build different things? Name the ambiguous phrase.
2. Observable outcome: is there at least one behaviour someone can watch
   happen (a command's output, a file written, a status change)?
3. Acceptance present: is there an `ACCEPTANCE` / `## Acceptance` section, or
   criteria that play that role? If not, draft two.
4. Why is stated: does the spec say what problem it solves or who asked?
5. Right type: is a bug really a defect, a story really user-visible, a task
   really a chore? Suggest a retype if not.

Verdict line to record: `gate well-formed@v1: pass|warn — <one sentence>`.
