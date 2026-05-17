# Plan: TASK-285 — `aida queue work --batch NAME --auto-complete` (autonomous batch drain)

Date: 2026-05-16
Specs: TASK-285
Status: Completed
Complexity: ~260 prod LOC, ~140 test LOC, 1 commit, risk low

<!--
  Composes TASK-229's batch head-pickup with STORY-246's --auto-complete
  orchestrator. The natural mental-model workflow: scope to a batch, drive
  each member's full lifecycle, advance, repeat until empty.
-->

## Approach

`aida queue work --batch NAME --auto-complete` was rejected by a clap
`conflicts_with` rule even though the composition maps to a real workflow —
drain a whole batch autonomously, one PR per member. Drop `batch` from
`--auto-complete`'s conflict set, and add a batch-drain loop that re-resolves
the `batch:NAME` head, runs that spec's full `--auto-complete` lifecycle,
advances to the new head, and repeats until the batch is empty, a `--max N`
cap is hit, or a phase fails. The drain *sequencing* is factored behind a
`BatchDriver` trait — the same shape as STORY-246's `PhaseDriver` — so the
loop is unit-tested against a mock, no subprocesses spawned. A phase failure
stops the drain at that spec with the rest of the batch left queued for retry.

### Diagram

```
  --batch NAME --auto-complete
        │
        ▼
   ┌─► next_head ──(none)──► Drained  ✓ exit 0
   │      │
   │   (--max hit?) ──yes──► MaxReached ✓ exit 0
   │      │ no
   │   run_spec(head) ──fail──► Failed(phase) ✗ exit = phase index
   │      │ ok
   └──────┘  (head completes → leaves queue → loop)
```

## Decisions

- **Loop re-resolves the head each iteration** rather than snapshotting the
  member list. **Rationale**: a completed spec leaves the queue, so re-resolve
  is the natural head-advance and tolerates concurrent queue edits. A
  non-advancing-queue guard (a shipped spec resurfacing as head) breaks a
  would-be infinite loop.
- **Factor sequencing behind a `BatchDriver` trait.** **Rationale**: mirrors
  the existing `orchestrate` + `PhaseDriver` pattern; lets the acceptance
  tests (3 green / phase-1 failure on item 2) run without spawning Claude.
- **`handle_auto_complete` split into a non-exiting `run_auto_complete`.**
  **Rationale**: the batch loop must chain runs; the old `-> !` could only end
  the process. `handle_auto_complete` is now a thin exiting wrapper.
- **`--max` requires `--auto-complete` (clap) + `--batch` (handler).**
  **Rationale**: `requires` is single-valued and the `batch:NAME` positional
  bypasses clap, so the batch half is enforced in the handler — same pattern
  as TASK-270's `--type` conflict re-check.
- **Empty batch exits 1.** **Rationale**: matches the plain `--batch`
  head-pickup path; a named batch with nothing queued is a user error worth a
  non-zero code even though `drain_batch` calls it `Drained`.

## Files (in build-order)

### `aida-cli/src/auto_complete.rs` — drain sequencing + tests

- `AutoCompleteVariant::describe`: `fn` → `pub(crate) fn` (batch header reuse).
- `enum BatchDrainOutcome`, `struct BatchDrainResult`: new result types.
- `trait BatchDriver`: `next_head` + `run_spec`.
- `fn drain_batch`: the pure loop (head advance, `--max`, fail-stops, stall guard).
- `mod tests`: `MockBatchDriver` + 7 `drain_batch_*` tests.

### `aida-cli/src/cli.rs` — flag surface

- `Work.auto_complete`: drop `"batch"` from `conflicts_with_all`.
- `Work.batch` / `Work.auto_complete`: doc-comment composition note.
- `Work.max`: new `Option<usize>`, `requires = "auto_complete"`.

### `aida-cli/src/main.rs` — wiring + real driver

- `QueueCommand::Work`: destructure `max`; route `--batch + --auto-complete`
  to `handle_auto_complete_batch`; reject `--max` without `--batch`.
- `fn run_auto_complete`: non-exiting body of the old `handle_auto_complete`.
- `fn handle_auto_complete`: thin `-> !` wrapper over `run_auto_complete`.
- `fn resolve_batch_members`: extracted from the inline `--batch` block.
- `struct RealBatchDriver` + `impl BatchDriver`: re-resolve head, run spec.
- `fn handle_auto_complete_batch`, `fn emit_batch_drain_summary`: entry +
  closing summary (human + `--json`).

## Critical Files

- `aida-cli/src/auto_complete.rs`
- `aida-cli/src/cli.rs`
- `aida-cli/src/main.rs`
- `CLAUDE.md`

## Reusable helpers (do not reimplement)

- `auto_complete::orchestrate` + `PhaseDriver` — the per-spec lifecycle; the
  batch loop calls into it via `run_auto_complete`, never re-implements phases.
- `resolve_queue_role_filter`, `entry_matches_role_filter`, `is_terminal_status`
  (`aida-cli/src/main.rs`) — the role/terminal filtering `resolve_batch_members` reuses.
- `record_auto_complete_run` (`aida-cli/src/main.rs`) — TASK-266 telemetry;
  fires per spec inside `run_auto_complete`, so each batch member is logged.

## Risks + gotchas

1. **Risk**: a `run_spec` reports success but the spec stays queued → infinite
   loop on the same head. **Mitigation**: stall guard — a shipped spec
   resurfacing as the head returns `Stalled` (exit 1).
2. **Risk**: `--max` silently no-ops on the single-spec path. **Mitigation**:
   handler bails with a concrete "pair it with `--batch NAME`" message.
3. **Risk**: a hard environment error (project root unresolvable) recurs every
   iteration. **Mitigation**: `run_auto_complete` exits the process directly on
   those — they are not per-spec phase failures.

## Tests (named)

- `drain_batch_three_green_ships_all_three` — acceptance: 3-item batch all green.
- `drain_batch_phase1_failure_on_item2_leaves_item3_untouched` — acceptance:
  item 1 shipped, item 2 stopped, item 3 never ran.
- `drain_batch_failure_carries_failed_phase_exit_code` — exit code = phase index.
- `drain_batch_max_caps_the_drain` — `--max` stops early.
- `drain_batch_max_equal_to_size_reports_drained` — `--max` == size → `Drained`.
- `drain_batch_empty_batch_is_a_clean_drain` — empty batch.
- `drain_batch_stall_guard_stops_a_non_advancing_queue` — non-advancing guard.

## Verification

```bash
A=target/debug/aida
# composition accepted (was a clap conflict error):
$A queue work --batch nonexistent-xyz --auto-complete --json   # exit 1, JSON batch-drain line
# variants compose:
$A queue work --batch nonexistent-xyz --auto-complete=through-ci --json
$A queue work --batch nonexistent-xyz --auto-complete=through-merge --json
# guards:
$A queue work --batch foo --max 3                       # clap: --max requires --auto-complete
$A queue work TASK-X --auto-complete --max 3            # handler: --max needs --batch
cargo test -p aida-cli --bin aida auto_complete::       # 49 pass
```

## Followups

- End-to-end batch drain against a live 3-spec batch (real Claude sessions) — the unit tests mock the driver; a smoke run would exercise `RealBatchDriver`.

## Related

- Builds on: STORY-246 (`--auto-complete` orchestrator), TASK-229 (batch tag).
- See also: TASK-249 (`/aida-drain-queue` skill — should wrap this CLI form
  rather than re-implement the loop), TASK-266 (failure telemetry).
