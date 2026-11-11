---
name: testable-acceptance
version: 1
summary: Does each acceptance criterion name concrete evidence appropriate to its type?
binds_to: groom
deterministic: ears-lint:low-testability,missing-behavior,empty-body
---
# Gate: testable-acceptance

Tier 1 (deterministic) has already run over the spec text, filtered to the
testability categories. Its findings are reproducible.

Tier 2 (heuristic, rung 4). For each acceptance criterion:

1. Name the evidence type and check. Behavioral criteria require automated
   tests. Configuration validation names the configuration and validation
   command/result or routing analysis. Documentation inspection names the
   document and claims to check. Manual inspection names a repeatable procedure,
   expected observation and limitations. Use `[evidence: behavioral-test]`,
   `[evidence: configuration-validation]`, `[evidence: documentation-inspection]`
   or `[evidence: manual-inspection]` after the criterion label. Missing or
   unknown annotations retain the behavioral test requirement. "Reviewer agrees"
   and the annotation itself are not evidence. Do not relabel behavioral
   requirements to waive tests. <!-- trace:STORY-2702 | ai:codex -->
2. Measurable words only: replace "fast", "clean", "intuitive", "robust" with
   a number, a threshold, or an observable.
3. Negative case: does at least one criterion say what must NOT happen (the
   unchanged path, the refused input)?
4. Scope of the check: does a passing check prove the criterion on the primary
   caller, not just on a helper?

Verdict line to record: `gate testable-acceptance@v1: pass|warn — <one sentence>`.
