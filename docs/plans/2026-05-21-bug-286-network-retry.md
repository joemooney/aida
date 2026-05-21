# Plan: BUG-286 — orchestrator-side retry for transient gh/git errors

Date: 2026-05-21
Specs: BUG-286
Status: Completed
Complexity: ~330 prod LOC, ~280 test LOC, 1 commit, risk low

## Approach

The orchestrator's phase-4 `gh pr merge` (and phase-3 reconcile / shipped-spec
read paths) used to treat a sub-second network blip the same as a permanent
failure — stalling a headless drain for ~5 min of morning recovery on
something that resolved in seconds. Add a thin `network_retry` module that
wraps each orchestrator-side gh subprocess in a 3-attempt exponential-backoff
loop (1s, 4s) keyed off a configurable transient-error allow-list. Retry
events surface to stderr (`↻ gh pr merge 157 — transient error (attempt 1/3) —
retrying in 1000ms`) AND to `.aida/drain-state.json` so post-hoc analysis can
correlate stalls with API health.

### Call flow

```
   Phase 4 merge ──► gh pr merge (retry × 3, transient classify)
                              │
                              ├── success ──► echo stdout/stderr, Ok
                              └── final fail ──► PhaseFailure (orchestrator
                                                  proceeds to recovery hint)

   Phase 3 reconcile ──► detect_merged_pr ──► pr_is_merged_with_sink
   shipped_spec_id   ──► pr_credited_spec_id_with_sink
                              │
                              └── transient retry, log to stderr + drain-state
```

## Decisions

- **Retry inside the shared helpers, not above them**: `pr_is_merged_with_sink`
  / `pr_credited_spec_id_with_sink` take a `&mut dyn RetrySink` so the
  orchestrator passes its drain-state-recording sink while keeping retry
  behaviour testable in isolation. **Rationale**: avoids duplicating the gh
  invocation logic across orchestrator and helper.
- **Patterns are configurable, not hardcoded**: `.aida/config.toml
  [orchestrator] transient_error_patterns = [...]` overrides the default
  allow-list. **Rationale**: real-world transient stderr is provider-specific
  (gh CLI version, git transport); a project that hits a class of blips the
  defaults miss needs an escape hatch.
- **Capture-output, then echo on completion**: `gh pr merge` originally used
  `.status()` to stream gh's confirmation line to the user's tty. Retry
  needs `.output()` to inspect stderr for transient classification, so we
  echo the captured streams after the final attempt. **Rationale**:
  preserves the user-visible "✓ Squashed and merged" without giving up
  retry detection.
- **Default attempts: 3, backoff: 1s/4s**: worst-case ~5s wait before giving
  up. **Rationale**: the BUG-286 incident resolved on the immediate retry;
  the long tail (TLS renegotiation, DNS flap) settles inside the 4s wait.
  Going wider (5×, 1s/4s/16s/64s) makes a real outage feel like a stall.
- **Don't wrap `aida pull` (phase 5) yet**: phase 5 invokes the `aida pull`
  subprocess which has its own internal git fetch/pull logic. Wrapping the
  parent subprocess in retry would lose the live terminal progress and mask
  partial-state edge cases (fetched but not pulled). Filed as TASK followup
  rather than rolled into this BUG.

## Files (in build-order)

### `aida-cli/src/network_retry.rs` — new module

- `struct RetryConfig`: 3-attempt × 1s-base × 4× factor defaults, loadable
  from `.aida/config.toml [orchestrator]`.
- `fn default_transient_patterns`: 23-entry allow-list covering gh/git
  network, TLS, HTTP 5xx, and GitHub-specific transient shapes.
- `trait RetrySink` + `StderrSink` + `NoopSink` + `DualSink`: pluggable
  retry-event sinks; the orchestrator composes `Stderr × DrainState`.
- `fn run_with_retry`: capture-output loop. Builds a fresh `Command` per
  attempt (since `Command` isn't `Clone`), classifies stderr, applies
  backoff, returns the final `Output` for the caller to decide.

### `aida-cli/src/drain_state.rs` — record retries

- `struct DrainState`: add `retries: Vec<DrainRetry>` field (`skip_serializing_if =
  "Vec::is_empty"` so a blip-free drain leaves the file shape unchanged).
- `struct DrainRetry`: label, spec, phase, attempt/max, backoff_ms,
  stderr_snippet, RFC-3339 timestamp.
- `fn append_retry`: best-effort append to the live drain-state file.
- `struct DrainStateSink`: `RetrySink` impl that calls `append_retry`.

### `aida-cli/src/main.rs` — wire orchestrator phases

- `mod network_retry;`: register the new module.
- `fn pr_is_merged` → `fn pr_is_merged_with_sink`: take a sink, wrap the gh
  call in `run_with_retry`.
- `fn pr_credited_spec_id` → `fn pr_credited_spec_id_with_sink`: same shape.
- `RealPhaseDriver::merge`: wrap `gh pr merge` through the dual sink.
- `RealPhaseDriver::detect_merged_pr`: thread a dual sink into the
  reconcile read.
- `RealPhaseDriver::shipped_spec_id`: thread a dual sink into the BUG-245
  credited-spec read.

## Critical Files

- `aida-cli/src/network_retry.rs`
- `aida-cli/src/drain_state.rs`
- `aida-cli/src/main.rs`

## Reusable helpers

- `network_retry::run_with_retry` is the single retry entry point.
  Future callers (phase 5 `aida pull` wrapping, `aida push` warning's
  `gh pr list` read, the `aida fetch` two-leg refresh) should route through
  it rather than re-implementing the classify + backoff loop.

## Risks + gotchas

- **`gh pr merge` stderr echoing**: switching from `.status()` to
  `.output()` means the user no longer sees gh's progress live — they see
  it all at once when the orchestrator echoes after the final attempt
  completes. Acceptable for a sub-second command but would feel laggy if a
  future call has long-running output.
- **`Command` is not `Clone`**: `run_with_retry` takes a `FnMut() -> Command`
  closure so it can re-spawn. Callers must not capture state that mutates
  across attempts (e.g. file handles) inside the closure.
- **`tempfile` is a dev-dependency only**: tests in `network_retry::tests`
  use it inside `#[cfg(test)]`, so release builds aren't affected.

## Tests

- `network_retry::tests::classifies_transient_stderr` — pattern matching
  positive case.
- `network_retry::tests::classifies_permanent_stderr` — pattern matching
  negative case.
- `network_retry::tests::empty_pattern_does_not_match_everything` — guards
  against an empty config entry classifying every stderr as transient.
- `network_retry::tests::default_patterns_cover_bug_286_symptom` — the
  exact BUG-286 stderr (`error connecting to api.github.com`) is classified
  transient by defaults.
- `network_retry::tests::retries_then_succeeds_on_transient` — fake
  subprocess fails twice with transient stderr, succeeds third — asserts 2
  RetryEvents logged + final success.
- `network_retry::tests::permanent_failure_is_not_retried` — fake
  subprocess fails with HTTP 404, asserts zero retries.
- `network_retry::tests::transient_failure_exhausts_attempts` — fake
  subprocess fails forever with transient stderr, asserts retries hit max
  and the final failure Output is returned.
- `network_retry::tests::success_on_first_attempt_logs_nothing` — happy
  path doesn't pollute the sink.
- `network_retry::tests::dual_sink_fans_out` — `DualSink` invokes both
  inner sinks.
- `network_retry::tests::config_load_reads_overrides` — `[orchestrator]`
  TOML overrides land.
- `network_retry::tests::config_load_uses_defaults_for_missing_file` —
  missing config falls back to defaults.
- `network_retry::tests::config_load_uses_defaults_for_malformed_toml` —
  broken TOML doesn't poison the retry path.
- `network_retry::tests::max_attempts_clamped_to_safe_range` — silly
  values are clamped.
- `network_retry::tests::integration_fake_gh_with_transient_blip` — fake
  gh subprocess that emits the literal BUG-286 stderr; asserts the
  orchestrator-shape `label` flows through.

## Verification

```bash
cargo build -p aida-cli
cargo test -p aida-cli --bin aida network_retry
cargo test -p aida-cli --bin aida drain_state
cargo test -p aida-cli --bin aida          # full suite, 934 tests
cargo fmt --all -- --check
```

The next end-to-end exercise will be the orchestrator merge that ships
this BUG itself: a real `gh pr merge` going through the new retry path,
with this BUG's own drain as the empirical witness.

## Followups

- Wrap phase 5 `aida pull` subprocess in a coarser retry-on-failure that
  inspects the captured stderr for transient patterns — the parallel for
  `git fetch` / `git push` blips during the pull leg. Worth its own TASK
  because the design tradeoffs (live progress vs capture, partial-state)
  differ from a single gh subprocess.
- Route `gh_pr_list_first`'s gh subprocess through `network_retry` to
  collapse its `PrLookup::GhUnreachable` path into the same retry-on-
  transient mechanism. Currently the function classifies network errors
  into a distinct enum variant the caller handles separately — retry would
  make the variant fire only on persistent outages.
- Add a `[orchestrator] retry_backoff_factor` knob so a project that wants
  longer waits (a multi-region outage scenario) can tune without code
  changes.

## Related

- BUG-257 — phase-1 implementer's gh-CLI transient classification (already
  shipped). This BUG is the parallel for phases 3-6.
- BUG-266 — phase-1 Anthropic 529 inconclusive (already shipped).
- BUG-241 — the reconcile step that this BUG's retry hardens.
- BUG-245 — the dispatched-vs-credited check that this BUG's retry
  hardens.
- STORY-384 — `aida queue recover` wizard. After this BUG ships, the
  wizard sees fewer stalled-mid-merge cases.
