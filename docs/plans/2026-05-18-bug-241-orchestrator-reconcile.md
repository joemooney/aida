# Plan: BUG-241 — Orchestrator reconciles against reality before failing any phase

Date: 2026-05-18
Specs: BUG-241
Status: Completed
Complexity: ~210 prod LOC, ~120 test LOC, 1 commit, risk low

<!--
  Reconcile step only — BUG-241 items 1,2,3,6,7. Items 4-5 (the escalation
  handshake) were folded into STORY-306's acceptance criteria, not filed as a
  standalone TASK. trace:BUG-241
-->

## Approach

The `--auto-complete` orchestrator equated "the phase ended without the artifact
I poll for" (an open PR, a verdict file) with "the phase failed." That is a
false equivalence: a phase can end abnormally-but-successfully — a spec resolved
by supersession needs no PR (instance B), a reviewer escalation lets a human
merge out-of-band so no verdict file is written (instance A). The fix adds one
phase-agnostic **reconcile step**: before `orchestrate()` declares *any* phase a
failure, it asks the driver — via the new `PhaseDriver::reconcile_failure` hook —
whether ground truth shows the spec shipped anyway. `RealPhaseDriver` answers by
checking a merged PR (`gh pr view`) and the spec's status in the git store. When
reality confirms the spec shipped, the run becomes a `finish_reconciled` success
(exit 0, no `failed_phase`) so the batch advances instead of crashing with a
false "shipped 0". When reality confirms nothing shipped, the failure stands
unchanged — the reconcile step only ever *ratifies* a real success.

### Diagram

```
  phase Err(f) ──► resolve_phase_failure ──► driver.reconcile_failure(phase, f)
                                                  │
                        ┌─────────────────────────┴───────────────────────┐
              ShippedOutOfBand{reason}                            GenuineFailure
                        │                                                 │
                 finish_reconciled                                 finish_failure
            (exit 0, batch advances)                       (exit = phase index, as before)
```

## Decisions

- **Reconcile in `orchestrate()`, not per-phase**. **Rationale**: the spec
  mandates a phase-agnostic fix, not a phase-3 patch. `orchestrate()` is the
  single place every phase failure funnels through, so one `resolve_phase_failure`
  seam covers all seven failure sites.
- **A new `PhaseDriver::reconcile_failure` trait method with a `GenuineFailure`
  default**. **Rationale**: keeps the pure orchestration loop testable against a
  mock; the default means a driver that cannot check reality leaves every failure
  standing (the conservative direction).
- **`RealPhaseDriver` reconciles only Phase 1 and Phase 3**. **Rationale**: those
  two phases fail by *not observing an expected artifact*, which can legitimately
  be absent on success. Phases 2/4/5/6 fail by a gate or command genuinely not
  passing (red CI, a failed merge, a divergent pull, a broken build) — reconciling
  those would mask a real failure. The mechanism is phase-agnostic; the evidence
  is grounded in what each phase's failure means.
- **Ground truth = a merged PR OR a Completed spec (either alone)**. **Rationale**:
  requiring both would miss instance A (the status auto-bump lags a human's
  out-of-band merge) and instance B (a no-work spec never gets a PR).
- **A `ShippedOutOfBand` reconcile short-circuits the whole run to success**.
  **Rationale**: in both verified instances the out-of-band success means the
  spec is fully resolved; the remaining phases would themselves fail (merge an
  already-merged PR, etc.). Returning `finish_reconciled` matches the spec's
  "treat the phase as succeeded, advance the batch."
- **Items 4-5 (escalation handshake) folded into STORY-306, not a new TASK**.
  **Rationale**: they are STORY-306's escalation mechanism and belong with its
  A/B/C calibration + punt-channel design. The reconcile step alone fixes both
  of BUG-241's verified instances.

## Files (in build-order)

### `aida-cli/src/auto_complete.rs` — phase-agnostic reconcile seam

- `enum PhaseReconcile`: new — `GenuineFailure` | `ShippedOutOfBand { reason }`.
- `trait PhaseDriver`: add `fn reconcile_failure(&mut self, Phase, &PhaseFailure)
  -> PhaseReconcile` with a `GenuineFailure` default.
- `fn finish_reconciled`: new — prints the reconciled epilogue, builds a success
  `OrchestrationResult` (exit 0, `failed_phase: None`).
- `fn resolve_phase_failure`: new — routes a phase `Err` through
  `reconcile_failure`, dispatching to `finish_reconciled` or `finish_failure`.
- `fn orchestrate`: all 7 failure sites call `resolve_phase_failure` instead of
  `finish_failure` directly.
- `struct MockPhaseDriver`: add a `reconcile` field + `reconciles_as` builder +
  `reconcile_failure` impl (test scaffolding).

### `aida-cli/src/main.rs` — `RealPhaseDriver` ground-truth check

- `fn pr_is_merged`: new — `gh pr view <N> --json state`; `None` on any `gh`
  failure (cannot-confirm → failure stands).
- `fn spec_status`: new — reads a spec's status from the git-canonical store.
- `fn reconcile_verdict`: new — pure decision over `(merged_pr, spec_completed)`.
- `RealPhaseDriver::detect_merged_pr`: new method — prefers a known PR number,
  else a branch-keyed merged-PR lookup.
- `RealPhaseDriver::reconcile_failure`: new — gates to Phase 1/3, skips
  un-redeemable failure kinds, then calls `reconcile_verdict`.

## Critical Files

- `aida-cli/src/auto_complete.rs`
- `aida-cli/src/main.rs`
- `.aida-store/objects/STORY/000/STORY-306.yaml` (acceptance criteria for the
  absorbed items 4-5)

## Reusable helpers (do not reimplement)

- `detect_merged_pr_for_branch` (`aida-cli/src/main.rs`) — branch-keyed merged-PR
  lookup, reused by `detect_merged_pr`.
- `resolve_gh_binary` (`aida-cli/src/main.rs`) — PATH-robust `gh` resolution.
- `aida_core::GitBackend` + `DatabaseBackend::get_requirement_by_spec_id` —
  canonical spec read, used by `spec_status`.
- `finish_failure` / `finish_success` (`auto_complete.rs`) — the existing
  epilogue builders `finish_reconciled` mirrors.

## Risks + gotchas

1. **Risk**: a reconcile false-positive masks a real failure. **Mitigation**:
   `reconcile_verdict` returns `GenuineFailure` whenever no merged PR exists and
   the spec is not Completed; only phases 1 & 3 are reconciled at all; un-redeemable
   kinds (Spawn/MissingTool/Internal) short-circuit. Regression-guard tests pin it.
2. **Risk**: a `gh` outage during reconcile. **Mitigation**: `pr_is_merged`
   returns `None` on any `gh` failure → treated as "cannot confirm a merge" →
   the failure stands. Fail-safe, never a silent success.
3. **Risk**: the spec-status read sees a stale cache. **Mitigation**: `spec_status`
   reads `GitBackend` (the `.aida-store` worktree YAML) directly — canonical
   ground truth, no cache staleness.
4. **Risk**: a reconciled phase-3 success skips phases 5-6 (pull/build), leaving
   local `main` slightly behind. **Mitigation**: accepted — the spec explicitly
   says reconcile → advance the batch; the next spec's pull or a manual pull
   catches up. Not in scope for BUG-241.

## Tests (named, not "add tests")

- `orchestrate_reconciles_phase1_no_work_spec` — instance B: phase-1 failure +
  `ShippedOutOfBand` → exit 0, no `failed_phase`.
- `orchestrate_reconciles_phase3_out_of_band_merge` — instance A: phase-3 failure
  + `ShippedOutOfBand` → exit 0.
- `orchestrate_reconciles_rejected_verdict_when_pr_merged` — a non-Approved
  verdict is also routed through reconcile.
- `orchestrate_genuine_phase1_failure_still_fails` — regression guard: default
  `GenuineFailure` → exit 1.
- `orchestrate_genuine_phase3_failure_still_fails` — regression guard at phase 3.
- `reconcile_failure_default_is_genuine_failure` — pins the conservative default.
- `reconcile_verdict_tests::*` — the four `(merged_pr, completed)` quadrants.

## Verification

```bash
cd /home/joe/ai/aida-bug-241
cargo build -p aida-cli                        # compiles clean
cargo test -p aida-cli --bins 2>&1 | tail -1   # expect: test result: ok. 679+ passed
cargo test -p aida-cli reconcile 2>&1 | grep 'test result'   # reconcile suite green
cargo fmt --all -- --check                     # exit 0
```

## Followups

None. The escalation handshake (BUG-241 items 4-5) is absorbed into STORY-306's
acceptance criteria, not a standalone follow-up TASK — deliberately, so the
escalation design lives in one place with STORY-306's A/B/C calibration and punt
channel.

## Related

- Builds on: STORY-246 (auto-complete orchestrator), STORY-263 (verdict-file
  handshake), BUG-233 (orchestrator corroboration), BUG-237 (zen corroboration —
  fixed instance A's original trigger).
- Composes with: STORY-306 (advisor escalation tier — now owns items 4-5),
  BUG-236 (orchestrator recovery-hint accuracy — sibling false-signal family).
