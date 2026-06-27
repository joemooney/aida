# BUG-637 — Spec-scoped claim: refuse duplicate pickup + warn on editing a claimed spec (liveness-aware)

**Date**: 2026-06-27
**Specs**: BUG-637
**Status**: In Progress
**Complexity**: Medium

## Approach

The "claim" is a spec-scoped session **lease** — no new store. AIDA-launched work
(`aida queue work <spec>`, `aida agent new --spec`) already writes a lease whose
`scope == SPEC-N` (the `SessionLease` written in `handle_queue_work`). We add two
liveness-aware gates that read those leases:

1. **pre-pickup** (`handle_queue_work`): before claiming `owns`, if a *different*
   session already holds a **live** spec-scoped lease for `owns` → refuse/skip.
2. **pre-edit** (`aida edit`, via `enforce_session_lease`): editing/rejecting a
   spec a *different* session **live**-claims → warn (or block with `--strict`);
   `--force` overrides. This is the EPIC-54-reject fix.

Liveness reuses STORY-694's `lease_state_for` (which folds in `pid_is_alive` +
worktree existence + age). A claim whose holder is DEAD/Stale is ignored by both
gates → no crash-deadlock.

```
aida queue work SPEC-N ──▶ preflight_spec_status (status gate, unchanged)
                          └▶ NEW: live_spec_claim_by_other(SPEC-N)? ──▶ refuse
aida edit SPEC-N --status rejected ──▶ enforce_session_lease
                          └▶ lease_owning_spec now LIVENESS-FILTERED ──▶ warn/block
                          └▶ --force ⇒ skip entirely
```

## Decisions

- **Reuse, don't add surface.** Both gates read the existing `.aida/sessions/*.toml`
  registry via `list_leases` + `lease_state_for`. No new file, no new struct.
- **Liveness is the deadlock guard.** `lease_owning_spec` previously returned ANY
  foreign lease regardless of holder liveness — a crashed agent's stale lease would
  permanently block edits. We filter to `LeaseState::Live` only.
- **`--force` on edit fully bypasses** the claim gate (operator override). `--strict`
  still escalates Warn→Block for the live case.
- **Pre-pickup refuses only on a LIVE foreign claim**, never on our own lease, never
  on a stale/dead one, never on a non-spec scope. `--force-claim` bypasses (operator
  takeover), as does orchestrator-corroboration (parent already owns the spec).

## Files (build order)

- `aida-cli/src/main.rs`
  - `lease_is_live` (new): wraps `lease_state_for == Live` over a pre-probed
    live-session set, so callers share one probe.
  - `lease_owning_spec`: add an `is_live` predicate param; skip non-live leases.
  - `enforce_session_lease`: compute liveness, pass predicate; honor `--force`.
  - `live_spec_claim_by_other` (new): the pre-pickup gate's pure-ish check.
  - `handle_queue_work`: call it after preflight, refuse on a live foreign claim.
  - `Command::Edit` arm: pass `force` so the claim gate is bypassable.

## Critical files

- `aida-cli/src/main.rs` — `lease_owning_spec`, `enforce_session_lease`,
  `handle_queue_work`, `spec_scoped_lease`, `lease_state_for`.

## Reusable helpers

- `list_leases`, `lease_state_for`, `process_probe::probe_live_claude_sessions`,
  `process_probe::pid_is_alive`, `spec_scoped_lease`, `active_lease_for_cwd`.

## Risks + gotchas

- **Wrongly blocking legitimate work** — the chief risk. Mitigations:
  - Only LIVE claims gate (stale/dormant ignored).
  - Never gate against our OWN lease (self-lease excluded).
  - `--force` / `--force-claim` always override.
  - Orchestrator-corroborated children bypass pre-pickup.
  - Non-spec scopes (path globs) never gate.
- A `Dormant` lease (worktree present, no live claude, <24h) is treated as NOT
  blocking — matches `aida status`'s "is a LIVE process working it?".

## Tests (named)

- `lease_owning_spec_skips_dead_holder` (pre-edit liveness — stale ignored)
- `lease_owning_spec_returns_live_foreign` (pre-edit blocks live)
- `lease_owning_spec_skips_own_live_lease` (self never blocks)
- `live_spec_claim_by_other_blocks_on_live` (pre-pickup refuse)
- `live_spec_claim_by_other_ignores_stale` (pre-pickup stale ignored)
- `live_spec_claim_by_other_ignores_self` (pre-pickup self ignored)

## Verification

```
env -u AIDA_SESSION_ROLE cargo test -p aida-cli -p aida-core
cargo fmt --all -- --check
cargo clippy --workspace -- -D clippy::correctness
bash scripts/glyph-lint.sh --block
```

## Followups (not in this slice)

- Agent-tool fan-out leases are generic `harness-worktree` scopes, not spec-scoped —
  making THOSE spec-claimable is the documented follow-on (the BUG-637 advisor
  fan-out gap). Note, don't build here.
- STORY-711 commit-side authorization gate (a commit refusing to land work for a spec
  the committer doesn't hold the claim on) is a separate follow-on.

## Related

- STORY-694 (`aida status <spec>` liveness), STORY-696 (`aida ps`), STORY-48
  (session-lease enforcement), TASK-619 (cross-machine dup-pickup), BUG-634 (the
  duplicate-dispatch incident), EPIC-54 (the reject-while-working incident).
