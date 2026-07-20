-- Cache projection of the git-canonical requirements store.
-- The truth lives in YAML files in the git orphan store; this SQLite file
-- exists only to make list/filter/search queries fast for the web UI and CLI.
-- Drop and rebuild from git anytime — see cache::rebuild_from_git.

CREATE TABLE IF NOT EXISTS requirements_cache (
    id TEXT PRIMARY KEY NOT NULL,         -- UUID
    spec_id TEXT,                          -- e.g., FR-0042 or FR-1-001
    agreed_id TEXT,                        -- e.g., FR-1 (post merge-gate)
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '', -- kept short — full record is in YAML
    status TEXT NOT NULL,
    priority TEXT NOT NULL,
    owner TEXT NOT NULL DEFAULT '',
    assignee TEXT,                         -- STORY-639: team-member this spec is assigned to; NULL when unassigned
    feature TEXT NOT NULL DEFAULT '',
    req_type TEXT NOT NULL,
    tags_json TEXT NOT NULL DEFAULT '[]', -- JSON array, queried via LIKE for tag filters
    created_at TEXT NOT NULL,
    modified_at TEXT NOT NULL,
    archived INTEGER NOT NULL DEFAULT 0,
    archived_at TEXT,                      -- ISO RFC3339; NULL when not archived (STORY-441)
    deferred INTEGER NOT NULL DEFAULT 0,   -- STORY-584: view-flag parallel to archived
    deferred_at TEXT,                      -- ISO RFC3339; NULL when not deferred (STORY-584)
    deferred_until TEXT,                   -- free-text revisit trigger; NULL when none (STORY-584)
    -- STORY-632: deterministic local graph-centrality, computed during cache
    -- rebuild from the relationship graph — NEVER stored in canonical YAML.
    -- in_degree  = count of inbound edges (specs that reference/depend-on this);
    --              high = foundational / load-bearing heft.
    -- out_degree = count of this spec's own outbound edges; high = coupling.
    -- heft       = type-weighted combined score (static RelationshipType->weight
    --              lookup applied to both in + out edges). See cache::edge_weight.
    in_degree INTEGER NOT NULL DEFAULT 0,
    out_degree INTEGER NOT NULL DEFAULT 0,
    heft INTEGER NOT NULL DEFAULT 0,
    -- TASK-902: "has an incomplete BlockedBy edge" projected into the cache so
    -- `aida list --blocked` (and the `blocked` field of `--json`) reads the
    -- cache like --status/--archived instead of a full backend.load() over
    -- every object. Graph-derived (depends on each BlockedBy target's status),
    -- so authoritative only after a full rebuild — same rebuildable-projection
    -- contract as in_degree/heft. NEVER stored in canonical YAML.
    blocked INTEGER NOT NULL DEFAULT 0,
    -- TASK-1065: "has an unanswered DecisionRequest" projected into the cache so
    -- `aida status --full`'s decision-inbox count reads the cache instead of a
    -- full backend.load() over every object. Per-row projection of
    -- `decision_request.is_pending()`; authoritative after any single-row upsert
    -- (unlike the graph-derived `blocked`). NEVER stored in canonical YAML.
    has_pending_decision INTEGER NOT NULL DEFAULT 0,
    -- STORY-776: the advisor's bless-time execution-mode classification
    -- (drain|drive|guided|operator|decide), projected from the canonical YAML
    -- `execution_mode` field so `aida list --fields ...,mode` reads the cache.
    -- NULL = ungroomed.
    execution_mode TEXT,
    -- FR-283: the optional first-class numeric weight/score, projected from the
    -- canonical YAML `weight` field so `aida list --sort weight` and the
    -- `--min-weight` / `--max-weight` filters read the cache. NULL = unset.
    weight REAL,
    -- STORY-634: the multi-repo origin dimension ("repo" or "repo/component",
    -- ADR-12), projected from the canonical YAML `origin` field so
    -- `aida list --fields ...,origin` reads the cache. NULL = single-repo.
    origin TEXT,
    yaml_path TEXT NOT NULL                -- relative path within the git store
);

CREATE INDEX IF NOT EXISTS idx_cache_spec_id ON requirements_cache(spec_id);
CREATE INDEX IF NOT EXISTS idx_cache_agreed_id ON requirements_cache(agreed_id);
CREATE INDEX IF NOT EXISTS idx_cache_status ON requirements_cache(status);
CREATE INDEX IF NOT EXISTS idx_cache_owner ON requirements_cache(owner);
CREATE INDEX IF NOT EXISTS idx_cache_assignee ON requirements_cache(assignee);
CREATE INDEX IF NOT EXISTS idx_cache_modified ON requirements_cache(modified_at);
CREATE INDEX IF NOT EXISTS idx_cache_type ON requirements_cache(req_type);
CREATE INDEX IF NOT EXISTS idx_cache_feature ON requirements_cache(feature);
CREATE INDEX IF NOT EXISTS idx_cache_archived ON requirements_cache(archived);
CREATE INDEX IF NOT EXISTS idx_cache_archived_at ON requirements_cache(archived_at);
CREATE INDEX IF NOT EXISTS idx_cache_deferred ON requirements_cache(deferred);
CREATE INDEX IF NOT EXISTS idx_cache_deferred_at ON requirements_cache(deferred_at);
-- STORY-632: index heft so `aida list --sort heft` orders without a table scan.
CREATE INDEX IF NOT EXISTS idx_cache_heft ON requirements_cache(heft);
-- FR-283: index weight so `aida list --sort weight` orders without a table scan.
CREATE INDEX IF NOT EXISTS idx_cache_weight ON requirements_cache(weight);
-- TASK-902: index blocked so `aida list --blocked` filters without a table scan.
CREATE INDEX IF NOT EXISTS idx_cache_blocked ON requirements_cache(blocked);

-- TASK-955: parent->child hierarchy edges, materialized at rebuild from the
-- relationship graph so `aida list --parent <id> --recursive` can walk the full
-- transitive subtree with one WITH RECURSIVE query instead of a backend.load().
-- The hierarchy edge can live on EITHER endpoint (a parent carries Child->child,
-- a child carries Parent->parent), so both are normalized to parent->child here
-- — the same union `aida graph --tree` walks (BUG-448). Rebuildable projection,
-- NEVER stored in canonical YAML; authoritative after a full cache rebuild.
-- BUG-764: `author_id` records WHICH requirement's record carries the edge
-- (either endpoint can, and reciprocal writes mean both may). Single-row
-- upserts delete/re-derive only the edges the written row AUTHORED, so an
-- epic-self write (e.g. a comment add) no longer destroys the child-authored
-- edges its rollup membership depends on. The recursive membership CTEs
-- keep reading (parent_id, child_id); duplicate rows across authors are
-- deduped by their UNION semantics.
CREATE TABLE IF NOT EXISTS hierarchy_edges (
    parent_id TEXT NOT NULL,               -- UUID of the parent (epic/story)
    child_id TEXT NOT NULL,                -- UUID of the child
    author_id TEXT NOT NULL DEFAULT '',    -- UUID of the record carrying the edge
    PRIMARY KEY (parent_id, child_id, author_id)
);
CREATE INDEX IF NOT EXISTS idx_edges_parent ON hierarchy_edges(parent_id);
CREATE INDEX IF NOT EXISTS idx_edges_child ON hierarchy_edges(child_id);

CREATE VIRTUAL TABLE IF NOT EXISTS requirements_fts USING fts5(
    id UNINDEXED,
    spec_id,
    agreed_id,
    title,
    description,
    external_refs,                         -- STORY-476: space-joined provider:id refs
    tokenize = 'porter unicode61'
);

-- Cache-level metadata: schema version, last rebuild time, source git HEAD SHA.
CREATE TABLE IF NOT EXISTS cache_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
