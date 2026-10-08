# STORY-1096 slice 1 — `aida supervise watch --objective` (oversight watch loop)

- **Date:** 2026-09-13
- **Specs:** STORY-1096 (slice 1); ADR-29 (architecture); reuses STORY-1051 (redrive), STORY-1052 (nudge)
- **Status:** in progress
- **Complexity:** medium

## Approach

Add a third `supervise` subcommand, `watch`, that is the substrate-first oversight
loop from ADR-29. It **composes** the two shipped reflexes (`redrive`, `nudge`) and
adds the one genuinely new capability — **objective-drift detection + realign** —
then surfaces only human-decision items. It never drives or merges.

```
aida supervise watch --objective EPIC-63 [--execute] [--interval SECS] [--json]

  one pass (default = dry-run report):
    1. resolve objective EPIC + its children
    2. DRIFT: children Approved but not queued & not deferred/archived
         --execute → queue them for implementer (realign)
         dry-run   → report what it would queue
    3. reflex: redrive transient parks   (supervisor::handle_supervise_command)
    4. reflex: nudge advisor on stalls   (supervise_cmd::handle_supervise_nudge)
    5. surface: objective progress (done/total) + `aida awaiting` items
  --interval SECS → repeat the pass on a sleep loop (else single pass)
```

## Decisions (from ADR-29)

- **Objective input:** `--objective <EPIC>` flag; `[oversight] objective` config default when omitted.
- **Authority:** realign-queue + fire reflexes + surface/escalate. NEVER drive/merge.
- **Default is dry-run:** mirrors `redrive` — `--execute` to act. Safe to run blind.
- **Substrate-first:** reads events/store/queue; no dependence on a live agent. `--interval` is a thin sleep loop, not a daemon (revisit a real service in slice 2).

## Files (build order)

1. `aida-cli-lib/src/cli.rs` — add `SuperviseCommand::Watch { objective, execute, interval, json }`.
2. `aida-cli-lib/src/supervise_cmd.rs` — dispatch arm → `handle_supervise_watch`; the handler composing realign + redrive + nudge + surface.
3. `aida-cli-lib/src/tests/story_1096_supervise_watch_tests.rs` — drift detection + dry-run/execute + no-objective config fallback.

## Reusable helpers (don't reimplement)

- `crate::supervisor::handle_supervise_command(backend, project_root, opts)` — redrive.
- `crate::supervise_cmd::handle_supervise_nudge(backend, store_path)` — nudge.
- `ensure_queued_for_implementer(storage, user_id, spec)` (lib.rs, STORY-246) — the realign primitive.
- `graph_walk` / `RelationshipType::Parent` — resolve EPIC children.
- `aida awaiting` surface — reuse its computation for the human-decision report.

## Risks / gotchas

- Realign must be **idempotent** — never double-queue a spec already queued (reuse the `ensure_queued_for_implementer` guard).
- Epic status is a read-only rollup — do not try to set it; only queue children.
- Don't queue deferred/archived children (respect the view tiers).
- Keep SPEC-IDs out of user-facing stdout (trace comments only).

## Tests

- `drift_detects_approved_unqueued_children`
- `realign_is_idempotent_for_already_queued`
- `dry_run_reports_without_queueing`
- `objective_from_config_when_flag_omitted`

## Verification

```
cargo test -p aida-cli-lib story_1096
aida supervise watch --objective EPIC-63            # dry-run report
aida supervise watch --objective EPIC-63 --execute  # realign + reflexes
```

## Followups

- Slice 2 = STORY-1107 (promote to first-class `oversight` role).
- `/aida-oversee` skill (judgment layer) — after the loop is dogfooded.

## Related

ADR-29, STORY-1051, STORY-1052, STORY-1107, EPIC-62, docs/testimonials/2026-09-12-llm-field-report.md
