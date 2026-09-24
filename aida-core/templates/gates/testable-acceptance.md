---
name: testable-acceptance
version: 1
summary: Can each acceptance criterion be checked by a test or a command, not by opinion?
binds_to: groom
deterministic: ears-lint:low-testability,missing-behavior,empty-body
---
# Gate: testable-acceptance

Tier 1 (deterministic) has already run over the spec text, filtered to the
testability categories. Its findings are reproducible.

Tier 2 (heuristic, rung 4). For each acceptance criterion:

1. Name the check: a unit test, an integration test, or a shell command whose
   exit code or output decides it. "Reviewer agrees" is not a check.
2. Measurable words only: replace "fast", "clean", "intuitive", "robust" with
   a number, a threshold, or an observable.
3. Negative case: does at least one criterion say what must NOT happen (the
   unchanged path, the refused input)?
4. Scope of the check: does a passing check prove the criterion on the primary
   caller, not just on a helper?

Verdict line to record: `gate testable-acceptance@v1: pass|warn — <one sentence>`.
