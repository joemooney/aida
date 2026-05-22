---
name: feedback_sketch_first_pays_for_itself
description: "When a sibling agent's architecture-class work goes through sketch-first sign-off (master review BEFORE implementation), the sketch surfaces course-corrections that would otherwise become post-merge cleanup PRs. The governance friction is empirically cheaper than the alternative."
type: feedback
propagation: scaffolding-pack
originSessionId: 1bf450af-f9e9-49d0-a098-79696b14cefc
---
The architecture-sign-off discipline from [[feedback_one_master_advisor_until_subsystems]] feels like friction at the moment of imposition — the implementer has to write a sketch, wait for master, wait again on revise/decline. The natural objection is "isn't this slowing things down?"

**Empirical answer from BUG-328's sketch:** No. The sketch caught three substantive issues that would have been more expensive to address post-merge:

1. **Acceptance-criteria ambiguity.** BUG-328's filed acceptance said "any non-terminal status (Draft, Approved, Planned, In-Progress, Done) is eligible." Codex's sketch flagged Draft as a real decision point: a Draft spec with a commit reference is ambiguous (exploratory? skipped-approval?). Codex recommended excluding Draft; master agreed. Without sketch-first, the implementer would have implemented the literal acceptance ("any non-terminal") and we'd have had to back out Draft eligibility in a follow-up PR after observing weird auto-completions.

2. **Adjacent-code awareness.** Codex's sketch identified an existing review-story exception path (`collect_stale_review_story_flips`) that already promotes Approved/InProgress for PR-number-matched review stories. Without that surfacing, the implementer might have generalized the direct scanner in a way that disturbed the review-story logic — silent regression at best, broken review-story handling at worst.

3. **Code-path coverage.** Two parallel scanner paths (`auto_bump_done_to_completed` for pull-time + `handle_db_reconcile_status` for manual replay) both needed the change. The sketch identified this. A less-thorough implementer might have caught only one and shipped half a fix.

Three substantive course-corrections that happened in a sketch comment instead of in a post-merge cleanup PR. **The sketch-first overhead (~10-20 minutes of write + read + agree) saves the much-larger overhead of revise-revise-revise cycles after implementation.**

**Why sketch-first works particularly well for sibling agents:**

- A sibling agent (Codex, Antigravity, future agents) has a slice of context, not the full project history. The master holds the full context.
- The sketch is the explicit join-point where the agent's bounded analysis meets the master's broader awareness.
- Without it, the agent's bounded analysis becomes implementation, which then has to be reconciled with broader awareness at review time — expensive.
- With it, the join happens before implementation; the implementation reflects the joined understanding from the start.

**When to demand sketch-first:**

- File format / on-disk schema changes
- MCP tool contract changes
- Orchestrator behavior changes
- EPIC-shaped work
- Cross-cutting conventions (commit format, trace format, role taxonomy)
- Memory pack / discipline doc edits
- Any code that affects how OTHER agents/subsystems interact

**When NOT to:**

- Bug fixes with clear-cut localization and acceptance
- Test infrastructure improvements
- Documentation contributions to non-architecture files
- Refactors that don't change observable behavior
- Anything where the spec body unambiguously dictates the implementation shape

**How to invite sketch effectively:**

In the brief to the sibling agent, explicitly name:
- That the work is architecture-class
- That sketch-first is required
- What the sketch should cover (locations, proposed change, edge cases, tests, open questions)
- The format (comment on the spec, ~10-20 lines)
- The expected master turnaround (approve/revise/decline within N hours)

Codex's BUG-328 sketch on 2026-05-22 followed this exact pattern and produced the validation evidence above.

## Composes with

- [[feedback_one_master_advisor_until_subsystems]] — the governance principle this discipline operationalizes
- [[feedback_sibling_agents_stop_and_flag]] — the worktree-discipline counterpart for the implementation phase
- [[feedback_question_existing_form_not_just_existence]] — Codex's sketch did exactly this (identified the existing review-story exception that contradicted the naive generalization)
