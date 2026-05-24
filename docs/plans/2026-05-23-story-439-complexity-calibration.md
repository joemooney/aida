# STORY-439 — Assistance + complexity calibration layer

Date: 2026-05-24 · Specs: STORY-439 · Status: Plan · Complexity: Medium

## Summary

TASK-340 (WIP on task-340-2) measures ACTUAL autonomy outcomes per drain. STORY-439 adds the PREDICTED side + complexity dimension across three measurement touchpoints (pickup, ship, review) so substrate can report:

1. Are we estimating our own work accurately?
2. Are we shipping faster at the same complexity?
3. Are reviewers seeing what implementers expected?

Reviewer's verdict is most objective (full diff visibility). Substrate-gap signal: a class of work systematically misjudged → memory candidate.

## Approach

Per-spec calibration record at `.aida/complexity-calibration/<SPEC>.yaml` (mirrors STORY-347's `.aida/punts/<id>/calibration.yaml` layout). Accumulates 3 slots over spec's lifecycle. Tag conventions (`complexity:low|med|high`, `estimated-assistance:none|advisor|human`) are source-of-truth. Reviewer verdict-file schema gains 2 fields. New `aida autonomy calibration mismatches` surfaces pickup-vs-reviewer divergences.

## Key decisions (7 total)

- D1: File-per-spec yaml, not JSONL — three writes per spec want upsert semantics; matches STORY-347 shape
- D2: Tags as source of truth + YAML as projection — composes with existing tag tooling
- D3: No interactive prompt at pickup — STORY scope is "human/implementer-supplied" + no auto-estimation; would block headless drains
- D4: Don't introduce `aida autonomy report` here — TASK-340's WIP already defines AutonomyCommand {List, Stats}. This PR adds only Calibration(CalibrationSubcommand) variant; coordinate via rebase
- D5: Punt count from .aida/punts.jsonl filtered by spec — no new instrumentation
- D6: Backwards-compatible verdict schema — `#[serde(default)] Option<String>` on both new fields
- D7: Telemetry kill-switch reuses usage::is_enabled — every write gates on it (punt::append_to_ledger pattern)

## Critical Files

- aida-cli/src/complexity_calibration.rs (NEW — module mirroring calibration.rs/STORY-347 shape)
- aida-cli/src/main.rs (mod decl + handle_autonomy_command dispatch + 3 capture wirings: pickup/ship/review)
- aida-cli/src/cli.rs (--complexity + --assist-est on QueueCommand::Work; --complexity on PrCommand::Ship; new AutonomyCommand + CalibrationSubcommand enums)
- aida-cli/src/reviewer_summary.rs (VerdictFile: 2 new Option<String> fields + format_reviewer_summary lines)
- aida-cli/src/pr_ship.rs (PrShipOptions.complexity + format_dry_run_plan line)
- aida-core/templates/docs/aida-discipline/machinery-glossary.md (complexity + estimated-assistance entries)
- aida-core/templates/skills/{aida-pickup,aida-review,aida-pr}.md (calibration capture instruction)

## Reusable helpers

usage::is_enabled (telemetry kill-switch), calibration.rs (STORY-347 shape this mirrors), punt::read_ledger (ship-side punt count), findings::pr_number_from_tag (canonical tag-prefix scan idiom), reviewer_summary::{parse_verdict_file, VerdictFile} (extend don't fork), pr_ship::{extract_spec_ids_from_text, derive_squash_subject_spec_ids}.

## Risks

1. TASK-340 WIP collision on AutonomyCommand enum — D4 mitigates via additive variant only
2. Existing verdict files lack new fields — Option<String> + serde(default)
3. Pickup capture must not block headless drains — flags opt-in, no prompt code path
4. Spec resolution on aida pr ship — capture against every spec extract_spec_ids_from_text returns
5. complexity: tag duplication on re-pickup — apply_complexity_tag strips existing first
6. Reviewer agreement is implementer-vs-reviewer (not pickup-vs-reviewer); mismatch view independently computes pickup-vs-review from YAML
7. AskUserQuestion constraints in headless reviewer — verdict fields emitted by skill template, not prompted
8. Filesystem race on concurrent slot writes — sequential lifecycle makes unlikely; one-slot-wins is acceptable

## Tests (18 in-module + 4 reviewer + 2 ship + 3 cli)

complexity_calibration.rs: complexity_level_parse_round_trip, assistance_level_parse_round_trip, apply_complexity_tag_replaces_existing, apply_complexity_tag_preserves_unrelated, apply_assistance_tag_replaces_existing, complexity_from_tags_none_when_absent, compute_agreement_matched/under/over (3), pickup_capture_round_trip, ship_upsert_preserves_pickup, review_upsert_preserves_pickup_and_ship, upsert_pickup_no_ops_when_telemetry_disabled, read_all_captures_skips_garbage, mismatches_drops_records_missing_review_half, mismatches_surfaces_low_vs_high_underestimate, mismatches_filters_by_since, punt_count_for_spec_aggregates_only_matching

reviewer_summary.rs: verdict_file_parses_implementation_complexity, verdict_file_back_compat_no_complexity_fields, summary_surfaces_implementation_complexity_and_agreement, summary_omits_complexity_lines_when_unset

pr_ship.rs: dry_run_plan_includes_complexity_capture_when_set, dry_run_plan_omits_complexity_line_when_absent

cli.rs: queue_work_parses_complexity_and_assist_est_flags, pr_ship_parses_complexity_flag, autonomy_calibration_mismatches_parses_since_and_json_flags

## Composes with

STORY-325 (punt ledger — intervention-count source), STORY-347 (calibration ledger pattern), STORY-122 (telemetry opt-out), TASK-340 (autonomy report — coordinates via Decision 4 + extension followup), STORY-451 (effort estimation 4 touchpoints — sibling calibration axis).

## Followups

- TASK: extend aida autonomy report with --by priority,complexity --calibration --since (folds into TASK-340's eventual PR)
- TASK: interactive aida queue work prompt for --complexity / --assist-est (gated off --no-human / non-TTY)
- TASK: 4-week-bucketed trend rendering on aida autonomy calibration mismatches
- TASK: starter memory feedback_complexity_calibration_substrate_gap_signal.md
- TASK: cross-reference calibration view from aida-digest template

(Full plan generated by web /ultraplan 2026-05-23 evening; canonical text in session chat record. PR-270 +1580/-31 is the implementation; CI green after Codex's BUG-371 fix.)
