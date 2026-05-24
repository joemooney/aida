-- PostgreSQL schema for AIDA requirements management
-- Schema version 8

-- Schema version tracking
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);

INSERT INTO schema_version (version) VALUES (8) ON CONFLICT DO NOTHING;

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
    meta_subtype TEXT,
    dependencies JSONB NOT NULL DEFAULT '[]',
    tags JSONB NOT NULL DEFAULT '[]',
    relationships JSONB NOT NULL DEFAULT '[]',
    comments JSONB NOT NULL DEFAULT '[]',
    history JSONB NOT NULL DEFAULT '[]',
    archived BOOLEAN NOT NULL DEFAULT FALSE,
    archived_at TIMESTAMPTZ,
    custom_status TEXT,
    custom_priority TEXT,
    custom_fields JSONB NOT NULL DEFAULT '{}',
    urls JSONB NOT NULL DEFAULT '[]',
    trace_links JSONB NOT NULL DEFAULT '[]',
    weight REAL,
    attachments JSONB NOT NULL DEFAULT '[]',
    gitlab_issues JSONB NOT NULL DEFAULT '[]',
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

-- Index for archived_at filtering (STORY-441 archive sweep)
CREATE INDEX IF NOT EXISTS idx_requirements_archived_at ON requirements(archived_at);

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

-- GitLab sync state table (STORY-0325)
CREATE TABLE IF NOT EXISTS gitlab_sync_state (
    requirement_id UUID NOT NULL,
    spec_id TEXT NOT NULL,
    gitlab_project_id BIGINT NOT NULL,
    gitlab_issue_iid BIGINT NOT NULL,
    gitlab_issue_id BIGINT NOT NULL,
    linked_at TIMESTAMPTZ NOT NULL,
    last_sync TIMESTAMPTZ NOT NULL,
    aida_content_hash TEXT NOT NULL DEFAULT '',
    gitlab_content_hash TEXT NOT NULL DEFAULT '',
    link_origin TEXT NOT NULL DEFAULT 'ManualLink',
    sync_status TEXT NOT NULL DEFAULT 'Untracked',
    last_error TEXT,
    PRIMARY KEY (requirement_id, gitlab_issue_iid)
);

-- Index for looking up sync state by requirement
CREATE INDEX IF NOT EXISTS idx_gitlab_sync_requirement ON gitlab_sync_state(requirement_id);

-- Index for looking up sync state by GitLab issue
CREATE INDEX IF NOT EXISTS idx_gitlab_sync_issue ON gitlab_sync_state(gitlab_project_id, gitlab_issue_iid);

-- Index for filtering by sync status
CREATE INDEX IF NOT EXISTS idx_gitlab_sync_status ON gitlab_sync_state(sync_status);

-- Queue entries table (STORY-0366) - personal work queue per user
CREATE TABLE IF NOT EXISTS queue_entries (
    user_id TEXT NOT NULL,
    requirement_id UUID NOT NULL,
    position INTEGER NOT NULL,
    added_by TEXT NOT NULL,
    note TEXT,
    added_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (user_id, requirement_id)
);

-- Index for efficient queue listing ordered by position
CREATE INDEX IF NOT EXISTS idx_queue_user_position ON queue_entries(user_id, position);
