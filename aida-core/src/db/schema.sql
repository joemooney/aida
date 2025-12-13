-- Schema version tracking
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);

INSERT INTO schema_version (version) VALUES (4);

-- Requirements table
CREATE TABLE IF NOT EXISTS requirements (
    id TEXT PRIMARY KEY NOT NULL,
    spec_id TEXT,
    prefix_override TEXT,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'Draft',
    priority TEXT NOT NULL DEFAULT 'Medium',
    owner TEXT NOT NULL DEFAULT '',
    feature TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    created_by TEXT,
    modified_at TEXT NOT NULL,
    req_type TEXT NOT NULL DEFAULT 'Functional',
    dependencies TEXT NOT NULL DEFAULT '[]',
    tags TEXT NOT NULL DEFAULT '[]',
    relationships TEXT NOT NULL DEFAULT '[]',
    comments TEXT NOT NULL DEFAULT '[]',
    history TEXT NOT NULL DEFAULT '[]',
    archived INTEGER NOT NULL DEFAULT 0,
    custom_status TEXT,
    custom_priority TEXT,
    custom_fields TEXT NOT NULL DEFAULT '{}',
    urls TEXT NOT NULL DEFAULT '[]',
    trace_links TEXT NOT NULL DEFAULT '[]',
    implementation_info TEXT,
    ai_evaluation TEXT,
    version INTEGER NOT NULL DEFAULT 1
);

-- Index for spec_id lookups
CREATE INDEX IF NOT EXISTS idx_requirements_spec_id ON requirements(spec_id);

-- Index for feature filtering
CREATE INDEX IF NOT EXISTS idx_requirements_feature ON requirements(feature);

-- Index for status filtering
CREATE INDEX IF NOT EXISTS idx_requirements_status ON requirements(status);

-- Index for archived filtering
CREATE INDEX IF NOT EXISTS idx_requirements_archived ON requirements(archived);

-- Users table
CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY NOT NULL,
    spec_id TEXT,
    name TEXT NOT NULL,
    email TEXT NOT NULL DEFAULT '',
    handle TEXT NOT NULL,
    pin_hash TEXT,
    created_at TEXT NOT NULL,
    archived INTEGER NOT NULL DEFAULT 0,
    version INTEGER NOT NULL DEFAULT 1
);

-- Index for handle lookups
CREATE INDEX IF NOT EXISTS idx_users_handle ON users(handle);

-- Metadata table (single row with id=1)
CREATE TABLE IF NOT EXISTS metadata (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    name TEXT NOT NULL DEFAULT '',
    title TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    id_config TEXT NOT NULL DEFAULT '{}',
    features TEXT NOT NULL DEFAULT '[]',
    next_feature_number INTEGER NOT NULL DEFAULT 1,
    next_spec_number INTEGER NOT NULL DEFAULT 1,
    prefix_counters TEXT NOT NULL DEFAULT '{}',
    relationship_definitions TEXT NOT NULL DEFAULT '[]',
    reaction_definitions TEXT NOT NULL DEFAULT '[]',
    meta_counters TEXT NOT NULL DEFAULT '{}',
    type_definitions TEXT NOT NULL DEFAULT '[]',
    allowed_prefixes TEXT NOT NULL DEFAULT '[]',
    restrict_prefixes INTEGER NOT NULL DEFAULT 0,
    ai_prompts TEXT NOT NULL DEFAULT '{}',
    baselines TEXT NOT NULL DEFAULT '[]',
    teams TEXT NOT NULL DEFAULT '[]',
    store_version INTEGER NOT NULL DEFAULT 1
);

-- Insert default metadata row
INSERT INTO metadata (id) VALUES (1);
