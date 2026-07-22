# Plan: session reap — tear down finished+merged+exited sessions on substrate state

Date: 2026-07-22
Specs: TASK-1177 (child of FR-284)
Status: Completed
Complexity: ~380 prod LOC, ~180 test LOC, 1 commit, risk medium (destructive path)

## Approach

A scoped implementer session cannot tear down its own worktree (it *is* the live
cwd — `aida session end` correctly refuses), so a headless agent that exits when
its work lands strands a worktree + lease + branch and every spec boundary costs
a human three commands. This slice adds `aida session reap`: a supervisor pass
over every session lease that tears the finished ones down whole. The predicate
is deliberately narrow and entirely substrate-derived — spec is **Done or
Completed**, branch is **merged**, process has **EXITED** — so it handles the
headless case automatically while leaving a still-running interactive session
completely untouched (removing a live agent's cwd is exactly the leak
`session end` refuses). No terminal scraping, no force-kill. The final safety
gate delegates to the very same `classify_agent_worktree` predicate
`aida worktree gc` runs, so dirty / unique-unmerged-commit worktrees are refused
by shared code rather than a parallel copy of the rules.

### Diagram

```
  per lease ──► protected checkout/branch? ──yes──► skip (hard floor)
                    │no
                    ▼
              dirty? locked? ──yes──► skip (never auto-removed)
                    │no
                    ▼
        process EXITED? ──no──► skip  (live session: detect-and-leave,
                    │yes                never terminate; reaped next pass)
                    ▼
        spec Done/Completed? ──no──► skip
                    │yes
                    ▼
        classify_agent_worktree (the shared worktree-gc gate)
                    │Removable
                    ▼
        teardown worktree ──► delete lease + activity + manifest ──► git branch -D
```

## Decisions

- **New `aida session reap` verb rather than a flag on `aida worktree gc`**.
  **Rationale**: the reap is *lease*-driven (it reaps sessions, and must delete
  the lease), while `worktree gc` is *worktree*-scan-driven; and `worktree gc
  --json` has an existing single-document contract that a second report would
  break. The parent spec explicitly allows "a supervisor pass" as the shape.
- **Also wired into the post-merge ship hook** (`aida pr ship` step 6, right
  before the existing worktree GC), `quiet_when_empty`. **Rationale**: that is
  where "no manual step" is actually earned for the autonomous path; a merge is
  precisely the event that makes a session reapable.
- **Process-liveness is proven, never assumed**. `session_process_exited`
  requires all three of: `lease_owner_process_gone == Some(true)` (every
  recorded pid absent), the lease not reading `Live`, and no live agent process
  inside the worktree. `None` (no pid ever recorded) is **not** treated as dead.
  **Rationale**: absence of evidence is not evidence of death — the same floor
  the stale-lease recovery classifier already encodes.
- **Delegate the merge/dirty/unique-commit gate to `classify_agent_worktree`**
  rather than re-deriving it. **Rationale**: "reuse the gc safety checks" is
  literal reuse, not a second copy that can drift.
- **A hard protected floor above the predicate**: a lease whose worktree IS the
  project root, or whose branch is `main`/`master`/`aida-store`/the default
  branch/any `*/aida-store` mirror ref, is skipped before any fact is gathered,
  and the branch guard is re-asserted immediately before `git branch -D`.
  **Rationale**: real leases in this repo point at the main checkout on `main`
  (in-place harness leases) — without the floor the whole checkout would be in
  the blast radius of a future predicate bug.
- **Human report names only near-misses**; sessions whose spec is not finished
  are summarized as a count. **Rationale**: 29 leases of "not finished" noise
  buries the one row that matters.

## Files (in build-order)

### `aida-cli-lib/src/doctor_cmd.rs` — widen the shared gc gate

- `enum AgentWorktreeVerdict`, `struct AgentWorktreeFacts` (+ its four fields),
  `fn classify_agent_worktree`: `pub(crate)` so the reap pass can delegate to
  the same predicate instead of copying it.

### `aida-cli-lib/src/session_reap.rs` (new) — the pass

- `enum ReapVerdict`, `struct ReapFacts`, `fn classify_session_reap`: the pure
  predicate (dirty → locked → liveness → completion → shared gc gate).
- `fn session_process_exited`: the pure three-signal exit derivation.
- `fn branch_is_protected` + `PROTECTED_BRANCHES`: the never-delete floor.
- `fn finished_scopes`: one cached-backend open resolving every lease scope to
  Done/Completed.
- `fn scan_reapable`: fact gathering (git ancestry, forge merged-PR lookup for
  the squash case, liveness probe, lock probe) → `ReapReport`.
- `fn reap_one`: dirty re-check + salvage, shared worktree teardown, lease +
  activity + manifest removal, guarded branch delete.
- `fn run_session_reap` + `struct ReapOptions`: report, confirm, execute.

### `aida-cli-lib/src/cli.rs` — the verb

- `enum SessionCommand`: add `Reap { dry_run, yes, json }`.

### `aida-cli-lib/src/lib.rs` — registration + dispatch

- `mod session_reap;`
- `fn handle_session_command`: dispatch the new arm.

### `aida-cli-lib/src/pr_cmd.rs` — the automatic path

- post-merge block: run the reap pass (quiet on the no-op) before the existing
  merged-worktree GC. Best-effort — never turns a landed PR into a failed ship.

## Critical Files

- `aida-cli-lib/src/session_reap.rs`
- `aida-cli-lib/src/doctor_cmd.rs`
- `aida-cli-lib/src/cli.rs`
- `aida-cli-lib/src/lib.rs`
- `aida-cli-lib/src/pr_cmd.rs`
- `aida-cli-lib/src/tests/task_1177_session_reap_tests.rs`

## Reusable helpers

Nothing new was invented where something shipped already:

- `fn classify_agent_worktree` / `struct AgentWorktreeFacts` — the squash-aware
  merged/dirty/unique-commit safety model.
- `fn lease_owner_process_gone`, `fn lease_state_for`, `fn pid_is_alive`,
  `fn probe_live_claude_sessions` — the shipped liveness detection.
- `fn active_worktree_paths` / `fn worktree_is_active` / `fn worktree_is_locked`
  — the worktree-GC activity and lock probes.
- `fn teardown_worktree_path` — shared teardown (pre-destroy cargo-clean hook +
  worktree-pool deregistration).
- `fn salvage_worktree_patch` — dirty-since-scan salvage.
- `fn aggregate_session_activity_into_roles` — the session-end activity fold.

## Risks + gotchas

- **Destructive path.** Mitigated by: the protected checkout/branch floor, the
  dirty re-check immediately before removal (with salvage patch), delegation to
  the shared gc gate, and default-to-skip ordering.
- **PID recycling** could in principle make a dead pid read alive (never the
  reverse) — the conservative direction.
- **Legacy centralized projects** have no distributed store, so `finished_scopes`
  returns empty and nothing is ever reapable. Deliberate: no completion signal,
  no reap.
- **Non-TTY without `--yes`** reports and stops rather than blocking on a prompt
  nobody can answer.

## Tests (named)

`aida-cli-lib/src/tests/task_1177_session_reap_tests.rs` — pure, no repo/store/
process/worktree touched:

- `finished_merged_exited_session_is_reaped`
- `squash_merged_session_is_reaped_on_the_forge_signal`
- `live_process_is_never_reaped_even_when_finished_and_merged`
- `unfinished_spec_is_not_reaped`
- `dirty_worktree_outranks_every_other_signal`
- `locked_worktree_is_operator_protected`
- `unmerged_branch_is_not_reaped`
- `squash_merged_branch_with_extra_unique_commits_is_kept`
- `dirty_beats_liveness_in_the_reported_reason`
- `exited_requires_proof_of_death`
- `unknown_liveness_is_never_treated_as_exited`
- `a_living_pid_or_a_live_lease_blocks_the_exit_verdict`
- `a_live_process_inside_the_worktree_blocks_the_exit_verdict`
- `protected_branches_are_never_deletable`
- `ordinary_session_branches_are_not_protected`

## Verification

```bash
cargo build
cargo test                      # with AIDA_SESSION_ROLE unset
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D clippy::correctness
aida session reap --dry-run     # reports, touches nothing
aida session reap --json | python3 -c 'import json,sys; json.load(sys.stdin)'
python3 docs/cli/verify-manual.py
```

## Followups

- NOTIFY slice (FR-284): a live interactive session whose spec is Done gets a
  "safe to exit; the worktree reaps when you do" notice over the existing
  mailbox/brief channel. Out of scope here.
- CHAIN slice (FR-284): after a reap, optionally launch the next queued spec
  behind an explicit opt-in. Out of scope here (guided).

## Related

- FR-284 — the parent (ratified design + hard boundaries).
- TASK-878 / TASK-1145 — the merged-agent-worktree GC whose safety model this
  reuses.
- BUG-614 — the conservative worktree GC.
- BUG-777 — the stale-lease recovery floor this borrows its "proof of death"
  rule from.
