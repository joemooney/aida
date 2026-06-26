# BUG-626 — Epic status is a read-only rollup of child states

- Date: 2026-06-26
- Specs: BUG-626
- Status: Done
- Complexity: Medium

## Approach

An EPIC's status is today a manually-set field that drifts from reality both
directions (childless epics read In-Progress; epics with shipping children read
Draft). Make the EPIC's *displayed* status a read-only rollup derived from its
children's statuses, and **reject manual status edits on epics** (extend the
existing "Epic specs cannot be promoted to Approved" guard to all manual epic
status transitions, with a `--force` recovery escape).

```
child statuses -- derive_epic_status() --> epic's effective status
                       |
        +--------------+---------------------------+
   cache rebuild   show / why / status          edit guard
   (project col)   (full-store compute)         (reject manual set)
```

### Derivation rule (`aida_core::rollup::derive_epic_status`)

Given an epic and the rollup of its child subtree (the SAME walk
`aida graph --tree` prints via `graph_walk::child_status_rollup`):

1. **No children** (`total == 0`)        -> `Draft` (nothing started; "decompose me").
2. **All children Completed**            -> `Completed`.
3. **All children Done or Completed**    -> `Done` (work finished on branches, not all merged).
4. **>=1 child InProgress**, OR a mix of done-and-not-done -> `InProgress` (actively moving).
5. **>=1 child NeedsAttention** and none InProgress -> `NeedsAttention` (a child is shelved / blocked; there is no `Blocked` status variant so the shelved state stands in for "blocked").
6. **Only remaining (Draft/Approved/Planned) children**, none in progress -> `Draft` (queued, not started).
7. **Only Rejected children** (every non-rejected bucket empty) -> keep the epic's *stored* status (recovery / human call -- we don't auto-reject an epic).

Edge precedence is evaluated top-to-bottom; the rollup buckets are
`completed / done / in_progress / remaining / shelved / rejected` (already on
`StatusRollup`).

## Design decision: compute-on-read, materialized into the cache (mirror `compute_blocked`)

Two options were on the table:
- **compute-on-read** -- always correct, but `aida list` is cache-backed and
  filters/displays `status` from a SQL column, so a pure read-time projection
  would have to re-load the full store on every list (defeats the cache).
- **store-the-derived-status-in-cache** -- fast list, must recompute on child change.

**Chosen: store the derived epic status in the cache's `status` column at
rebuild time**, exactly mirroring `compute_blocked` / `compute_degrees` -- both
are whole-graph derived facts materialized into the cache at rebuild, queried in
SQL like any other column. The full store is available at `rebuild_from_store`,
so the rollup is computed correctly there. The single canonical derivation
(`derive_epic_status`) is reused at the full-store display surfaces (`show`,
`why`, project-`status`) so cache and non-cache paths agree.

Staleness: the `blocked`/inbound-degree precedent accepts "authoritative after a
full rebuild; a single-row write may leave a neighbor momentarily stale." We do
**better** than that precedent: `upsert_requirement` is extended so that when a
child row is written, the child's Parent epic's cached `status` is recomputed
from the children's cached statuses -- closing the gap so `aida list` reflects a
child status flip immediately, without a full rebuild.

## Files (build order)

1. `aida-core/src/rollup.rs` (new) -- `derive_epic_status(store, epic) -> Option<RequirementStatus>` + `derive_epic_status_from_rollup`. Unit-tested in-module.
2. `aida-core/src/lib.rs` -- register `pub mod rollup;` + re-export.
3. `aida-core/src/db/cache.rs` -- at `rebuild_from_store`, override each epic row's projected status with the derived value; at `upsert_requirement`, recompute the upserted child's parent epic row.
4. `aida-cli/src/main.rs` -- `show`, `why`, project-`status` use `derive_epic_status` for epics; the edit guard rejects all manual epic status transitions.

## Tests

- `aida-core`: childless epic -> Draft; one InProgress child -> InProgress; all-Completed -> Completed; mix done/not -> InProgress; all-Done -> Done; shelved child -> NeedsAttention; only-rejected -> keep stored.
- `aida-cli`: manual epic status edit is rejected (and `--force` bypasses); show/why reflect rollup.

## Verification

```
cargo build
cargo fmt --all -- --check
cargo clippy --workspace -- -D clippy::correctness
bash scripts/glyph-lint.sh --block
env -u AIDA_SESSION_ROLE cargo test -p aida-cli -p aida-core
```

## Related

- `aida-core/src/db/cache.rs` `compute_blocked` (the pattern this mirrors)
- `aida-core/src/graph_walk.rs` `child_status_rollup` (the reused rollup walk)
- BUG-543 (epic-ready-to-close rollup), STORY-489 (graph queries)
