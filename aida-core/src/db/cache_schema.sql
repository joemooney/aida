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
    feature TEXT NOT NULL DEFAULT '',
    req_type TEXT NOT NULL,
    tags_json TEXT NOT NULL DEFAULT '[]', -- JSON array, queried via LIKE for tag filters
    created_at TEXT NOT NULL,
    modified_at TEXT NOT NULL,
    archived INTEGER NOT NULL DEFAULT 0,
    archived_at TEXT,                      -- ISO RFC3339; NULL when not archived (STORY-441)
    yaml_path TEXT NOT NULL                -- relative path within the git store
);

CREATE INDEX IF NOT EXISTS idx_cache_spec_id ON requirements_cache(spec_id);
CREATE INDEX IF NOT EXISTS idx_cache_agreed_id ON requirements_cache(agreed_id);
CREATE INDEX IF NOT EXISTS idx_cache_status ON requirements_cache(status);
CREATE INDEX IF NOT EXISTS idx_cache_owner ON requirements_cache(owner);
CREATE INDEX IF NOT EXISTS idx_cache_modified ON requirements_cache(modified_at);
CREATE INDEX IF NOT EXISTS idx_cache_type ON requirements_cache(req_type);
CREATE INDEX IF NOT EXISTS idx_cache_feature ON requirements_cache(feature);
CREATE INDEX IF NOT EXISTS idx_cache_archived ON requirements_cache(archived);
CREATE INDEX IF NOT EXISTS idx_cache_archived_at ON requirements_cache(archived_at);

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
