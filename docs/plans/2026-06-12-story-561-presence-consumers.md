# Plan: STORY-561 — wire the operator-presence CONSUMERS (home/away keys the autonomy ladder)

Date: 2026-06-12
Specs: STORY-561 (presence consumers; primitive shipped as TASK-756). Composes with SPIKE-57 (`aida human` vector), STORY-306 (escalation cascade), STORY-555 (`aida questions` inbox), STORY-545 (`burndown run`).
Status: In progress
Complexity: medium; risk medium (touches the `queue work --auto-complete` autonomy-mode resolution). Keyboard-supervised (`needs-supervised-build`).

<!-- trace:STORY-561 | ai:claude — symbol refs over line refs (TASK-92) -->

## Approach

The presence PRIMITIVE already shipped (TASK-756): `presence::current_presence(now) -> Presence::{Home,Away}`, a timestamped `~/.aida/presence.toml` with TTL + interactive-TTY auto-flip, surfaced in `aida status` + the statusline. **D0 of the binding design: REUSE it; do NOT rebuild the primitive.** This build wires only the CONSUMERS (acceptance #3) under a new tunable `[presence]` config block (acceptance #4/#5: advisory, explicit flags win, integrity gates always apply).

The consumers split into two clean groups by risk:

1. **Drain-mode default (consumers a + b)** — the autonomy-ladder site. The real `--no-human` / escalate axis lives on `aida queue work --auto-complete` (NOT `burndown run`, which is already headless-only). When presence is **away** and no explicit `--no-human`/escalate flag is given, presence fills the default per `[presence] away_drain`. A pure resolver makes "explicit flag always wins" unit-testable.

2. **Status surfacing (consumers c + d home-side)** — display-only, zero execution risk. When **home**, `aida status` surfaces the decision inbox (`aida questions`) count and the keystone set as "ready for `--zen`" (per `[presence] home_offer`). When **away**, these accumulate quietly.

**Consumer-d away-side safety floor is already structural:** `needs-supervised-build` / `needs-human` are parking tags (`pickability::parking_tag`), so keystone work is *already* excluded from the autonomous ready set unconditionally — presence adds the home-side surfacing, not a new away-side filter (re-filtering would double-count).

### `[presence]` policy block (binding design, operator-resolved 2026-06-12)

```
[presence]
away_ttl   = "8h"                 # TASK-756 (already read)
consumers  = "on"   | "off"       # P0 master switch (default on)
away_drain = "headless-both"      # P1 — default; | "headless-escalate-defaults" | "headless-park"
home_offer = "surface"            # P2 — default; | "dont-block"
```

`away_drain` → (no_human, escalate) mapping (operator-confirmed "by behavior", 2026-06-12):
- `headless-both` (default)         → `--no-human=both` + escalate **defaults**
- `headless-escalate-defaults`      → `--no-human=both` + escalate **defaults** (explicit spelling)
- `headless-park`                   → `--no-human=both` + escalate **blocks** (punts park for triage)

## Files (build-order)

1. **`aida-cli/src/presence.rs`** — add the `[presence]` policy: `ConsumersMode` / `AwayDrain` / `HomeOffer` enums (+ `from_config_str` + defaults), `PresenceConfig` + `read_presence_config(config_path)`, and the pure `resolve_drain_mode(...) -> DrainModeResolution` resolver (explicit flags win). Unit tests for parse + resolver + integrity-gate-respect.
2. **`aida-cli/src/main.rs`** — consumer (a)+(b): at the `queue work --auto-complete` resolution (the `no_human_mode` / `escalate_mode` block, near the `EscalateMode::from_flags` call), call `resolve_drain_mode` with `current_presence` + config; apply the effective mode; print a one-line advisory banner when presence supplied the default; keep the existing kickoff gate + escalate validation unchanged.
3. **`aida-cli/src/main.rs`** — consumer (c)+(d) home-side: a presence-gated block in `handle_status_command_distributed` (after `print_status_presence_line`) — home → surface decisions count (`collect_decision_requests`) + keystone-ready set (`needs-supervised-build`-tagged open specs) as "ready for `--zen`"; away → quiet.
4. **`aida-cli/src/cli.rs`** — document the `[presence]` knobs + defaults in the `Away` / `Home` / `Presence` command doc-comments (the `--help` documentation surface, per the spec).

## Critical Files
- `aida-cli/src/presence.rs` — `current_presence`, `away_ttl_secs`/`read_away_ttl_from_config` (the config-read pattern to mirror), the new policy + resolver.
- `aida-cli/src/auto_complete.rs` — `NoHumanMode` (`ReviewerOnly`/`Both`, `parse`), `EscalateMode` (`Blocks`/`Defaults`, `from_flags`).
- `aida-cli/src/main.rs` — the auto-complete `no_human_mode`/`escalate_mode` resolution; `no_human_kickoff_gate` (already non-TTY-safe); `handle_status_command_distributed`; `collect_decision_requests`.
- `aida-core/src/pickability.rs` / `aida-cli/src/burndown.rs` — `parking_tag` (keystone already parked → away-side floor), `OpenBucket::needs_human`.

## Reusable helpers (do not reimplement)
- `presence::read_away_ttl_from_config` — the exact `[presence]`-section toml-read pattern; mirror it.
- `auto_complete::NoHumanMode::parse` / `EscalateMode::from_flags` — reuse to turn the resolver's slug into typed modes.
- `collect_decision_requests` — the `aida questions` pending count (consumer c).
- `burndown::parking_tag` — proof the keystone away-side floor already holds.

## Risks + gotchas
1. **Auto-headless from a file.** `--no-human=both` is normally explicit + loud. Mitigation: presence is only ever `away` in a **non-TTY** context (the TTY auto-flip preempts interactive runs → home), the existing kickoff gate still applies (bails non-TTY unless `AIDA_NO_HUMAN_ACKNOWLEDGED=1`), and a loud advisory banner prints. Presence picks the MODE; the scope-ack stays a separate integrity gate (acceptance #4).
2. **Double-filtering keystone.** Don't re-exclude keystone in the drain — `parking_tag` already does. Presence only adds home-side surfacing.
3. **Explicit-flag-wins regressions.** The resolver must return the explicit value untouched whenever a flag is present. Unit-tested directly.
4. **escalate validation.** Presence-applied escalate only fires when no_human resolves to `both`, so the `--escalate-* needs --no-human=both` validation stays satisfied.

## Tests (named)
- `presence_config_parses_knobs_and_defaults` — `[presence]` block + missing-key defaults (on / headless-both / surface).
- `away_drain_advice_maps_three_rungs` — the (no_human, escalate) mapping per rung.
- `resolve_drain_mode_explicit_no_human_wins` — explicit `--no-human` untouched regardless of presence.
- `resolve_drain_mode_explicit_escalate_wins` — explicit escalate flag untouched.
- `resolve_drain_mode_home_is_interactive` — home → no presence-supplied default.
- `resolve_drain_mode_away_applies_config` — away + on → away_drain advice; away + off → no default.

## Verification
```bash
AIDA_BIN="$(git rev-parse --show-toplevel)/target/debug/aida"
cargo build -p aida-cli 2>&1 | tail -5
cargo test -p aida-cli presence 2>&1 | tail -20
"$AIDA_BIN" plan verify docs/plans/2026-06-12-story-561-presence-consumers.md
# Manual: AIDA_HOME=/tmp/ptest "$AIDA_BIN" away; AIDA_HOME=/tmp/ptest "$AIDA_BIN" status   # away line quiet-surfacing
```

## Followups
- Make consumers (b)/(c) per-knob tunable under `[presence]` if the fixed mapping proves too rigid (today they ride the away/home defaults).
- `aida human away/home` namespacing (SPIKE-57 Phase 2-4) — these aliases + the `aida human` vector; this build keeps the existing `aida away`/`home`/`presence` verbs.
- Promote "needs-triage" to its own `OpenBucket` label if the shelf grows (SPIKE-57 followup).

## Related
- Primitive: TASK-756 (`presence.rs`). Absorbing design: SPIKE-57 (`docs/plans/2026-06-11-human-as-first-class-role.md`).
- Composes: STORY-306 (escalation), STORY-555 (questions inbox), STORY-545 (`burndown run`), STORY-564 (`--zen` auto-exit).
