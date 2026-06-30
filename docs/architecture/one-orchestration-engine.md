# One per-spec orchestration engine (ADR-7)

**Status:** accepted (ADR-7, 2026-06-29) · enforcement ADR-9 + ADR-10 (2026-06-30)

## The invariant

There is **ONE per-spec orchestration engine**:
`auto_complete::orchestrate_with_resume` in `aida-cli/src/auto_complete.rs`. It
runs the fixed per-spec lifecycle:

```
implement → CI → review → merge → pull → build
```

as a single linear driver over the `Phase` enum, with the phase work delegated
through the `PhaseDriver` trait (`RealPhaseDriver` in production).

Every per-spec driver reaches a spec's lifecycle through this engine. They
differ **only** in:

- **scope** — a from-scratch spec / the continuous queue / an already-Done PR;
- **lifetime** — one-shot vs continuous;
- **start phase** — `--from-pr` re-enters at the reviewer phase (phases 3-6);
- **autonomy mode** — `--zen` (supervised) vs `--no-human` (headless).

They must **never** differ in the per-spec lifecycle itself. A new lifecycle
phase is added **once** to the shared engine and every entry point inherits it
— which is why `aida zen` got an independent reviewer phase "for free" once it
was pointed at the engine (TASK-1049).

### Why this exists

`aida zen` slice-1 (PR #1231) stopped at implement + PR with **zero review**
(`reviewDecision=NONE`) because it ran an *inlined implementer phase* instead of
reusing the engine. That is the anti-pattern ADR-7 forbids: a per-entry-point
orchestration variant that truncates or re-forks the lifecycle.

## How each entry point routes

| Entry point | Routing | Mechanism |
|---|---|---|
| `aida queue work --auto-complete` | **is the engine** | calls `orchestrate_with_resume` in-process (`run_auto_complete`) |
| `aida zen` | subprocess → engine | `zen_drive::drive_args` → `queue work --auto-complete` (phases 1-6) |
| `aida queue integrate` | subprocess → engine | `integrate::drive_args` → `queue work --auto-complete --from-pr` (phases 3-6) |
| `aida burndown` | **fleet-layer exception** | fans out worktree-isolated implementer subagents (see below) |

## The single allow-listed exception: `aida burndown`

`aida burndown` deliberately does **not** drive each spec through the per-spec
engine. It is a **fleet-layer orchestrator** one level *above* the per-spec
lifecycle: it fans out worktree-isolated implementer subagents via the harness's
native subagent fan-out, then integrates their PRs. Its *individual* specs, if
it routed them, would hit the engine — so it is sanctioned, not non-conforming.

This is a conscious, **single** exception (ADR-9). It is the empirically-working
autonomous drain; forcing it onto the subprocess-per-spec engine path was
rejected as a bad trade (high blast radius against the one piece of autonomy
that reliably works, for literal conformance). Routing **any other** command
outside the engine requires a new ADR.

## Enforcement: substrate-as-bouncer, not prose (ADR-9)

A prose rule did not hold — zen slice-1 re-forked despite the lifecycle being
"obvious". So the invariant is a **CI gate**:
`aida-cli/src/orchestration_routing.rs` is a registry classifying every per-spec
driver, with tests that assert:

- each non-exception driver routes through the engine, tied to the **real**
  `drive_args` helpers (a regression dropping `--auto-complete` trips CI);
- `aida burndown` is the **single** allow-listed fleet-layer exception;
- the registry covers exactly the known driver set — so a new per-spec driver
  must be **consciously classified** (routed through the engine, or added to the
  named-exception list).

What it does **not** do: it is a registry, not an AST scan — a brand-new command
that inlines the lifecycle without being added to the registry is not
auto-detected. The contract is therefore: **a new per-spec driver MUST be
classified in `routing_table()`**. This doc states that rule for humans; the
registry states it for CI.

## Autonomy is a uniform typed value (ADR-10)

The two autonomy axes act at **different layers**, and both resolve into one
typed `AutonomyMode`:

- **`--no-human` (headless)** gates **parent-engine** behavior — headless
  implementer spawn, CI-watch — so it is a first-class typed parameter
  (`NoHumanMode` carried on `RealPhaseDriver`, `EscalateMode` on the engine).
- **`--zen` (supervised)** is a **child-session** signal — skill templates in
  the spawned phase sessions auto-resolve `kind:confirmation` prompts when
  `AIDA_ZEN` is set. The env var is therefore the correct **cross-process
  transport** to those children; it is set once by the dispatch arm.

In-process, both axes resolve **once** into a typed value via
`AutonomyMode::for_auto_complete_run` (pure core `resolve_run`) — the single
source of truth — instead of re-reading `AIDA_ZEN` as a bare env bool in
scattered spots. `--zen` is recovered from the zen-**intent token**
(`AIDA_ZEN_TOKEN`, minted only by the `--zen` dispatch arm), so a leaked
`AIDA_ZEN=1` is never mistaken for a zen run (BUG-237). The engine carries no
`zen` field the way it carries `no_human`, precisely because zen's pauses fire
in the child sessions, not the parent engine.

## Related

- ADR-7 (the decision), ADR-9 (guardrail), ADR-10 (autonomy parameter)
- TASK-1049 (zen reviewer phase), STORY-721 (zen), STORY-520 (integrate loop)
- `aida-cli/src/auto_complete.rs` (engine), `orchestration_routing.rs` (guardrail)
- `docs/plans/2026-06-30-adr-7-one-orchestration-engine.md`
