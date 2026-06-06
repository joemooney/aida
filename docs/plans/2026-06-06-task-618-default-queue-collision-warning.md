# TASK-618 — Warn on 'default' user_id queue collision across machines

- **Date:** 2026-06-06
- **Specs:** TASK-618
- **Status:** Implemented
- **Complexity:** Low

## Approach

The distributed queue shards per `user_id` into `registry/queues/<user_id>.yaml`
inside the orphan `aida-store` branch. Distinct users never collide (intended
sharding). The hazard is two *different machines* resolving to the SAME
`user_id` — acute for the BUG-89 `"default"` fallback (CI/containers where
`$USER`/`$AIDA_USER`/`$USERNAME` are all unset), which makes both clones write
the same `default.yaml` → concurrent commits → merge conflict on the next
`aida db sync` rebase.

Per the operator decision on the spec, we only **warn** on the genuinely
dangerous shape; we do NOT build conflict-tolerant YAML merges (premature —
sharding already works for distinct users).

```
queue add (user_id == "default")
   │
   ├─ read existing default.yaml entries
   ├─ any entry stamped with a DIFFERENT machine fingerprint? ── yes ─▶ eprintln! warning
   │                                                            └─ no  ─▶ silent
   └─ stamp this clone's hostname into the new entry's added_by_machine
```

## Decisions

- **Fingerprint source:** `hostname()` (existing aida-cli helper), recorded on
  the entry as a new optional `QueueEntry.added_by_machine` field. Chosen over
  the git-committer of the queue commit because it is local, deterministic, and
  unit-testable as a pure predicate. aida-core never shells out to `hostname`;
  stamping happens in the CLI add path only.
- **Schema is additive + backward compatible:** `#[serde(default,
  skip_serializing_if = "Option::is_none")]`. Pre-TASK-618 entries and entries
  written through non-CLI paths (orchestrator, server, tests) carry `None` and
  are ignored by the predicate — never a false alarm.
- **Warn, never refuse** (consistent with the queue's other advisory warnings,
  e.g. STORY-333). No SPEC-ID in the user-facing stderr text (TASK-268 convention).
- **Only the "default" id triggers the check** — distinct user ids are the
  intended sharding and must stay silent.

## Files (build order)

1. `aida-core/src/models.rs` — add `QueueEntry.added_by_machine: Option<String>`.
2. All `QueueEntry { .. }` literal sites (git_backend, sqlite_backend,
   postgres_backend, backlog, rest, and main.rs non-add paths) — set
   `added_by_machine: None`.
3. `aida-cli/src/main.rs`:
   - `default_queue_collision_fingerprint(...)` — pure predicate.
   - `queue add` path — call it, emit the warning, stamp `Some(hostname())`.
   - unit tests for the predicate.

## Reusable helpers

- `hostname()` (aida-cli) — machine fingerprint.
- `default_queue_collision_fingerprint()` — pure, returns the foreign
  fingerprint to name, or `None`.

## Risks + gotchas

- `QueueEntry` has 20+ literal construction sites; missing one is a compile
  error (caught by the build), not a silent bug.
- The warning is best-effort: it only fires once a *foreign-stamped* entry is
  already on disk, so the very first cross-machine writer (before sync) won't
  see it — but the second one will, before the conflict compounds.

## Tests

- `collision_predicate_ignores_non_default_user`
- `collision_predicate_flags_default_foreign_machine`
- `collision_predicate_silent_when_all_same_machine`
- `collision_predicate_ignores_unknown_fingerprints`

## Verification

```bash
cargo build --release -p aida-cli
cargo test -p aida-cli --release collision_predicate
cargo fmt --all -- --check
```

## Followups

- If multi-machine `default`-id collaboration becomes common, revisit option
  (b): conflict-tolerant append-merge of queue YAML by entry uuid.

## Related

- BUG-89 (queue identity / "default" fallback)
- STORY-333 (warn-never-refuse queue advisory precedent)
- TASK-268 (no SPEC-IDs in user-facing text)
