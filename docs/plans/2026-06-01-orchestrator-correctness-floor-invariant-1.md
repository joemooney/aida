# Orchestrator correctness floor — invariant 1 (lease lifecycle) + the held-outcome

**Date:** 2026-06-01 · **Specs:** TASK-133, BUG-431 (#1, #3), BUG-250 · **Epic:** EPIC-33 · **Status:** implemented (3 branches, pure cores unit-tested) — pending supervised `--zen --auto-complete` drain validation · **Complexity:** high (keystone orchestrator)

## Execution log (2026-06-01)
- **TASK-133** — shipped on `task-133-phase1-reset-compensation`. Pure core `should_compensate_phase1_bump` (5 cases) + `restore_phase1_status_on_lease_failure`. On a phase-1 failure with no lease recorded, restores the captured prior status and clears the spurious shelve `failure_reason`.
- **BUG-431 #1** — shipped on `bug-431-story-scope` (stacked on TASK-133). `derive_scope_from_entry` no longer falls back to the parent epic; a child story scopes to its own id. Worktree + branch follow (both slugify the scope). Parent-epic label kept for display clustering only.
- **BUG-431 #3** — re-evaluated: mooted by #1 (story-scoped leases don't block siblings). Residual lease-release-preserve-worktree deferred to post-drain evaluation (recorded as a BUG-431 comment); not built speculatively.
- **BUG-250** — shipped on `bug-250-held-outcome` (stacked on BUG-431 #1). `ImplementerOutcome::Held` + `finish_held` + `BatchDrainOutcome::Held` + `aida pr hold` + the `punt::HoldSignal` handshake. Scoped to the outcome model (acceptance #1-4); the `--resume` re-entry (#5) is filed as **TASK-630**.
- **Remaining:** the supervised `aida queue work next N --zen --auto-complete` drain on a multi-story-epic batch (validates items 1+2 together; a push-hold-PR finish validates 4). Per TASK-456: keyboard, not `--no-human`.

Operator signed off BUG-431 #1 (story-scoping) on 2026-06-01. All four are queue-work/orchestrator-drain changes whose correctness only shows under a live `aida queue work --zen --auto-complete` startup — so **validation requires a supervised drain**, not unit tests alone. Do them in one focused session (clean context), in the order below.

## Key correction (discovery 2026-06-01)
TASK-133's bug is **not** in `session_start` — that function already writes the lease (`main.rs:25115`) *before* bumping status (`25134`), which is the correct order. The real bug is the **orchestrator parent**: `prepare_auto_complete_phase1_status` (`main.rs:72994`, called at `71023`) flips Approved→InProgress *before* spawning the implementer child that acquires the lease (intentional, per BUG-369 — the child treats InProgress-without-lease as "parent corroborated"). If the child's lease-acquire (or spawn) then fails, the spec is stranded InProgress→NeedsAttention with no lease and no clean reset.

## Sequence (dependency order)

### 1. TASK-133 — reset-on-failure compensation (most bounded; do first)
- The orchestrator phase-1 setup (`main.rs:71023`, around `prepare_auto_complete_phase1_status`) bumped status. If phase 1 then fails to acquire a lease / spawn, **reset the spec to its prior status** (the value `prepare_auto_complete_phase1_status` returns as the old status — see its `Some((id, old_status))` return, test `prepare_auto_complete_phase1_status_flips_approved_before_spawn`).
- Capture the pre-bump status; on the phase-1 failure path where no lease was recorded for the spec, restore it. Also fix the misleading error ("already In Progress" → "phase-1 startup failed to acquire lease; status restored to <prior>").
- **Test:** pure — given (bumped=true, lease_acquired=false, prior=Approved) → restore to Approved. **Drain-validate:** a spec whose lease-acquire is forced to fail ends Approved (re-queueable), not stranded NeedsAttention.

### 2. BUG-431 #1 — story-scope queue-work sessions (architectural root)
- Find the scope-derivation that yields `scope: EPIC-11 (cluster-derived)` for a child story (the `cluster`/`scope_root` path — `main.rs` has `scope_root` + planned-cluster manifest logic ~22944/33xxx; pinpoint the queue-work session scope resolver). Change it so a story's session scopes to the **story's own** id, not the parent epic.
- Knock-on: worktree name (`quizdom-epic-11` → `quizdom-story-76`) and branch (`epic-11` → story branch) derive from scope — verify they follow. Confirm the planned-cluster manifest (visual clustering) is **separate** from the session scope and unaffected (clustering for display ≠ lease scope).
- **Drain-validate:** `aida queue work next N --no-human=both` on a batch of same-epic stories — each gets its own scope/worktree/branch; no contention.

### 3. BUG-431 #3 — re-evaluate, then (if needed) release-lease-preserve-worktree
- The sibling-cascade is mostly mooted by #1 (story-scoped leases don't block siblings). **After #1 + the drain**, check whether a residual problem remains. If it does: on a shelvable failure, release the lease (free the scope) while **preserving** the worktree (the failure path deliberately keeps it for triage — `auto_complete.rs:947`). That's a lease/worktree-ownership decoupling — design carefully; don't auto-delete a worktree with uncommitted triage state.

### 4. BUG-250 — `Held` outcome (sketch-signed; can fold into the same drain)
- Per the sketch on BUG-250: (a) a `pr-held` sentinel (mirror the punt handshake) recording {spec, branch, reason}; (b) `ImplementerOutcome::Held { reason, branch }` in `auto_complete.rs` → non-failure drain action (record done-on-branch/PR-held marker, advance, correct hint); (c) the held marker stays **resumable** (`aida queue work <spec> --resume` accepts it; not Done-terminal).
- **Drain-validate:** a push-branch-hold-PR finish reports Held (not phase-1 failure) and the session re-enters to finish the deferred PR.

## Critical files
- `aida-cli/src/main.rs` — `prepare_auto_complete_phase1_status` (72994 / call 71023), `session_start` (24456), the scope/cluster derivation (`scope_root`, planned-cluster), `resolve_pr_to_spec` (already most-specific, BUG-431 #2 done).
- `aida-cli/src/auto_complete.rs` — `ImplementerOutcome` (held variant), phase-1 finish/failure path, the `947` worktree-preserve hint.

## Verification (executable)
1. `cargo test -p aida-cli --bin aida` (the pure cores: status-reset, held-outcome decision).
2. **Supervised `--zen --auto-complete` drain** (advisor + operator watching), one multi-story-epic batch, validating items 1+2 (+3 if kept) together; a separate push-hold-PR finish validates 4. Per TASK-456 (recursive-failure-risk → keyboard, not `--no-human`).

## Why a fresh context
The edits are intricate + interdependent + keystone; a mistake hangs the drain (the recursive failure this whole floor guards against). Execute with full context budget. This plan starts execution at the implementation, not discovery.

## Related
EPIC-33 (correctness floor), EPIC-28 (resilient drain / outcome model), BUG-369 (the bump-before-spawn design), BUG-431 #2 (resolve_pr_to_spec, shipped).
