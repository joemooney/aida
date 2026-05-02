# Git-Canonical Storage with SQLite Cache View

**Date**: 2026-05-02
**Epic**: EPIC-1-001
**Status**: Approved (design phase) — implementation pending phase-by-phase approval

## Context

AIDA currently maintains four equivalent storage backends as first-class options: YAML (single file), SQLite, PostgreSQL, and a git-orphan-branch sharded YAML store. Maintaining four equivalent code paths is unsustainable for a one-person project and dilutes AIDA's defensible niche — the durable, agent-readable, distributed spec layer.

The git-orphan-branch backend is the only one that simultaneously satisfies all four properties AIDA's vision asks for:
- **Agent-readable** (one YAML file per requirement, plain text)
- **Durable** (git history captures every change)
- **Distributed** (HLC timestamps + agreed-IDs + merge-gate enable offline-capable multi-node operation)
- **Code-adjacent** (lives in the same git repo as the code that traces to it)

YAML and SQLite as standalone backends are subsets of what git-store provides; PostgreSQL is a different-shape solution for a different audience (teams wanting a server-backed shared view).

This document specifies the collapse of the four backends into a single canonical model with a derived read cache, following CQRS / event-sourcing patterns.

## Target Architecture

```
                    ┌──────────────────────────────┐
   CLI / MCP /  ──> │     Write path               │
   Server                │  1. Mutate git store      │
                         │  2. Update SQLite cache   │
                    └──────────────────────────────┘
                                  │
                  ┌───────────────┴───────────────┐
                  ▼                               ▼
       ┌────────────────────┐         ┌────────────────────────┐
       │  Git orphan store  │         │   SQLite cache         │
       │  (canonical)       │ ───────>│   (rebuildable view)   │
       │  aida-store/       │ rebuild │   requirements.db      │
       │  YAML per object   │         │   indexed for queries  │
       └────────────────────┘         └────────────────────────┘
                  │                               │
                  │                               ▼
                  │                    ┌────────────────────┐
                  │                    │   Read path        │
                  │                    │  Web UI, search,   │
                  │                    │  analytics, list   │
                  │                    └────────────────────┘
                  │
                  ▼
       ┌────────────────────┐
       │   YAML export      │
       │   requirements.yaml│
       │   (pre-commit hook)│
       └────────────────────┘
```

### Roles

- **Git orphan branch (`aida-store/`)** — canonical writer of record. All mutations land here first. Stores one YAML file per requirement under `objects/<TYPE>/<shard>/<spec_id>.yaml`. HLC + agreed-ID + node-identity machinery already exists.
- **SQLite cache (`requirements.db`)** — derived, rebuildable read cache. Optimized for filter / search / aggregation queries. Schema is a *projection* of the canonical model, not a duplicate.
- **YAML export (`requirements.yaml`)** — single-file diffable artifact via pre-commit hook. Not a runtime backend. Useful for cross-repo diff review.
- **PostgreSQL** — opt-in via `aida-backend-postgres` plugin crate. For teams wanting a shared server-backed projection. Not in default builds.

## Decisions

All four open questions resolved 2026-05-02. See per-question rationale below.

### Q1. Cache write strategy: write-through vs. on-demand rebuild? — **WRITE-THROUGH**

Every mutation updates git first, then atomically updates the SQLite cache as part of the same operation. If git write succeeds but cache update fails, the cache is marked stale and rebuilt on next read.

- **Pro:** Simple mental model. Web UI sees CLI changes immediately. No background daemon required.
- **Con:** Git writes are slower than SQLite writes (~50ms vs <1ms locally).
- **Deferred:** Write-behind batched commits for bulk-import paths (Jira sync, GitHub pull, large `aida import`) — captured as a child requirement under EPIC-1-001.

### Q2. Pull-time cache sync (after `git pull` or `db sync`)? — **DETECT-AND-REBUILD** Compare the orphan branch's HEAD SHA against a `cache_head_sha` value stored in the cache metadata. On mismatch, trigger a full rebuild before serving any read.

- **Pro:** No background daemon. Always-correct. Cheap to detect.
- **Con:** First read after a remote sync is slow (full rebuild). For typical AIDA stores (~hundreds of requirements) this is sub-second; for 100K+ stores we add incremental rebuild later.

### Q3. Cache schema: identical to current SQLite, or simplified? — **SIMPLIFY AGGRESSIVELY** Drop columns that exist only because SQLite was authoritative — heavy JSON columns for `history`, `comments`, `urls`, `attachments`, `relationships`, `gitlab_issues`, `custom_fields`. These remain available by reading the YAML file from the git store on demand. Cache holds only what's needed for fast filter/search:

```sql
CREATE TABLE requirements_cache (
    id TEXT PRIMARY KEY,        -- UUID
    spec_id TEXT,               -- e.g., FR-0042
    agreed_id TEXT,             -- e.g., FR-1
    title TEXT NOT NULL,
    status TEXT NOT NULL,
    priority TEXT NOT NULL,
    owner TEXT,
    feature TEXT,
    req_type TEXT NOT NULL,
    tags TEXT,                  -- JSON array, indexed via FTS
    created_at TEXT NOT NULL,
    modified_at TEXT NOT NULL,
    archived INTEGER NOT NULL DEFAULT 0,
    yaml_path TEXT NOT NULL     -- pointer to canonical YAML
);
CREATE INDEX idx_cache_spec_id ON requirements_cache(spec_id);
CREATE INDEX idx_cache_status ON requirements_cache(status);
CREATE INDEX idx_cache_owner ON requirements_cache(owner);
CREATE INDEX idx_cache_modified ON requirements_cache(modified_at);
CREATE VIRTUAL TABLE requirements_fts USING fts5(spec_id, title, description, tags);

CREATE TABLE cache_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- e.g., ('cache_head_sha', '<git-sha>'), ('schema_version', '1'), ('built_at', '...')
```

When the web UI shows a requirement detail, the server reads the YAML directly for the full record. List views never touch YAML.

- **Pro:** Cache is small, fast, easy to rebuild. No duplicate-state bugs (the truth is in YAML).
- **Con:** Detail view becomes one extra YAML read. Negligible.

### Q4. Backward compatibility window for standalone YAML / SQLite users? — **HARD CUT**

No deprecation window. AIDA has no external users; the single dogfooder migrates atomically with the migration tool in Phase 2. Removing standalone-canonical paths in the same release as the migration tool is faster and avoids carrying duplicate code.

- **Pro:** Less code to maintain, no half-state. Forces honest commitment to the new model.
- **Con:** Migration must be reliable on first try. Mitigated by idempotency, pre-migration SQLite snapshot, and a query-equality assertion (post-migration query results == pre-migration).

## Phasing (compressed)

Original plan had 4 phases; compressed to 3 by merging "read-only rebuild" and "switch write path" into a single Phase 1 (since both touch the same code surface and the user wants to move faster).

### Phase 0 — Design (this document) and epic capture

**Status:** Complete. EPIC-1-001 captured.

### Phase 1 — Implement git-canonical storage end-to-end

The cache module, the rebuild command, and the write-through path land together. No half-state; either git-canonical works in distributed mode or this phase isn't done.

**Deliverables:**
- `aida-core/src/db/cache.rs` — cache projection module with `rebuild_from_git()` and incremental write hooks
- `aida-core/src/db/cache_schema.sql` — simplified schema (per Q3)
- New `Storage::GitCanonical` variant wrapping `(GitBackend, CacheBackend)`
- Mutation operations: write YAML to git → commit → update cache (write-through)
- Read operations: list/search/filter hit cache; detail/show reads YAML directly
- Stale-cache detection: HEAD-SHA mismatch triggers rebuild before next read
- CLI: `aida cache rebuild` (force full), `aida cache status` (HEAD comparison)
- Tests: rebuild produces identical query results to existing SQLite backend on a sample dataset; full mutation cycle (add/edit/delete/comment) round-trips correctly

**Risk:** medium — touches mutation paths but only when distributed mode is active. Standalone SQLite/YAML untouched until Phase 3.

### Phase 2 — Migration tool and AIDA self-host

Add `aida db migrate --to git-canonical` to convert SQLite-canonical projects, then run it on AIDA itself.

**Deliverables:**
- Migration command: SQLite → git-store + cache
- Idempotent (safe to re-run); takes pre-migration SQLite snapshot
- Query-equality assertion: post-migration query results match pre-migration on representative queries
- AIDA's own `requirements.db` migrated as the first dogfood
- Web dashboard verified to work against migrated store

**Risk:** medium — UUIDs, timestamps, history, relationships, comments must round-trip. Mitigated by snapshot + assertion.

### Phase 3 — Hard cut: remove legacy paths, extract Postgres

**Deliverables:**
- Remove `yaml_backend.rs` and `sqlite_backend.rs` standalone-canonical variants (cache-only SQLite remains)
- Extract `aida-core/src/db/postgres_backend.rs` + schema to new `aida-backend-postgres` crate
- Update CLI to error on standalone backend file detection with a "run `aida db migrate --to git-canonical`" message
- Update CLAUDE.md, OVERVIEW.md, README to reflect single-canonical model

**Risk:** low — by this point AIDA itself is on git-canonical and migration is proven.

## Migration Strategy (AIDA itself)

The user's AIDA already has `aida-store/` populated with 355 YAML files (close to the 362 SQLite count) — meaning the data is largely already in canonical form from earlier `db export-git` runs. The "migration" is therefore more about declaring git-canonical as the operating mode than moving data.

Concrete steps when Phase 3 lands:
1. Run `aida db export-git -o aida-store --refresh` to ensure the store is current with `requirements.db`
2. Run `aida db migrate --to git-canonical` — this writes `.aida/config.toml` and rebuilds the cache
3. Verify `aida list` and the web dashboard return the same data
4. Commit `aida-store/` and `.aida/config.toml`; the SQLite file (`requirements.db`) stays gitignored

## Risks and Mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| Cache rebuild too slow at scale | Low (current scale is hundreds, not 100K) | Incremental rebuild via git-diff in Phase 5+ |
| Cache + git divergence due to bug | Medium | Stale-cache flag triggers rebuild; tests cover invariants |
| Migration loses data (UUIDs, timestamps, relationships) | Medium | Idempotent migration, full equality assertion against pre-migration SQLite snapshot |
| Distributed users on legacy SQLite mode break | Low | One-release deprecation warning before removal |
| Web UI latency regression | Low | Cache schema indexed for current query patterns; benchmarked in Phase 1 |

## Out of Scope

- New query capabilities (this is a refactor, not a feature)
- Changes to MCP tool surface (Phase 2 swaps the backend transparently)
- Changes to commit hook (already works on YAML export)
- React dashboard changes (server REST surface unchanged)

## Related Requirements

- EPIC-1-001 — Git-canonical storage with SQLite cache view (this design doc)
- (deferred) Bulk-import write-behind batching — child of EPIC-1-001, captured for future work

Will spawn child requirements per phase as work begins.

## Status

In Progress — Phase 0 complete (this doc + EPIC-1-001), Phase 1 implementation starting.
