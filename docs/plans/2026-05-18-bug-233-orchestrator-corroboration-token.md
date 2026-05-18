# BUG-233 — Orchestrator-child corroboration token

- **Date:** 2026-05-18
- **Specs:** BUG-233
- **Status:** In progress
- **Complexity:** Medium

## Approach

An orchestrator-spawned `aida queue work <SPEC>` child is today indistinguishable
from a standalone one the user typed: same argv, and `AIDA_AUTO_COMPLETE=1` is an
unverifiable bare flag. The child *guesses* its parentage and guesses wrong both
ways (false "stray export leak" warnings; BUG-232-style limbo).

Fix: a **corroboration token**. The orchestrator mints a per-run UUID, writes a
marker file `.aida/orchestrator-runs/<uuid>` recording its own PID, and passes
`AIDA_AUTO_COMPLETE_TOKEN=<uuid>` to every phase child alongside
`AIDA_AUTO_COMPLETE=1`. A child trusts orchestrator-mode **only** when
`AIDA_AUTO_COMPLETE=1` *and* the token names a marker whose PID is alive.

```
orchestrate(BUG-233)
  │  RunMarkerGuard::register()  →  writes .aida/orchestrator-runs/<uuid>  (pid=<orch>)
  │  RealPhaseDriver { run_token: <uuid> }
  ├─ run_implementer:  aida queue work BUG-233   env: AIDA_AUTO_COMPLETE=1 + _TOKEN=<uuid>
  │     └─ child: orchestrator::detect()  →  marker exists + pid alive  →  Orchestrated
  │           └─ claude session: skill runs `aida orchestrator status` → "orchestrated"
  ├─ run_reviewer:     aida queue work PR-N      env: AIDA_AUTO_COMPLETE=1 + _TOKEN=<uuid>
  │  (Drop) RunMarkerGuard removes the marker when orchestrate() returns
  
standalone:  aida queue work BUG-233   (no env)        → detect() → Interactive
leaked var:  AIDA_AUTO_COMPLETE=1, no token / dead pid  → detect() → Uncorroborated → note
```

## Decisions

- **Marker file, not a PID-only env check.** The token must be *verifiable* from
  a child. A marker file under `.aida/orchestrator-runs/` keyed by the UUID, with
  the orchestrator PID inside, is verifiable (file present + PID alive) and
  RAII-cleaned. STORY-301's drain-state file is not built yet, so the dedicated
  marker dir is the home (BUG-233 acceptance allows either).
- **A CLI command (`aida orchestrator status`), not a new propagated env var,
  for the skills.** A second env var would reintroduce the exact bare-var trust
  bug. The command re-runs the full live corroboration each call, so it cannot
  go stale, and `queue work` shares the same `detect()`.
- **Three-state verdict.** `Orchestrated` / `Interactive` / `Uncorroborated`.
  Only `Uncorroborated` (bare var, no live token) prints the informational note;
  it is otherwise treated identically to `Interactive`.
- **No "stray export leak" warning path to remove** — verified absent from code
  and templates (it was the BUG-228 implementer's ad-hoc message, never
  committed). Acceptance item 4 is satisfied by confirmed absence.
- **Pure `classify()` split from `detect()`** so the env/fs-free decision logic
  is unit-tested without spawning processes.

## Files (build order)

1. `aida-cli/src/process_probe.rs` — add `pub fn pid_is_alive(pid: u32) -> bool`.
2. `aida-cli/src/orchestrator.rs` — **new module**: `OrchestratorContext`,
   `UncorroboratedReason`, `classify`, `detect`, `run_is_live`, `RunMarker`,
   `RunMarkerGuard`, `runs_dir`/`marker_path`, env-var consts, note text, tests.
3. `aida-cli/src/cli.rs` — `Command::Orchestrator(OrchestratorCommand)` +
   `OrchestratorCommand::Status { json }`.
4. `aida-cli/src/main.rs` — `mod orchestrator;`; dispatch `Command::Orchestrator`
   before storage init; two `unreachable!` arms; `RealPhaseDriver.run_token`
   field + `.env(TOKEN_ENV, …)` on both child spawns; `run_auto_complete` mints
   the marker guard; `handle_queue_work` prints the `Uncorroborated` note.
5. `aida-core/templates/skills/aida-pickup.md`, `aida-pr.md` — swap the
   `echo "${AIDA_AUTO_COMPLETE:-}"` detection for `aida orchestrator status`.
6. `aida-core/templates/skills/aida-review.md`, `aida-implement.md` — note the
   corroboration model (their path-valued handshake vars already self-verify).
7. `docs/autonomous-drain.md` — document the orchestrator→child env contract.

## Critical Files

- `aida-cli/src/orchestrator.rs` (new) — the whole corroboration model.
- `aida-cli/src/main.rs` — `RealPhaseDriver::run_implementer` / `run_reviewer`
  (token-passing), `run_auto_complete` (marker lifecycle), `handle_queue_work`
  (the note), the `Command` dispatch + the two exhaustive `match` arms.
- `aida-cli/src/cli.rs` — `Command` enum.

## Reusable helpers

- `process_probe.rs` — `sysinfo`-based process enumeration; `exit_signal.rs`'s
  test `pid_is_live` is the exact liveness primitive to promote.
- `exit_signal.rs` — the RAII / sentinel-file pattern (`spawn_and_wait`,
  marker-path helper) is the model for `RunMarkerGuard` + `marker_path`.
- `find_main_worktree_root()` (main.rs) — resolves the shared `.aida/` root from
  any worktree, so orchestrator and child agree on the marker dir.

## Risks + gotchas

- **Sibling-worktree `.aida/`** — the child claude runs in a sibling worktree.
  `detect()` must resolve `find_main_worktree_root()`, not cwd's `.aida/`, so it
  reads the *orchestrator's* marker dir.
- **PID reuse** — a dead orchestrator's PID could be recycled. Acceptable: the
  window is tiny, and a false `Orchestrated` only restores today's behavior.
  The marker is RAII-removed on clean exit, shrinking the window further.
- **Path traversal** — reject non-UUID tokens before joining into `marker_path`
  so a crafted `AIDA_AUTO_COMPLETE_TOKEN` can't escape the dir.
- **Batch drains** — `run_auto_complete` is called per batch member; one marker
  per member is correct (each is a distinct run).

## Tests

In `orchestrator.rs`:
- `classify_no_var_is_interactive`
- `classify_bare_var_no_token_is_uncorroborated_notoken`
- `classify_var_with_token_live_is_orchestrated`
- `classify_var_with_token_dead_is_uncorroborated_dead`
- `classify_empty_var_treated_as_unset`
- `run_marker_round_trips`
- `run_marker_guard_drop_removes_file`
- `run_is_live_false_for_missing_marker` / `…_for_dead_pid` / `…_for_bad_token`
- `note_text_differs_by_reason`

In `process_probe.rs`:
- `pid_is_alive_true_for_self` / `pid_is_alive_false_for_unused_pid`

## Verification

```bash
cargo test -p aida-cli orchestrator:: process_probe::
cargo build -p aida-cli
# standalone → no note, status=interactive
./target/debug/aida orchestrator status
# bare var, no token → informational note + status=interactive
AIDA_AUTO_COMPLETE=1 ./target/debug/aida orchestrator status
# var + token naming a live pid → orchestrated
mkdir -p .aida/orchestrator-runs && printf 'pid=%s\nspec=X\n' $$ > .aida/orchestrator-runs/test-uuid
AIDA_AUTO_COMPLETE=1 AIDA_AUTO_COMPLETE_TOKEN=$(uuidgen) ./target/debug/aida orchestrator status  # dead/bad → interactive
cargo fmt --all -- --check
```

## Followups

- Audit whether `AIDA_ZEN` has the same can't-verify-provenance weakness
  (BUG-233 "Composes with" — STORY-287).
- When STORY-301's drain-state file lands, fold the run-UUID into it and retire
  the standalone `.aida/orchestrator-runs/` marker dir.

## Related

- STORY-301 (drain-state file), BUG-232 (`--zen` limbo), STORY-287 (`--zen`),
  TASK-329 (exit sentinel — RAII/marker-file precedent).
