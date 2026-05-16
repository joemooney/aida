# `aida queue work --auto-complete` — full lifecycle orchestrator

- **Date:** 2026-05-16
- **Specs:** STORY-246
- **Status:** Complete
- **Complexity:** High

## Approach

Collapse the implementer → CI → reviewer → merge → pull → build cycle into one
command. The orchestrator is a long-lived parent process that **spawns and
waits** on each phase — fundamentally different from today's `aida queue work`,
which `exec()`s `claude` and replaces the aida process. So `--auto-complete`
gets its own code path (`auto_complete.rs`), dispatched from the `queue work`
handler when the flag is set.

```
aida queue work TASK-247 --auto-complete
        │
        ├─ preflight: ensure TASK-247 is queued for implementer
        │
   P1 ──┤ subprocess `aida queue work TASK-247 --session-id <uuid>` (exec's claude)
        │   → wait for claude exit; discover the new lease; detect open PR
        │
   P2 ──┤ wait CI on the branch (probe → watch/wait → terminal)
        │   green → `aida session end <lease> --yes --skip-ci` (auto-queues Review PR-N)
        │
   P3 ──┤ subprocess `aida queue work PR-N --session-id <uuid2> --no-pull`
        │   with AIDA_REVIEW_VERDICT_FILE set → reviewer claude writes verdict JSON
        │   → read verdict; end reviewer session
        │
   P4 ──┤ gh pr merge N --squash --delete-branch
   P5 ──┤ aida pull   (auto-bumps Done→Completed)
   P6 ──┤ cargo build --release
        │
        ✓ exit 0
```

The orchestrator process stays in the main project root for its entire life —
it never `cd`s into a worktree, so `session end` removing worktrees is safe.

Testability: the orchestration sequencing is isolated behind a `PhaseDriver`
trait. `RealPhaseDriver` (in `main.rs`) does the subprocess/CI/lease work;
`MockPhaseDriver` (in tests) returns canned per-phase results. `orchestrate()`
is the pure-ish core unit-tested against the mock.

## Decisions

- **Spawn-and-wait, not exec.** `--auto-complete` cannot decorate the existing
  exec path; it is a sibling orchestrator.
- **Phases 1 & 3 subprocess `aida queue work`** (`current_exe`) so all existing
  worktree/lease/role/skill-routing logic is reused unchanged. The orchestrator
  mints the `--session-id` UUID up front and discovers the lease by
  snapshot-diffing `.aida/sessions/` across the subprocess.
- **CI wait is orchestrator-owned**, not delegated to `session end --wait-ci`:
  `session end` exits 0 even on red CI, so the orchestrator must probe CI
  itself (`probe_ci_state_for_branch` + `wait/watch_ci_terminal`) to detect red
  and emit exit 2. Green → `session end --yes --skip-ci`.
- **Verdict handshake via one env var.** The orchestrator sets
  `AIDA_REVIEW_VERDICT_FILE=<abs path>`; `/aida-review` writes the verdict JSON
  there and, when the var is set, STOPS before merge (orchestrator owns P4-P6).
  Unset → today's behavior. The var is both the trigger and the path.
- **Verdict file lives in the main root's `.aida/review-verdicts/PR-N.json`** —
  gitignored by the `.aida/*` deny-by-default rule, no `.gitignore` change.
- **Preflight auto-queue.** If the spec isn't queued, the orchestrator queues it
  (`aida queue add --for implementer`) so a fresh `aida add` → `--auto-complete`
  works without a manual queue step.
- **Process exit codes 0-6** via `std::process::exit` from the dispatch site.

## Files (build order)

1. `aida-cli/src/auto_complete.rs` *(new)* — `AutoCompleteVariant`, `Verdict`,
   `Phase`, `PhaseFailure`, `HintContext`, `PhaseDriver` trait, `orchestrate()`,
   `recovery_hint()`, `phase_event()` JSON, `#[cfg(test)] MockPhaseDriver` + tests.
2. `aida-cli/src/main.rs` — `mod auto_complete;`; `RealPhaseDriver` impl of the
   trait; `queue work` handler dispatch into the orchestrator when
   `--auto-complete` is set (preflight queue check + `process::exit`).
3. `aida-cli/src/cli.rs` — add `auto_complete: Option<String>` (bare → `full`)
   and `json: bool` to the `QueueCommand::Work` struct.
4. `aida-core/templates/skills/aida-review.md` — auto-complete branch after
   step 7: write `$AIDA_REVIEW_VERDICT_FILE` and stop before merge.

## Critical Files

- `aida-cli/src/auto_complete.rs` — the orchestrator core + trait + tests.
- `aida-cli/src/main.rs` — `fn handle_queue_work` (~41094) is the sibling the
  dispatch branches away from; `probe_ci_state_for_branch` / `wait_for_ci_terminal`
  / `watch_ci_terminal` / `detect_open_pr_for_branch` (~12540-13880) are reused
  by `RealPhaseDriver`; `struct SessionLease` (~10165) defines the lease TOML
  fields the orchestrator peeks (`id`, `branch`, `worktree_path`, `scope`).
- `aida-cli/src/cli.rs` — `QueueCommand::Work` struct (~2362).
- `aida-core/templates/skills/aida-review.md` — step 7/8 boundary (~252).

## Reusable helpers

- `probe_ci_state_for_branch`, `parse_ci_probe`, `decide_ci_action`,
  `wait_for_ci_terminal`, `watch_ci_terminal` — CI polling, reused as-is.
- `detect_open_pr_for_branch` — PR existence + number after the implementer phase.
- `find_main_worktree_root` — orchestrator's stable cwd / lease dir.
- `resolve_gh_binary` — `gh` path for the merge phase.

## Risks + gotchas

- **Lease discovery race.** Snapshot-diff `.aida/sessions/` assumes exactly one
  new lease per phase. 0 or >1 → hard error with a clear message (edge case).
- **Dirty implementer worktree** blocks `session end`; surfaces as exit 2 with a
  commit-or-discard hint (no `--force` — never auto-discard work).
- **No PR after implementer phase** (human forgot `/aida-pr`) → exit 1, detected
  by `detect_open_pr_for_branch` inside the implementer phase.
- **Auto-bump only fires on the default branch** — if the user runs
  `--auto-complete` from a feature branch, P5 pull still succeeds but won't bump;
  acceptable.

## Tests (named, in `auto_complete.rs`)

- `orchestrate_full_pipeline_runs_all_six_phases` — mock all-success → exit 0,
  call log == [P1..P6].
- `orchestrate_through_ci_stops_after_phase_2` — variant gating.
- `orchestrate_through_merge_stops_after_phase_4`, `orchestrate_skip_build_stops_after_phase_5`.
- `failure_injection_*` — each phase fails → exit == phase index (1-6).
- `ci_red_stops_at_phase_2` — `finish_ci` Err → exit 2, reviewer not called.
- `reviewer_rejected_stops_at_phase_3` — verdict ≠ Approved → exit 3.
- `recovery_hint_*` — one per exit code, asserts the right command is named.
- `phase_event_json_shape`, `variant_parse_*`, `verdict_parse_*`.

## Verification

```bash
cargo build -p aida-cli
cargo test -p aida-cli auto_complete
cargo fmt --all -- --check
cargo clippy -p aida-cli -- -D warnings
aida queue work --help | grep -A2 auto-complete
```

## Followups

- Auto-fixup loop: on exit 3, re-enter the implementer with review feedback and
  iterate until approved (own STORY — higher risk).
- Fully-autonomous variant: no human in the reviewer session (verdict from
  `/goal`-style completion conditions).
- Desktop/toast notifications on phase transitions.
- TUI mission-control `Enter` → invoke `--auto-complete` directly (STORY-244).

## Related

- TASK-249 (`/aida-drain-queue` becomes a loop over `--auto-complete`).
- STORY-244 (TUI launcher). TASK-233 (`session end --watch-ci`, reused).
- EPIC-23 (Session orchestration & autonomy — parent).
