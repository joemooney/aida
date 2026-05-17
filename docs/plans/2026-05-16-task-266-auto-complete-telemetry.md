# `--auto-complete` failure telemetry + auto-draft BUG (recurse-fix dogfood)

- **Date:** 2026-05-16
- **Specs:** TASK-266
- **Status:** Complete
- **Complexity:** Medium

## Approach

Make `--auto-complete` failures *systematically captured* so the orchestrator
can evolve through dogfooded use — every failure surfaces a gap, the gap
becomes a BUG, the BUG is fixed (when possible) by running `--auto-complete`
on itself.

```
   aida queue work X --auto-complete
            │
   orchestrate() ── runs phases, now also returns OrchestrationResult
            │        (exit code + failed_phase + failure + phase_durations)
            ▼
   record_auto_complete_run()          [main.rs, side-effecting]
            │
      ┌─────┴─────────────────────┐
      │                           │
  append one JSONL line       on failure: auto-draft a Draft BUG
  to ~/.aida/                 (parent EPIC-23, NOT queued) — unless an
  auto-complete.jsonl         identical recent failure already has one
      │
      ▼
   aida usage --auto-complete            → overview: success rate + recent fails
   aida usage --auto-complete --failures → full failure list + drafted-BUG status
   aida usage --auto-complete --pattern  → per-phase failure histogram
```

`auto_complete.rs` stays pure (trait-driven, mock-tested): `orchestrate` only
*computes* the result. All file I/O (JSONL write, `aida add` subprocess) lives
in `main.rs` so it never compromises the orchestrator's testability.

## Decisions

- **`orchestrate` returns `OrchestrationResult`, not bare `i32`.** The exit
  code is one field; `failed_phase`, `failure`, per-phase `phase_durations`,
  and `total_ms` are the telemetry the caller logs. Keeps the pure core pure.
- **New module `auto_complete_telemetry.rs`**, not an extension of `usage.rs` —
  separate log file, separate event shape. It *reuses* `usage::is_enabled` for
  the opt-out so there is a single privacy switch.
- **BUG drafted via the `aida add` subprocess**, matching
  `ensure_queued_for_implementer`. `--description-stdin` carries the multi-line
  body cleanly; `NO_COLOR=1` on the child keeps the parsed `ID:` line plain.
  Best-effort: a spawn failure just leaves `drafted_bug = None` — the JSONL
  line still captures the failure.
- **Parent EPIC-23, with a parent-less fallback.** The acceptance pins
  EPIC-23; the fallback keeps the auto-draft working in any other project.
- **Dedup on (spec, phase, kind) within 24h.** Re-running a still-broken
  `--auto-complete` reuses the existing Draft BUG instead of spamming new
  ones. The dedup key is the telemetry log itself — no store scan.
- **Draft, never queued.** The user triages and promotes — avoids backlog
  spam from environment/user errors (per the spec's "out of scope").

## Files (build order)

1. `aida-cli/src/auto_complete_telemetry.rs` *(new)* — `AutoCompleteEvent`,
   `PhaseDuration`, `log_path`/`append_event`/`read_events`, `summarize`,
   `failure_histogram`, `Summary`.
2. `aida-cli/src/auto_complete.rs` — `OrchestrationResult`; `orchestrate`
   returns it + tracks per-phase durations; `Phase::from_index`,
   `Phase::slug`/`FailureKind::slug` made `pub(crate)`, `AutoCompleteVariant::slug`.
3. `aida-cli/src/cli.rs` — `Usage` gets `--auto-complete` / `--failures` /
   `--pattern` flags (trace markers as plain `//` — TASK-268).
4. `aida-cli/src/main.rs` — `mod auto_complete_telemetry`;
   `record_auto_complete_run` / `existing_failure_bug` /
   `draft_auto_complete_failure_bug` / `add_draft_bug` / `parse_added_spec_id`;
   `handle_usage_command` gains the auto-complete dispatch;
   `handle_auto_complete_usage` / `render_auto_complete_failures` /
   `render_auto_complete_pattern` / `bug_status`.

## Critical Files

- `aida-cli/src/auto_complete.rs` — `fn orchestrate` (now returns
  `OrchestrationResult`); `struct OrchestrationResult`.
- `aida-cli/src/main.rs` — `fn handle_auto_complete` (calls
  `record_auto_complete_run`); `fn handle_usage_command` (auto-complete
  dispatch); the `Command::Usage` match arms (×2 — legacy + git-canonical).
- `aida-cli/src/auto_complete_telemetry.rs` — the event shape + log I/O.

## Reusable helpers

- `usage::is_enabled` — the shared telemetry opt-out (env + config.toml).
- `build_sha_short` — binary SHA for the event's `binary_sha`.
- `resolve_aida_exe` — BUG-217-safe binary path for the `aida add` subprocess.
- `parse_days_arg` — the `--since Nd` window parser, shared with `aida usage`.

## Risks + gotchas

- **`trace:` leak into `--help`.** clap `///` doc comments are help text; the
  three new flags keep the `trace:` marker as a plain `//` comment (TASK-268).
- **Subprocess `aida add` can fail** (spawn ENOENT on a `Spawn`-kind failure).
  Handled: `draft_auto_complete_failure_bug` returns `None`, JSONL still written.
- **Future-dated / unparseable timestamps** in the window filter are kept,
  not dropped — telemetry should never silently lose a row.

## Tests (named)

- `auto_complete_telemetry`: `event_round_trips_through_json`,
  `success_event_omits_failure_fields_from_json`, `summarize_tallies_*`,
  `failure_histogram_ranks_phases_by_count`,
  `failure_histogram_breaks_count_ties_by_phase_index`.
- `auto_complete`: `result_success_records_every_phase_duration_in_order`,
  `result_variant_caps_phase_durations_at_last_phase`,
  `result_failure_carries_failed_phase_and_reason`,
  `result_rejected_verdict_records_reviewer_as_failed_phase`,
  `phase_from_index_round_trips`, `variant_slug_matches_parse_spelling`.
- `task_266_tests`: `parses_spec_id_from_aida_add_output` + the None cases.

## Verification

```bash
cargo build -p aida-cli
cargo test -p aida-cli auto_complete
cargo test -p aida-cli task_266
cargo fmt --all -- --check
aida usage --auto-complete --help | grep -c trace   # → 0 (no SPEC-ID leak)
aida usage --auto-complete                          # overview view
aida usage --auto-complete --failures               # full failure list
aida usage --auto-complete --pattern                # per-phase histogram
```

## Followups

- The pre-existing STORY-122 `Usage` fields (`--since`, `--unused`, `--errors`,
  `--json`, `--limit`) still leak `trace:STORY-122` into `--help` — a TASK-268
  cleanup left out of this diff to keep it scoped to TASK-266.
- Auto-fixup loop: on a recurring drafted BUG, offer to run
  `aida queue work <BUG> --auto-complete` directly (the recurse-fix loop).
- `aida usage --auto-complete` could show median phase duration to flag slow
  phases, not just failing ones.

## Related

- STORY-246 (`--auto-complete` orchestrator — this is its observability layer).
- BUG-218 (`FailureKind` — telemetry records the kind slug).
- STORY-122 / `usage.rs` (the JSONL telemetry pattern this extends).
- BUG-114 (first dogfood failure — the kind of entry this would have caught).
- EPIC-23 (Session orchestration & autonomy — parent; drafted BUGs file here).
