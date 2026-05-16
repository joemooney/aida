# BUG-218 — auto-complete recovery hints: classify the failure, don't assume "CI is red"

- **Date:** 2026-05-16
- **Specs:** BUG-218
- **Status:** Complete
- **Complexity:** Low

## Approach

`aida queue work --auto-complete` orchestrates six phases. When a phase fails,
`finish_failure` prints a one-line `reason` plus a `→` recovery hint. The hint
came from `recovery_hint(phase, ctx)` — keyed **only on the phase index**. So
*every* phase-2 failure rendered "CI is red", including a subprocess spawn
ENOENT in the orchestrator's invocation of `aida session end`. The hint then
mis-routed the user to GitHub Actions when CI was green and the real bug was
local subprocess plumbing.

Fix: give each failure a **kind**, and key the hint on `(phase, kind)`.

```
PhaseFailure { reason, kind: FailureKind }
        │
        ├─ driver tags each failure: Spawn / CiRed / CiTimeout / NoPr /
        │                            MissingTool / NoVerdict / Internal / Failed
        ▼
recovery_hint(phase, kind, ctx)
        │
        ├─ Spawn    → "Subprocess spawn failed … not CI … finish by hand: <cmd>"
        ├─ Internal → "orchestrator bug … please file it"
        └─ (phase, kind) → phase- and kind-specific concrete next command
```

`Spawn` and `Internal` are cross-cutting (matched before the per-phase logic);
the rest are phase-specific. The phase default (`FailureKind::Failed`) keeps
the pre-BUG-218 wording, so the only behaviour change is that a *misclassified*
failure now gets its own hint instead of borrowing the phase default.

## Decisions

- **`FailureKind` enum, not free-text matching.** Classifying at the failure
  site (the driver knows it caught an ENOENT vs. a red CI probe) is reliable;
  re-deriving the kind by scanning `reason` strings would be fragile.
- **`Spawn` is cross-phase.** The orchestrator shells out in all six phases, so
  a spawn ENOENT can happen anywhere — one arm, with the manual-fallback
  command parameterised per phase.
- **`Internal` for invariant violations.** "branch not resolved", "lease not
  recorded", "PR not resolved" are AIDA bugs, not user errors — the hint says
  so and routes to `aida add --type bug` rather than blaming the user's work.
- **`PhaseFailure::new` stays the `Failed` default.** Most driver call sites
  are genuine work-failed cases; only the misclassified ones move to
  `PhaseFailure::of(kind, …)`. Minimal churn, no behaviour change for the
  already-correct hints.
- **`kind` slug added to the `--json` failed event.** Cheap, and feeds the
  failure-pattern telemetry that TASK-266 will mine to refine hints.

## Files (build order)

1. `aida-cli/src/auto_complete.rs` — `FailureKind` enum + `slug()`; `kind`
   field on `PhaseFailure` + `PhaseFailure::of`; `recovery_hint` rewritten to
   `(phase, kind, ctx)`; `finish_failure` passes `failure.kind` and emits it
   in the JSON event; tests.
2. `aida-cli/src/main.rs` — `RealPhaseDriver` + `read_verdict_file` tag each
   failure with its kind (`Spawn` / `CiRed` / `CiTimeout` / `NoPr` /
   `MissingTool` / `NoVerdict` / `Internal`).

## Critical Files

- `aida-cli/src/auto_complete.rs` — `recovery_hint` is the whole fix; pure and
  unit-tested independent of the driver.
- `aida-cli/src/main.rs` — `RealPhaseDriver::finish_ci` is where the reported
  incident originated (the `aida session end` spawn now tagged `Spawn`).

## Reusable helpers

- `PhaseFailure::of(kind, reason)` — the tagged constructor; use it at any new
  failure site instead of `PhaseFailure::new` when the kind is known.
- `FailureKind::slug()` — stable machine slug for telemetry/JSON.

## Risks + gotchas

- A new failure site that uses `PhaseFailure::new` silently defaults to
  `Failed` (phase default hint). Acceptable — that is the safe fallback — but
  prefer `::of` when the kind is knowable.
- `recovery_hint`'s `match (phase, kind)` is exhaustive via per-phase `_`
  arms; adding a `Phase` or `FailureKind` variant compiles fine but inherits
  the default arm — review the hint when extending either enum.

## Tests (named, in `auto_complete.rs`)

- `recovery_hint_ci_spawn_does_not_blame_ci` — the BUG-218 regression: phase-2
  `Spawn` hint contains "Subprocess spawn failed", not "CI is red"/"CI failed".
- `recovery_hint_ci_red_names_run_view_and_steal` / `…_without_run_id_…` —
  `CiRed` still names `gh run view` / `gh run list`.
- `recovery_hint_ci_timeout_says_not_red` — timeout ≠ red run.
- `recovery_hint_spawn_names_each_phases_manual_fallback` — every phase's
  `Spawn` hint names that phase's by-hand command.
- `recovery_hint_internal_routes_to_file_a_bug` — `Internal` → file a bug.
- `recovery_hint_three_distinguishable_patterns_per_phase` — acceptance #4:
  Spawn / Internal / Failed yield 3 distinct hints for every phase.
- `recovery_hint_implementer_no_pr_…`, `…_missing_tool_…`,
  `…_reviewer_no_verdict_…`, and the `…_failed_…` phase-default tests.

## Verification

```bash
cargo build -p aida-cli
cargo test -p aida-cli auto_complete   # 36 passed
```

## Followups

- TASK-266 can now bucket auto-complete failures by the `kind` slug in the
  JSON event to see which hints fire most and need sharpening.

## Related

- STORY-246 — the auto-complete orchestrator this hints into.
- BUG-217 — the mid-flight `cargo build` ENOENT the `Spawn` hint references.
- batch:workflow-hint-polish — TASK-267 / TASK-268, same hint-quality family.
