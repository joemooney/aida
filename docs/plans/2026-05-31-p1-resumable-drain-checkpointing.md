# Implementation plan — P1: resumable orchestrator drain checkpointing

**Date:** 2026-05-31 · **Specs:** SPIKE-45 P1 (to be filed as a STORY) · **Status:** Sketch — needs sign-off before build (architecture-class, touches core drain control flow) · **Complexity:** Large

> Close the durable-execution gap LangGraph/CrewAI/Temporal have and AIDA lacks (round-2 competitive analysis, P1). When `aida queue work --auto-complete` crashes mid-drain, it restarts the spec from phase 1 instead of resuming. Make the drain step-resumable. Authored from an inventory of the existing orchestrator/drain machinery (2026-05-31).

## The key insight (changes the whole approach)

The persistence substrate **already exists**: `.aida/drain-state.json` (STORY-301) records `current`, `current_phase` (e.g. `"3 (reviewer)"`), `orchestrator_pid`, `members[].state` (`in-phase-N`/`completed`/`failed`), and `run_uuid`, written atomically at every transition (`drain_state::set_phase` ← `mark_drain_phase`; `set_member_outcome`). So P1 is **not** "build checkpointing from scratch" — it is "add a *resume* path on top of the state we already write."

And the resume must **reconcile against reality, not replay a log.** Each phase has real side effects (a branch pushed, a PR opened, a squash-merge landed, the spec auto-bumped). A checkpoint saying "crashed in phase 4 (merge)" is only a *hint* — the merge may have actually completed before the crash. So resume computes the re-entry phase from the **actual world-state** (does the branch exist? is the PR open/merged? what's the spec's status?), using the checkpoint only to disambiguate. This matches AIDA's substrate-as-source-of-truth philosophy and is far more robust than trusting a log. **This is the central design decision and the main thing to sign off on.**

## Approach

```
aida queue work --resume [--drain-id <id>]
        │
        ▼
  1. Read .aida/drain-state.json  (existing DrainState)
  2. Liveness gate: is orchestrator_pid alive? → if YES, refuse (drain still running)
  3. For the `current` member: RECONCILE actual state ─────────────┐
       branch exists? PR open? PR merged? spec status?            │ reconcile_resume_phase()
       → compute the earliest phase whose postcondition is unmet  │ (the new core fn)
  4. Re-enter orchestrate_with_lifecycle_skip() at that phase ─────┘
       (phases before it are skipped because their effects already exist)
  5. Continue the drain normally (batch loop picks up remaining members)
```

Resume is `reconcile → re-enter`, not `replay`. The reconciliation function is the heart; the phase loop already exists.

## Key decisions (for sign-off)

1. **Reconcile-from-reality, not checkpoint-replay** (above). The checkpoint is a hint; git/PR/spec-status is truth.
2. **Per-phase postconditions define the resume point.** Define, for each `Phase`, an idempotent "is this phase's effect already present?" check:
   - Implementer(1): branch exists with commits referencing the spec.
   - Ci(2): CI run exists+green for the head SHA.
   - Reviewer(3): a review verdict file / approving state exists.
   - Merge(4): PR is merged (or spec is `Completed`/on default branch).
   - Pull(5): default branch locally contains the merge; spec auto-bumped.
   - Build(6): post-merge build succeeded (weakest signal — may always re-run; it's idempotent anyway).
   Resume re-enters at the **first phase whose postcondition is unmet.**
3. **Liveness gate is mandatory.** Never resume a drain whose `orchestrator_pid` is still alive (that's a double-drive, the worst outcome). Reuse the PID-liveness + worktree-clean logic from `classify_for_auto_release` (orchestrator.rs, BUG-307).
4. **Crash vs. deliberate-shelve disambiguation.** A shelved spec has member `state="failed"` + a `FailureReason` on the requirement; a crash has `state="in-phase-N"` + live-looking `run_uuid` + dead PID. Resume only re-enters `in-phase-N` members; shelved ones stay parked (EPIC-28 semantics unchanged).
5. **Stale lease cleanup on resume.** A crash leaves a session lease with a dead `creator_pid`. Resume runs the existing `classify_for_auto_release` gate before re-creating the phase child.
6. **Opt-in, explicit.** `--resume` is a deliberate flag, not automatic — auto-resuming on every `queue work` invocation risks surprising re-entry. (A later `aida drain status` could *suggest* `--resume` when it detects a resumable crashed drain.)

## Files (build order)

1. **`aida-cli/src/auto_complete.rs`** — add `fn phase_postcondition_met(phase: Phase, ctx: &ResumeCtx) -> bool` (per-phase reality checks) and `fn reconcile_resume_phase(...) -> Option<Phase>` (earliest unmet phase, or None = nothing to resume). Pure-ish, unit-testable with a mocked `ResumeCtx`.
2. **`aida-cli/src/drain_state.rs`** — add `DrainState::resumable()` helper (is `current` set + `orchestrator_pid` dead + member `in-phase-N`?). Optionally append a `phase_log: Vec<PhaseCheckpoint>` for richer diagnostics (secondary — reconciliation doesn't depend on it).
3. **`aida-cli/src/cli.rs`** — add `--resume` (+ optional `--drain-id`) to the `queue work` args.
4. **`aida-cli/src/main.rs`** — in `run_auto_complete()` / `handle_auto_complete*`, branch on `--resume`: load state, liveness-gate, reconcile, then call `orchestrate_with_lifecycle_skip()` with a new `start_phase` parameter (default `Implementer`; resume passes the reconciled phase).
5. **`orchestrate_with_lifecycle_skip()`** — thread a `start_phase: Phase` param; skip phases with `index() < start_phase.index()` (their effects already exist), emitting a `resumed-past` progress line for each.

## Critical files

- `aida-cli/src/auto_complete.rs` — `enum Phase` (@233), `orchestrate_with_lifecycle_skip` (@1681), `resolve_phase_failure` (@1478), `finish_failure` (@1306), `FailureKind::is_shelvable` (@471), `drain_batch` (@2067).
- `aida-cli/src/drain_state.rs` — `DrainState`, `set_phase` (@347), `set_member_outcome` (@405), `set_run`/`clear_run`.
- `aida-cli/src/main.rs` — `run_auto_complete` (@69533), `handle_auto_complete_batch` (@69910), `mark_drain_phase` (@72911).
- `aida-cli/src/orchestrator.rs` — `classify_for_auto_release` (@597, PID-liveness + worktree-clean gate to reuse).

## Reusable helpers (don't reimplement)

- `DrainState::read()` / `write_atomic` — the persistence is done; reuse it.
- `classify_for_auto_release` — PID liveness + dormant-lease detection (don't re-derive "is the process alive").
- The PR/branch/merge-state probing already in `aida pr ship` / the merge phase (reuse for postcondition checks — `gh pr view --json state,mergeStateStatus`, `git branch --contains`).
- `auto_bump_eligible_status` + the pull-leg scan — phase 5 postcondition (spec promoted to Completed).

## Risks + gotchas

- **Double-drive is the catastrophic failure.** If resume re-enters while the original orchestrator is somehow still alive, two processes drive the same spec. The liveness gate must be conservative — refuse on ANY doubt (PID alive, or recent mtime + can't confirm dead).
- **Non-idempotent phase re-entry.** The whole point of reconcile-from-reality: never re-open a PR that exists, never re-merge. Each postcondition check must be correct or resume causes duplicate side effects. Test each.
- **Phase 6 (build) has a weak postcondition** — it may always re-run on resume. That's acceptable (build is idempotent) but note it.
- **Headless (`--no-human`) resume** interacts with the punt/advisor handshake — a crash mid-punt is a distinct state (punt signal file present). v1 should refuse to auto-resume a mid-punt drain and route to human triage; don't try to resume an in-flight escalation.
- **Lease/worktree staleness** — resume must clean the dead orchestrator's lease before spawning a new phase child, or the lease-conflict gate blocks it.

## Tests (named)

- `reconcile::resume_phase_skips_completed_phases` — PR merged + spec Completed ⇒ reconcile returns None (nothing to resume) or Build only.
- `reconcile::resume_reenters_at_first_unmet_phase` — branch exists, PR open, not merged ⇒ re-enter at Merge(4), not Implementer.
- `reconcile::merged_before_crash_is_detected` — checkpoint says "phase 4 running" but PR is actually merged ⇒ skip merge, go to Pull.
- `resumable::live_pid_refuses_resume` — orchestrator_pid alive ⇒ `resumable()` is false / resume refuses.
- `resumable::shelved_member_is_not_resumed` — member `state="failed"` + FailureReason ⇒ not resumable (stays parked).
- `resume::mid_punt_drain_refuses_and_routes_to_human`.

## Verification

```bash
cargo test -p aida-cli reconcile resumable resume
cargo build -p aida-cli && cargo fmt --all -- --check && bash tests/test_mcp_doc_consistency.sh
# Manual: start a drain, kill -9 the orchestrator mid-phase-3, then:
aida queue work --resume          # expect: re-enters at reviewer (not implementer), PR not re-opened
aida drain status                 # should detect + suggest the resumable crashed drain
```

## Followups

- `aida drain status` suggests `--resume` when it detects a resumable crashed drain (discoverability).
- `phase_log: Vec<PhaseCheckpoint>` for richer post-mortem diagnostics (timing per phase).
- Auto-resume policy (opt-in config) for unattended overnight drains — only after the explicit `--resume` path is proven.

## Related

- Parent: SPIKE-45 (P1, the durable-execution gap). Sibling shipped: STORY-489 (P2 graph-query, CLI+MCP), STORY-490 (P5 drain legibility).
- Builds on: STORY-301 (drain-state.json), EPIC-28 (shelving/FailureReason), BUG-307 (lease auto-release), TASK-336 (run_uuid corroboration).
- Competitive: `docs/competitive-analysis/2026-05-31-round2-moat-gaps-moves.md` (P1 = "git-canonical AND crash-resumable", the edge no competitor pairs).
