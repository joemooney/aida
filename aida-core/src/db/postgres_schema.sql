-- PostgreSQL schema for AIDA requirements management
-- Schema version 4

-- Schema version tracking
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);

INSERT INTO schema_version (version) VALUES (4) ON CONFLICT DO NOTHING;

-- Requirements table
CREATE TABLE IF NOT EXISTS requirements (
    id UUID PRIMARY KEY NOT NULL,
    spec_id TEXT,
    prefix_override TEXT,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'Draft',
    priority TEXT NOT NULL DEFAULT 'Medium',
    owner TEXT NOT NULL DEFAULT '',
    feature TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL,
    created_by TEXT,
    modified_at TIMESTAMPTZ NOT NULL,
    req_type TEXT NOT NULL DEFAULT 'Functional',
    dependencies JSONB NOT NULL DEFAULT '[]',
    tags JSONB NOT NULL DEFAULT '[]',
    relationships JSONB NOT NULL DEFAULT '[]',
    comments JSONB NOT NULL DEFAULT '[]',
    history JSONB NOT NULL DEFAULT '[]',
    archived BOOLEAN NOT NULL DEFAULT FALSE,
    custom_status TEXT,
    custom_priority TEXT,
    custom_fields JSONB NOT NULL DEFAULT '{}',
    urls JSONB NOT NULL DEFAULT '[]',
    trace_links JSONB NOT NULL DEFAULT '[]',
    implementation_info JSONB,
    ai_evaluation JSONB,
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
    id UUID PRIMARY KEY NOT NULL,
    spec_id TEXT,
    name TEXT NOT NULL,
    email TEXT NOT NULL DEFAULT '',
    handle TEXT NOT NULL,
    pin_hash TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    archived BOOLEAN NOT NULL DEFAULT FALSE,
    version INTEGER NOT NULL DEFAULT 1
);

-- Index for handle lookups
CREATE INDEX IF NOT EXISTS idx_users_handle ON users(handle);

-- Metadata table (single row with id=1)
CREATE TABLE IF NOT EXISTS metadata (
    id INTEGER PRIMARY KEY CHECK (id = 1) DEFAULT 1,
    name TEXT NOT NULL DEFAULT '',
    title TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    id_config JSONB NOT NULL DEFAULT '{}',
    features JSONB NOT NULL DEFAULT '[]',
    next_feature_number INTEGER NOT NULL DEFAULT 1,
    next_spec_number INTEGER NOT NULL DEFAULT 1,
    prefix_counters JSONB NOT NULL DEFAULT '{}',
    relationship_definitions JSONB NOT NULL DEFAULT '[]',
    reaction_definitions JSONB NOT NULL DEFAULT '[]',
    meta_counters JSONB NOT NULL DEFAULT '{}',
    type_definitions JSONB NOT NULL DEFAULT '[]',
    allowed_prefixes JSONB NOT NULL DEFAULT '[]',
    restrict_prefixes BOOLEAN NOT NULL DEFAULT FALSE,
    ai_prompts JSONB NOT NULL DEFAULT '{}',
    baselines JSONB NOT NULL DEFAULT '[]',
    teams JSONB NOT NULL DEFAULT '[]',
    store_version INTEGER NOT NULL DEFAULT 1
);

-- Insert default metadata row
INSERT INTO metadata (id) VALUES (1) ON CONFLICT DO NOTHING;
