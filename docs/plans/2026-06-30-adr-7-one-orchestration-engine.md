# ADR-7 — One per-spec orchestration engine: enforce + complete

- **Date:** 2026-06-30
- **Specs:** ADR-7 (decision); sub-decisions ADR-9 (guardrail), ADR-10 (autonomy parameter)
- **Status:** In progress
- **Complexity:** Keystone (guided-implement; advisor reviews before merge — no auto-merge)

## Approach

ADR-7 asserts `aida zen` / `aida burndown` / `aida integrate` all call ONE per-spec
orchestration engine (`auto_complete::orchestrate_with_resume`), differing only in
scope + lifetime + autonomy mode. A code map (2026-06-30) found:

- **zen** → self-invokes `queue work --auto-complete` (full phases 1-6) — routes through the engine.
- **integrate** (`handle_queue_integrate`) → self-invokes `--from-pr` (re-enters the engine at phases 3-6) — routes through the engine.
- **queue-work `--auto-complete`** → IS the engine.
- **burndown** → DELIBERATELY reimplements the lifecycle as skill-prose + native subagent fan-out (a FLEET layer above the per-spec layer).

Autonomy: `--no-human` is a typed first-class engine parameter; `--zen` is an env-var
side-channel (`AIDA_ZEN`) re-read in scattered in-process spots.

```
 zen ──┐
       ├─ subprocess `queue work --auto-complete [--from-pr]` ──▶ orchestrate_with_resume()  (THE ENGINE)
 integ ┘                                                              ▲
 queue-work --auto-complete ─────────────────────────────────────────┘
 burndown ──▶ claude -p /aida-burndown ──▶ native subagent fan-out   (FLEET-layer exception, allow-listed)
```

## Decisions (recorded as ADRs)

- **ADR-9:** Reconcile the invariant to the per-spec ENGINE layer; allow-list burndown's
  fan-out as the SINGLE named fleet-layer exception; ship a CI guardrail test (the crux)
  so a new per-spec driver that re-forks the lifecycle trips CI. Substrate-as-bouncer, not prose.
- **ADR-10:** Make autonomy mode a uniform typed engine parameter (`AutonomyMode` carried on
  the driver, resolved once); keep `AIDA_ZEN` env strictly as the cross-process transport to
  spawned children / skill templates, set once from the typed value — not the in-process source of truth.

## Files (build order)

1. `aida-cli/src/integrate.rs` — extract the inline integrate drive argv into a pure
   `drive_args(id)` mirroring `zen_drive::drive_args`; unit-test that it routes through the engine.
2. `aida-cli/src/main.rs` (`handle_queue_integrate`) — call the new `integrate::drive_args`.
3. NEW `aida-cli/src/orchestration_routing.rs` — the routing registry + guardrail:
   a `PerSpecDriver` enum, an `EngineRouting` classification per driver, and tests asserting
   (a) zen/integrate/queue-work route through the engine, (b) burndown is the lone allow-listed
   exception, (c) a completeness assertion forcing the table to be updated for any new driver.
4. `aida-cli/src/auto_complete.rs` + `main.rs` — carry `AutonomyMode` on `RealPhaseDriver`;
   resolve once; replace scattered in-process `AIDA_ZEN` reads with the typed field; keep the
   env-set as child transport.
5. NEW `docs/architecture/one-orchestration-engine.md` — the invariant, the burndown exception
   rationale, the autonomy-parameter model.

## Critical files

- `aida-cli/src/auto_complete.rs::orchestrate_with_resume` — the engine; do not change its phase semantics.
- `aida-cli/src/main.rs::run_auto_complete` (~133140) + `RealPhaseDriver` (~138125) — autonomy threading.
- `aida-cli/src/zen_drive.rs::drive_args` — the mirror for the integrate extraction + the routing pattern.

## Reusable helpers

- `zen_drive::drive_args` (the pure argv assembler pattern to mirror for integrate).
- `resolve_autonomy_mode` (main.rs ~140572) → `AutonomyMode::{Default,Zen,NoHuman}` (already exists; carry it).
- `NoHumanMode` / `EscalateMode` on the driver (the existing typed-autonomy precedent).

## Risks + gotchas

- `main.rs` is hot (in-flight CLI work) — rebase before PR; keep the autonomy change surgical.
- `AIDA_ZEN` MUST keep propagating to child processes / skill templates (they auto-resolve
  `kind:confirmation` prompts from it) — the env var stays; only the in-process re-reads move to the typed field.
- The guardrail must be honest about what it can/can't catch (it gates known + new registry entries; it is not an AST scan) — say so in its doc-comment.
- Do NOT refactor burndown (operator decision) — only allow-list + document it.

## Tests

- `integrate::drive_args` routes-through-engine unit tests (mirror the zen `drive_args_*` tests).
- `orchestration_routing` guardrail tests: engine-routed set + sole-exception + completeness.
- Autonomy: a test that the typed `AutonomyMode` carried on the driver matches the resolved CLI flags.

## Verification

```
cargo build -p aida-cli
cargo fmt --all -- --check
cargo clippy -p aida-cli -- -D clippy::correctness
env -u AIDA_SESSION_ROLE cargo test -p aida-cli
```

## Followups

- (Possible) extend the guardrail toward a stronger structural/AST check if re-forks recur.

## Related

- ADR-7, ADR-9, ADR-10; TASK-1049 (zen reviewer phase, completed); STORY-721 (zen); STORY-520 (integrate loop).
