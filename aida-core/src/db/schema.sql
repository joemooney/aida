-- Schema version tracking
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);

INSERT INTO schema_version (version) VALUES (7);

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
    meta_subtype TEXT,
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

-- GitLab sync state table (STORY-0325)
CREATE TABLE IF NOT EXISTS gitlab_sync_state (
    requirement_id TEXT NOT NULL,
    spec_id TEXT NOT NULL,
    gitlab_project_id INTEGER NOT NULL,
    gitlab_issue_iid INTEGER NOT NULL,
    gitlab_issue_id INTEGER NOT NULL,
    linked_at TEXT NOT NULL,
    last_sync TEXT NOT NULL,
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
    requirement_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    added_by TEXT NOT NULL,
    note TEXT,
    added_at TEXT NOT NULL,
    PRIMARY KEY (user_id, requirement_id)
);

-- Index for efficient queue listing ordered by position
CREATE INDEX IF NOT EXISTS idx_queue_user_position ON queue_entries(user_id, position);
