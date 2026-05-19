# Plan: STORY-301 — `aida drain status`

Date: 2026-05-18
Specs: STORY-301
Status: Completed
Complexity: ~380 prod LOC, ~210 test LOC, 1 commit, risk low

## Approach

An `aida queue work --auto-complete` orchestrator is invisible from inside the
Claude session it spawns — a user mid-drain cannot tell what launched it,
whether it is a single-spec or a batch drain, how far through it is, or what
happens when they exit. The orchestrator *knows* all of it. This story makes
the orchestrator write it down: a new `aida-cli/src/drain_state.rs` module
defines `DrainState`/`DrainMember` serialized to `.aida/drain-state.json`. The
three orchestrator entry points (`run_auto_complete` for single,
`handle_auto_complete_batch`, `handle_auto_complete_next_n`) write the file at
drain start and clear it on a clean exit; `RealPhaseDriver` updates the current
phase per transition, and `run_auto_complete` stamps each member's terminal
outcome. The file's *presence* means a live (or crashed) drain. `aida drain
status` reads it, corroborates the recorded `orchestrator_pid` against a
liveness probe, and prints a human summary — or `No drain in progress.` (exit
0). A file whose PID is dead is *stale*; `--clear` removes it. The
`/aida-pickup` banner surfaces `aida drain status` when orchestrated, and
`aida session leases` annotates the lease the orchestrator is currently
driving.

### Diagram

```
  drain start ──► .aida/drain-state.json written (orchestrator_pid, members)
       │                       ▲
   per phase ──► set_phase ─────┤  read by `aida drain status`
   per member ─► set_member_outcome
       │                       │
  clean exit ──► clear()       PID alive? ─► Active   PID dead? ─► Stale
  crash ──────► file survives (stale, detectable)
```

## Decisions

- **One drain-state file, not a directory of per-PID files**. **Rationale**:
  STORY-301 scopes out multi-orchestrator support; a single file assumes one
  drain. A second drain clobbers the first's file — acceptable per the spec's
  "Out of scope".
- **`run_auto_complete` gains an `owns_drain_state` bool**. **Rationale**: it
  is called both standalone (single drain — owns + clears the file) and per
  batch member (the batch handler owns the file; the member only announces
  `current`). An explicit flag beats sniffing for a pre-existing file, which a
  stale crashed file would confuse.
- **`RealPhaseDriver` writes phase transitions, not `orchestrate`**.
  **Rationale**: keeps the heavily unit-tested pure `orchestrate` /
  `PhaseDriver` trait untouched — six one-line `mark_drain_phase` calls in the
  real driver's phase methods, best-effort and a no-op when no file exists.
- **`render_human` is plain text, no ANSI color**. **Rationale**: it is a pure,
  substring-tested function; color codes would break the assertions and the
  block reads fine uncolored.
- **`--clear` refuses a live drain**. **Rationale**: a live orchestrator
  removes the file itself on exit; clearing from under it would hide work in
  progress.

## Files (in build-order)

### `aida-cli/src/drain_state.rs` — new module

- `struct DrainState` / `DrainMember`: serde structs for `.aida/drain-state.json`.
- `new_single` / `new_batch` / `new_next_n`: per-mode constructors.
- `write` (via `aida_core::write_atomic`) / `read` / `clear`.
- `set_current` / `set_phase` / `set_member_outcome`: read-modify-write updates.
- `probe` → `DrainStatus::{None,Active,Stale}` via `process_probe::pid_is_alive`.
- `render_human` / `render_json`: pure rendering.

### `aida-cli/src/main.rs` — orchestrator wiring + command

- `mod drain_state;`.
- `run_auto_complete`: new `owns_drain_state` param; write/clear single-drain file.
- `RealPhaseDriver::mark_drain_phase`: called at the top of all six phase methods.
- `handle_auto_complete_batch` / `handle_auto_complete_next_n`: write + clear.
- `handle_drain_command` / `drain_clear`: the `aida drain status` handler.
- `session_leases`: annotate the lease matching the drain's `current` member.

### `aida-cli/src/cli.rs` — CLI surface

- `enum DrainCommand { Status { json, clear } }`; `Command::Drain(DrainCommand)`.

### `aida-core/templates/skills/aida-pickup.md` — banner

- `## Drain position` executable section + a Step-1 prose paragraph.

## Critical Files

- `aida-cli/src/drain_state.rs`
- `aida-cli/src/main.rs`
- `aida-cli/src/cli.rs`
- `aida-core/templates/skills/aida-pickup.md`

## Reusable helpers (do not reimplement)

- `aida_core::write_atomic` — tear-proof writes (TASK-331).
- `process_probe::pid_is_alive` — orchestrator-PID liveness probe.
- `orchestrator::RunMarkerGuard` — the corroboration-marker pattern this mirrors.
- `find_main_worktree_root` — resolves the shared `.aida/` from any worktree.
- `resolve_batch_members` / `auto_complete_head_candidates` — seed the member list.

## Risks + gotchas

1. **Risk**: a crashed orchestrator leaves a stale file forever.
   **Mitigation**: `probe` corroborates the PID; a dead PID renders as stale
   with a `--clear` hint.
2. **Risk**: a torn write observed by a concurrent `aida drain status`.
   **Mitigation**: every write goes through `write_atomic` (rename(2)).
3. **Risk**: a phase-failed single drain clears the file. **Mitigation**:
   intended — a phase failure *is* a clean orchestrator exit; the file means a
   *live* drain, not a *successful* one.

## Tests (named)

- `single_spec_state_round_trips` / `batch_state_round_trips` — write+read.
- `probe_none_when_no_file` / `probe_active_for_live_pid` / `probe_stale_for_dead_pid`.
- `set_phase_updates_current_and_member` / `set_member_outcome_marks_terminal_state`.
- `render_human_single_shows_command_and_phase` / `render_human_batch_shows_member_progress` / `render_human_stale_reports_crash_and_clear_hint`.
- `render_json_carries_status_word` / `on_drain_complete_predicts_per_mode`.
- `clear_removes_file_and_is_idempotent` / `updates_are_noops_without_a_file`.

## Verification

```bash
aida drain status            # expect: No drain in progress.  (exit 0)
aida drain status --json     # expect: {"status":"none"}
# write a fixture drain-state.json with a dead PID:
aida drain status            # expect: ⚠ Stale drain-state file ... --clear
aida drain status --clear    # expect: ✓ removed the stale drain-state file
```

## Followups

- `aida drain status` could show PR merge-state (`gh pr view`) per member.
- A `drain:NAME N/M` statusline segment sourced from `drain-state.json` (TASK-306).
- `aida session leases --json` could include the drain cross-reference.

## Related

- Composes with: TASK-294 (worker.cmd write-side), TASK-306 (statusline), BUG-233
  (orchestrator corroboration), BUG-229 (queue-list pipeline states), TASK-331
  (atomic writes).
- Batch: `batch:autonomy-modes`.
