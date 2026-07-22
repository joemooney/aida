# Drain integration-wait architecture — launcher-owned CI/merge waits

**Date:** 2026-07-21
**Specs:** TASK-1169 (follow-up to BUG-755); decisions ADR-21, ADR-22
**Status:** implemented
**Complexity:** medium — drain-loop surgery, guided/keyboard (not a headless drain of itself)

## Approach

BUG-755 (#1517) shipped a partial fix: when a headless drain session exited
with residual work, the launcher relaunched a bounded continuation turn. That
is recovery, not architecture — the load-bearing CI/merge wait still lived
inside an agent turn, where it is structurally unable to hold:

- a headless `claude -p` reaps its background tasks at turn end, so a promised
  background watch dies with the session (the observed BUG-755 failure), and
- a foreground tool call is capped at ~10 minutes, so a 30-minute
  cross-platform CI run cannot be covered by one foreground call either.

So the wait moved out of the agent entirely, per the operator's ratified
three-part design.

```
aida burndown run  (launcher — long-lived, holds the drain lock)
  │
  ├─ spawn headless `claude -p` /aida-burndown          ← implements the wave
  │     env: AIDA_BURNDOWN_LOCK_HELD, CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS  (ADR-22)
  │
  ├─ probe residual (BUG-755)  → open wave PRs + unstarted specs
  │
  ├─ INTEGRATE IN RUST (ADR-21, new)
  │     per PR: supervision gate → wait_for_ci_terminal → review/mergeable probe
  │             → merge + `aida pull` | hold | park NeedsAttention + finding
  │
  ├─ re-probe residual (post-integration world)
  │
  └─ relaunch an agent turn ONLY for what needs an LLM (unstarted specs)
```

## Decisions

- **ADR-21 — integration waits are launcher-owned Rust.** Rejected: skill-level
  chunked foreground polls (stays prompt-enforced; an LLM can still choose a
  background task) and the both-paths hybrid (two code paths merging the same
  PR needs an idempotent merge guard for no added safety).
- **ADR-22 — bounded, launcher-set ceiling + punt-not-abort.** 45-minute
  default, `[burndown] bg_wait_ceiling_ms` / `AIDA_BURNDOWN_BG_WAIT_CEILING_MS`,
  clamped to a non-zero floor so the retired `=0` stopgap is unreachable.
  Applied at *every* headless spawn site, not just burndown's, so the
  inconsistency class is closed rather than this one instance. On expiry: park
  `NeedsAttention` + file a finding, PR left open, drain continues, exit 2.

## Files (build order)

1. `aida-cli-lib/src/burndown.rs` — the pure layer: `resolve_bg_wait_ceiling_ms`,
   `WavePrFacts` / `WavePrAction` / `wave_pr_action`, `IntegrationOutcome` +
   `render_integration_report`; `ResidualPr` gained `branch` (the CI wait's key)
   and `WaveSpecFacts` gained `supervised`.
2. `aida-cli-lib/src/lib.rs` — the impure boundary: `resolved_bg_wait_ceiling_ms`
   / `bg_wait_ceiling_env`, `integrate_wave_prs` and its helpers
   (`wave_pr_supervision_label`, `wave_pr_review_facts`,
   `local_verdict_blocks_merge`, `merge_wave_pr`, `park_wave_pr`,
   `file_integration_wait_finding`); wired into `handle_burndown_run`'s loop.
3. `aida-cli-lib/src/session.rs` — the ceiling on the central headless
   spawn/exec/resume paths.
4. `aida-core/templates/skills/aida-burndown.md`, `docs/autonomous-drain.md`,
   `docs/environment-variables.md`.

## Critical files

- `handle_burndown_run` (`lib.rs`) — the resume loop; the integration leg runs
  between the residual probe and `burndown::drain_followup`.
- `wait_for_ci_terminal` (`lib.rs`) — reused as-is; its `NoSignal` message is
  how a bound (idle / absolute) is distinguished from a plain no-CI-signal.

## Reusable helpers (do not reimplement)

- `integrate::classify_integration_action` — the RequestChanges / red-CI /
  conflict pre-merge gate. `wave_pr_action` delegates to it so the launcher and
  `aida integrate` share one gate rather than holding a second opinion.
- `ci_idle_timeout::{ci_wait_verdict, ci_progress_fingerprint}` (TASK-968) — the
  re-arming idle timer + absolute ceiling, via `wait_for_ci_terminal`.
- `pr_ship::merge_requires_supervision` (BUG-727), `forge::merge_change`,
  `shelve_spec_on_failure` (EPIC-28), `read_verdict_file`.

## Risks + gotchas

- **A supervision-held PR could wedge the relaunch loop.** `match_wave_prs` did
  not know about `execution_mode`, so a held PR would read as stranded and burn
  the resume budget on turns that every merge gate refuses. Fixed by the
  `supervised` fact.
- **`--delete-branch` is deliberately off** on the launcher's merge: the
  implementer worktree still holds the branch (BUG-758); worktree pruning owns
  branch deletion.
- **Degrade direction matters.** An unreadable forge probe yields
  `(no RequestChanges, Unknown mergeable)` — "couldn't tell" must never
  manufacture a reviewer objection, and the forge itself refuses a dirty merge.

## Tests

- `aida-cli-lib/src/tests/task1169_integration_wait_tests.rs` (19) — ceiling
  resolution incl. the zero-clamp; the gate's merge/hold/park truth table;
  supervised PRs excluded from residual; report rendering.
- `aida-cli-lib/tests/task1169_launcher_sets_ceiling_guardrail.rs` (2) — every
  headless spawn site sets the ceiling; nothing hard-codes wait-forever. This
  one found three spawn sites the first pass missed.

## Verification

```bash
env -u AIDA_SESSION_ROLE cargo test -p aida-cli-lib task1169
env -u AIDA_SESSION_ROLE cargo test -p aida-cli-lib --test task1169_launcher_sets_ceiling_guardrail
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D clippy::correctness
```

## Followups

- Retire the operator-side `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS=0` from any
  daemonized launch wrapper — the launcher now sets a bounded value itself, and
  an ambient `=0` would override it.
- Consider an `aida doctor` check that flags an ambient `=0` in the
  environment as the retired stopgap.
- `aida integrate` is still a read-only view; the Rust integration leg built
  here is the natural engine for making it act.

## Related

- BUG-755 (#1517) — the partial fix this completes.
- BUG-727 — supervised merge holds. TASK-968 — the CI-wait timers.
- TASK-836 — the pre-merge gate reused here.
