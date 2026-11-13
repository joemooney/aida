// trace:TASK-3428 | ai:codex

// ============================================================================
// TASK-113 — `aida ultraplan <SPEC>`. Assemble a rich, structured planning
// prompt from a spec's full context (description, acceptance, related specs,
// the AIDA plan template, trace-graph reusable helpers) so planners anchor on
// concrete requirements instead of a terse user prompt.
// trace:TASK-113 | ai:claude
// ============================================================================

/// The AIDA plan-template section list, inlined into the planning prompt so
/// the returned plan already matches `docs/plans/_TEMPLATE.md` (TASK-92) and
/// `aida plan verify` (TASK-93) passes on the saved plan.
pub(crate) const ULTRAPLAN_STRUCTURE: &str = "\
Produce the plan in AIDA's structured format — a header line with Date / \
Specs / Status / Complexity, then these sections in order:

1. **Approach** — one-paragraph executive summary, plus an ASCII diagram \
where a state machine or control flow would compress prose.
2. **Decisions** — each significant choice resolved, with rationale (not an \
open-questions list).
3. **Files (in build-order)** — symbol-anchored edits per file, ordered so \
each commit builds clean.
4. **Critical Files** — flat list of every must-touch path.
5. **Reusable helpers** — existing helpers to call rather than re-invent \
(seeded below).
6. **Risks + gotchas** — numbered, each with a mitigation.
7. **Tests** — named test functions, not \"add tests\".
8. **Verification** — an executable bash smoke test (positive + negative).
9. **Followups** — out-of-scope items to file as TASKs at completion time.
10. **Related** — builds-on / blocks / see-also links.";
