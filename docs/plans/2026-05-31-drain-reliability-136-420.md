# Drain-reliability hardening — TASK-136 + BUG-420

**Date:** 2026-05-31 · **Specs:** TASK-136, BUG-420 · **Status:** planned (decisions locked, implementation pending) · **Complexity:** medium (keystone orchestrator)

Operator-approved approaches (2026-05-31): TASK-136 = skip-shelve + short retry; BUG-420 = no-progress watchdog + wall-clock ceiling backstop. Both ride existing `auto_complete.rs` primitives. **Validation requires a supervised `--zen --auto-complete` drain** (the fix rides through the keystone; keyboard-watch it).

## Approach

Both changes funnel a previously-fatal/stuck state into the existing **EPIC-28 shelve-and-advance** path so an unattended batch keeps moving and a human triages later via `aida findings list`.

```
phase-1 verify GH ──unreachable──► retry(backoff ×N) ──still?──► shelve_on_failure ──► batch advances
headless phase running ──no-progress N min OR >ceiling──► abort child ──► shelve_on_failure ──► batch advances
```

## TASK-136 — inconclusive → skip-shelve + short retry

Today: GH-verify unreachable → `Inconclusive` outcome → `finish_inconclusive` → batch **pauses** (the 12h stall).

1. **Retry the verify** where the `Inconclusive` outcome is produced (the GH PR-existence check in phase-1 finish; the `Inconclusive` variant ~`auto_complete.rs:514`, `finish_inconclusive` ~544). Wrap the verify in a bounded backoff loop — 30s, 1m, 5m (config `[drain] gh_verify_retries`, default 3). Transient blips clear here.
2. **Still inconclusive → shelve, don't pause.** Route the post-retry inconclusive case through `shelve_on_failure` (~755) with a `FailureReason { kind: "pr-verification-inconclusive", detail: "GH unreachable after N retries", recovery_hint: "re-run the drain when GH is reachable; the spec is shelved, not failed" }`. Set `shelved_reason` so the batch loop treats it as a *recoverable shelve* (skip dependents, continue) — NOT `finish_inconclusive` (which stops). The batch loop already advances on `shelved_reason`.
3. **Exit code:** a shelved-inconclusive run contributes to the existing "exit 2 = something shelved/skipped" semantics (not a hard failure). Keep the `inconclusive_reason` field for the single-spec (non-batch) case where pausing-to-retry-later is still reasonable; the shelve+advance behavior is batch-mode.

**Risk:** the `Inconclusive` vs `shelve_on_failure` paths are distinct today (different exit/triage semantics). Don't collapse them — *add* a batch-mode branch that shelves inconclusive, leaving the single-spec inconclusive-pauses behavior intact.

## BUG-420 — no-progress watchdog + ceiling backstop

Today: a headless phase that degenerates (echo/sleep filler after committing) runs until killed.

1. **Watchdog thread/poll** around the headless phase spawn-and-wait (where the orchestrator `command.output()`/waits on the `claude -p` child). While the child runs, poll the worktree every ~60s:
   - new commit since last poll? (`git -C <worktree> rev-parse HEAD`)
   - any working-tree file mtime change? (`git -C <worktree> status --porcelain` non-empty delta, or a max-mtime walk)
2. **No-progress trip:** if BOTH are unchanged for `[drain] no_progress_minutes` (default 10) → kill the child → `shelve_on_failure` with `FailureReason { kind: "no-progress-watchdog", detail: "phase made no commit/file-change for Nm — likely degenerate" }`.
3. **Ceiling backstop:** a hard cap `[drain] phase_ceiling_minutes` (default 45) → kill + shelve regardless (in case progress-detection misses).
4. Both thresholds flag-overridable (`--no-progress-minutes`, `--phase-ceiling-minutes`); `0` disables a check.

**Risk:** killing the child cleanly (process-group kill so the headless `claude -p` and its tool subprocesses die) + not racing a child that's mid-commit (poll the commit SHA *after* the kill decision; if a commit landed in the grace window, treat as progress and reset).

## Critical files
- `aida-cli/src/auto_complete.rs` — `Inconclusive` (~514), `finish_inconclusive` (~544), `shelve_on_failure` (~755), `shelved_reason`/`inconclusive_reason` fields (~631-640), the batch-drain loop, and the headless phase spawn-and-wait.
- `aida-cli/src/main.rs` — `aida queue work` flag wiring for the new `--no-progress-minutes` / `--phase-ceiling-minutes` / retry config.

## Tests (named)
- `inconclusive_after_retries_shelves_and_advances_in_batch` — pure: an Inconclusive outcome in batch mode produces a `shelved_reason` + advances, not a pause.
- `single_spec_inconclusive_still_pauses` — the non-batch path is unchanged.
- `gh_verify_retry_backoff_schedule` — pure: the backoff sequence is 30s/1m/5m for N=3.
- `watchdog_trips_on_no_commit_no_filechange` — pure decision fn: given (last_commit, last_mtime, elapsed) → abort verdict.
- `watchdog_resets_on_observed_progress` — a new commit/file-change resets the timer.
- `ceiling_trips_regardless_of_progress` — wall-clock cap.

## Verification (executable)
1. `cargo test -p aida-cli --bin aida` (the pure decision fns above).
2. **Supervised** validation drain (keyboard, advisor watching): a batch where one member's GH-verify is forced inconclusive (e.g. offline) → confirm it shelves + advances, `aida findings list` shows it. A member rigged to spin → confirm the watchdog aborts + shelves within ~N min.

## Followups
- BUG-425 (bulk-archive batch+progress) is independent — ship separately.
- Consider surfacing watchdog/shelve events as a live `aida status` signal during unattended drains.

## Related
- EPIC-28 (resilient drain / shelve-and-advance) — the primitive both reuse.
- BUG-257/BUG-266 (the Inconclusive outcome) — TASK-136 extends it.
- `docs/autonomous-drain.md` — update the shelving section once shipped.
