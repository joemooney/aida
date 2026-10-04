---
name: single-deliverable
version: 1
summary: Is this spec one deliverable, or several that should be split before anyone starts?
binds_to:
deterministic:
---
# Gate: single-deliverable

No deterministic tier: this gate is heuristic only (rung 4). Advisory.

1. Count the verbs: list each distinct thing the spec asks to build or change.
   More than one independently shippable item is a split candidate.
2. One PR test: could this land as one reviewable PR? If the honest answer is
   "a series", name the slices.
3. Shared acceptance: does every acceptance criterion serve the same
   deliverable? A criterion that only one slice needs belongs to that slice.
4. If splitting, propose child titles and run `aida decompose`-style slicing
   (`/aida-decompose`) rather than filing them by hand.

Verdict line to record: `gate single-deliverable@v1: pass|warn — <one sentence>`.
