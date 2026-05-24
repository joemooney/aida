# STORY-442 — Lifecycle short-circuit tags

Date: 2026-05-24 · Specs: STORY-442 · Status: Plan · Complexity: ~180 prod LOC + ~220 test LOC, 1 commit, risk low

## Summary

aida queue work --auto-complete --no-human=both runs implementer → CI → reviewer → merge → pull → build. For typo fixes the work itself is 30s but lifecycle adds ~10min CI+reviewer tax. STORY-442 adds opt-in per-spec lifecycle tags (`lifecycle:no-ci-wait`, `no-review`, `no-build`, `trivial` shorthand) for 5-10x faster drains on mechanical batches. Phases 4 (merge) + 5 (pull/auto-bump) are unconditional — substrate-integrity guarantee.

## Key decisions (8 total)

- D1: Resolve LifecycleSkip at run_auto_complete (not inside orchestrate) — keeps orchestrate pure (no Storage dep)
- D2: Pass LifecycleSkip both to orchestrate (P3/P6 skip) AND RealPhaseDriver (P2 wait skip) — different responsibilities, live where they execute
- D3: Tags read from dispatched spec only — not tag-mates/parents/BUG-245-credited spec. Predictable semantics.
- D4: Phase 4 + Phase 5 UNCONDITIONAL — no lifecycle:no-merge or no-pull. Substrate integrity.
- D5: lifecycle:trivial = pure shorthand (ORs all three flags in)
- D6: finish_ci keeps calling aida session end --skip-ci — auto-queue review item is harmless cruft when lifecycle:no-review; documented as Followup cleanup
- D7: Surface lifecycle:* in format_tag_chip (extend existing batch:* hoist precedent) — visible-at-a-glance in queue list
- D8: NO new CLI flag — tag IS the API. Minimum surface. aida edit --add-tag lifecycle:trivial.

## Critical Files

- aida-cli/src/auto_complete.rs (LifecycleSkip struct + from_tags + banner_summary + orchestrate signature widen + phase 3/6 skip-gates)
- aida-cli/src/main.rs (resolve_lifecycle_skip helper + RealPhaseDriver field + finish_ci skip-wait + format_tag_chip lifecycle hoist)
- aida-core/templates/docs/aida-discipline/machinery-glossary.md (lifecycle:* term + 4 tags + risk model + unconditional merge+pull guarantee)
- aida-core/templates/docs/aida-discipline/lifecycle-vocabulary.md (one-sentence cross-ref)
- CLAUDE.md (3-line bullet under AIDA-developer workflow)

## Reusable helpers

tag_matches_exact (case-insensitive tag predicate), format_tag_chip (existing batch:* hoist — extend for lifecycle:*), spec_matches (SPEC-ID/UUID/short-id resolver), ensure_queued_for_implementer (template for store-load + req-find lookup shape), MockPhaseDriver (existing test harness — no new plumbing), AutoCompleteVariant::SkipBuild precedent (the "stop early at phase N" pattern; skip_build is same shape).

## Risks (5)

1. Orphaned Review PR-N queue item when lifecycle:no-review — harmless cruft, followup cleanup verb
2. Skip CI-wait → merge can race CI red — explicit risk model; banner makes visible per-run; document in discipline pack
3. format_tag_chip MAX_PLAIN truncation hiding lifecycle tag — hoist into separate bucket BEFORE MAX_PLAIN count (same batch:* pattern)
4. Tag typo silently disables short-circuit — out of scope v1; followup TASK for aida edit warn-on-unknown lifecycle:* prefix
5. LifecycleSkip widens orchestrate signature; ~40 test calls update — mechanical with cargo check; could add orchestrate_default wrapper if churn warrants

## Tests (12 named)

In auto_complete.rs:
- lifecycle_skip_from_tags_parses_each_individual_tag
- lifecycle_skip_trivial_implies_all_three
- lifecycle_skip_composes_individual_tags_with_trivial
- lifecycle_skip_is_case_insensitive
- lifecycle_skip_unknown_lifecycle_tag_is_ignored
- lifecycle_skip_banner_summary_lists_skipped_phases
- orchestrate_no_review_skips_phase_3_and_runs_merge_pull_build
- orchestrate_no_build_skips_phase_6_only
- orchestrate_trivial_skips_3_and_6_keeps_merge_and_pull
- orchestrate_no_review_unreachable_reviewer_escalation_does_not_fire
- orchestrate_lifecycle_none_runs_unchanged_full_pipeline (regression guard)

In main.rs format_tag_chip block:
- format_tag_chip_hoists_lifecycle_alongside_batch
- format_tag_chip_lifecycle_tags_always_shown_even_with_many_plain

## Verification

5-step bash smoke (cargo test, cargo build + dev activate, positive tag/show/list, negative remove-tag, kickoff-banner verification via aida queue work --json on a scratch spec).

## Followups (4)

- lifecycle:no-review auto-remove orphaned Review PR-N queue item after merge
- aida edit warn on unrecognized lifecycle:* tag spellings (typo guard)
- auto-complete telemetry record LifecycleSkip in JSONL event for retro analysis
- aida list --tag-prefix lifecycle: column for fast-track audit view

## Composes with

STORY-246 (--auto-complete orchestrator — built on), TASK-285 (--batch --auto-complete), TASK-351 (aida edit --add-tag / --remove-tag), TASK-238 (format_tag_chip batch:* hoist precedent), STORY-263/276 (--no-human — amplifies unattended drain throughput), STORY-306 (advisor tier — lifecycle:no-review short-circuits the reviewer escalation seat; operator owns the call), STORY-441 (archive timing unchanged), STORY-439 (calibration — future signal use), TASK-503 (pre-commit fmt hook — fmt-cleanup specs are canonical use case).

(Full plan generated by web /ultraplan 2026-05-23 evening; canonical text in session chat record. No PR yet — STORY-442 awaiting implementation pickup.)
