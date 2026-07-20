//! PostgreSQL database storage backend
//!
//! This backend stores requirements data in a PostgreSQL database,
//! providing scalability, concurrent access, and enterprise database features.
//!
//! # Connection String Format
//!
//! ```text
//! postgres://user:password@host:port/database
//! ```
//!
//! # Example
//!
//! ```ignore
//! use aida_core::db::PostgresBackend;
//!
//! let backend = PostgresBackend::new("postgres://user:pass@localhost:5432/aida")?;
//! let store = backend.load()?;
//! ```

use anyhow::{Context, Result};
use postgres::{GenericClient, Row};
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::models::{
    Attachment, Comment, CustomTypeDefinition, FeatureDefinition, GitLabIssueLink, HistoryEntry,
    IdConfiguration, ImplementationInfo, QueueEntry, ReactionDefinition, Relationship,
    RelationshipDefinition, Requirement, RequirementPriority, RequirementStatus, RequirementType,
    RequirementsStore, TraceLink, UrlLink, User,
};

use super::traits::{BackendType, DatabaseBackend, UpdateResult, VersionConflict};

/// Current schema version - updated to 8 for requirement weight/attachments/gitlab issues
const SCHEMA_VERSION: i32 = 8;

/// PostgreSQL backend implementation with connection pooling
pub struct PostgresBackend {
    /// Connection string stored as path for trait compatibility
    connection_string: PathBuf,
    /// Connection pool
    pool: Pool<PostgresConnectionManager<postgres::NoTls>>,
}

impl PostgresBackend {
    /// Creates a new PostgreSQL backend from a connection string
    ///
    /// Connection string format: `postgres://user:password@host:port/database`
    pub fn new<S: AsRef<str>>(connection_string: S) -> Result<Self> {
        let conn_str = connection_string.as_ref();

        let manager = PostgresConnectionManager::new(conn_str.parse()?, postgres::NoTls);

        let pool = Pool::builder()
            .max_size(10)
            .build(manager)
            .context("Failed to create connection pool")?;

        let backend = Self {
            connection_string: PathBuf::from(conn_str),
            pool,
        };

        backend.init_schema()?;
        Ok(backend)
    }

    /// Initialize the database schema
    fn init_schema(&self) -> Result<()> {
        let mut client = self
            .pool
            .get()
            .context("Failed to get connection from pool")?;

        // Check if schema_version table exists
        let table_exists: bool = client
            .query_one(
                "SELECT EXISTS (
                    SELECT FROM information_schema.tables
                    WHERE table_name = 'schema_version'
                )",
                &[],
            )?
            .get(0);

        let current_version: i32 = if table_exists {
            client
                .query_opt("SELECT version FROM schema_version LIMIT 1", &[])?
                .map(|row| row.get(0))
                .unwrap_or(0)
        } else {
            0
        };

        if current_version == 0 {
            // Create initial schema
            client.batch_execute(include_str!("postgres_schema.sql"))?;
        } else if current_version < SCHEMA_VERSION {
            // Handle migrations
            Self::migrate_schema(&mut *client, current_version)?;
        }

        Ok(())
    }

    /// Migrate schema from old version to current
    fn migrate_schema<C: GenericClient>(client: &mut C, from_version: i32) -> Result<()> {
        if from_version < 2 {
            client.batch_execute(
                r#"
                ALTER TABLE requirements ADD COLUMN IF NOT EXISTS version INTEGER NOT NULL DEFAULT 1;
                ALTER TABLE requirements ADD COLUMN IF NOT EXISTS custom_priority TEXT;
                ALTER TABLE requirements ADD COLUMN IF NOT EXISTS ai_evaluation JSONB;
                ALTER TABLE users ADD COLUMN IF NOT EXISTS version INTEGER NOT NULL DEFAULT 1;
                ALTER TABLE metadata ADD COLUMN IF NOT EXISTS ai_prompts JSONB NOT NULL DEFAULT '{}';
                ALTER TABLE metadata ADD COLUMN IF NOT EXISTS baselines JSONB NOT NULL DEFAULT '[]';
                ALTER TABLE metadata ADD COLUMN IF NOT EXISTS teams JSONB NOT NULL DEFAULT '[]';
                ALTER TABLE metadata ADD COLUMN IF NOT EXISTS store_version INTEGER NOT NULL DEFAULT 1;
                UPDATE schema_version SET version = 2;
                "#,
            )?;
        }

        if from_version < 3 {
            client.batch_execute(
                r#"
                ALTER TABLE requirements ADD COLUMN IF NOT EXISTS trace_links JSONB NOT NULL DEFAULT '[]';
                ALTER TABLE requirements ADD COLUMN IF NOT EXISTS implementation_info JSONB;
                UPDATE schema_version SET version = 3;
                "#,
            )?;
        }

        if from_version < 4 {
            client.batch_execute(
                r#"
                ALTER TABLE users ADD COLUMN IF NOT EXISTS pin_hash TEXT;
                UPDATE schema_version SET version = 4;
                "#,
            )?;
        }

        if from_version < 5 {
            // trace:STORY-0325 | ai:claude
            client.batch_execute(
                r#"
                -- GitLab sync state table for tracking sync between AIDA and GitLab
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

                CREATE INDEX IF NOT EXISTS idx_gitlab_sync_requirement ON gitlab_sync_state(requirement_id);
                CREATE INDEX IF NOT EXISTS idx_gitlab_sync_issue ON gitlab_sync_state(gitlab_project_id, gitlab_issue_iid);
                CREATE INDEX IF NOT EXISTS idx_gitlab_sync_status ON gitlab_sync_state(sync_status);

                UPDATE schema_version SET version = 5;
                "#,
            )?;
        }

        // Migrate from version 5 to version 6 (add meta_subtype column)
        if from_version < 6 {
            client.batch_execute(
                r#"
                -- Add meta_subtype column for Meta requirements
                ALTER TABLE requirements ADD COLUMN IF NOT EXISTS meta_subtype TEXT;

                UPDATE schema_version SET version = 6;
                "#,
            )?;
        }

        // Migrate from version 6 to version 7 (add queue_entries table)
        // trace:STORY-0366 | ai:claude
        if from_version < 7 {
            client.batch_execute(
                r#"
                -- Queue entries table for personal work queue per user
                CREATE TABLE IF NOT EXISTS queue_entries (
                    user_id TEXT NOT NULL,
                    requirement_id UUID NOT NULL,
                    position INTEGER NOT NULL,
                    added_by TEXT NOT NULL,
                    note TEXT,
                    added_at TIMESTAMPTZ NOT NULL,
                    PRIMARY KEY (user_id, requirement_id)
                );

                CREATE INDEX IF NOT EXISTS idx_queue_user_position ON queue_entries(user_id, position);

                UPDATE schema_version SET version = 7;
                "#,
            )?;
        }

        if from_version < 8 {
            client.batch_execute(
                r#"
                ALTER TABLE requirements ADD COLUMN IF NOT EXISTS weight REAL;
                ALTER TABLE requirements ADD COLUMN IF NOT EXISTS attachments JSONB NOT NULL DEFAULT '[]';
                ALTER TABLE requirements ADD COLUMN IF NOT EXISTS gitlab_issues JSONB NOT NULL DEFAULT '[]';

                UPDATE schema_version SET version = 8;
                "#,
            )?;
        }

        Ok(())
    }

    /// Serializes complex types to JSON for storage
    fn to_json<T: serde::Serialize>(value: &T) -> Result<serde_json::Value> {
        serde_json::to_value(value).context("Failed to serialize to JSON")
    }

    /// Deserializes complex types from JSON storage
    fn from_json<T: serde::de::DeserializeOwned>(value: &serde_json::Value) -> Result<T> {
        serde_json::from_value(value.clone()).context("Failed to deserialize from JSON")
    }

    /// Converts a RequirementStatus to a string for storage
    fn status_to_str(status: &RequirementStatus) -> &'static str {
        match status {
            RequirementStatus::Draft => "Draft",
            RequirementStatus::Approved => "Approved",
            RequirementStatus::Planned => "Planned",
            RequirementStatus::InProgress => "In Progress",
            RequirementStatus::Done => "Done",
            RequirementStatus::Completed => "Completed",
            RequirementStatus::Rejected => "Rejected",
            RequirementStatus::NeedsAttention => "Needs Attention",
        }
    }

    /// Parses a RequirementStatus from a string
    fn str_to_status(s: &str) -> RequirementStatus {
        match s {
            "Draft" => RequirementStatus::Draft,
            "Approved" => RequirementStatus::Approved,
            "Planned" => RequirementStatus::Planned,
            "In Progress" => RequirementStatus::InProgress,
            "Done" => RequirementStatus::Done,
            "Completed" => RequirementStatus::Completed,
            "Rejected" => RequirementStatus::Rejected,
            "Needs Attention" => RequirementStatus::NeedsAttention,
            _ => RequirementStatus::Draft,
        }
    }

    /// Converts a RequirementPriority to a string for storage
    fn priority_to_str(priority: &RequirementPriority) -> &'static str {
        match priority {
            RequirementPriority::High => "High",
            RequirementPriority::Medium => "Medium",
            RequirementPriority::Low => "Low",
        }
    }

    /// Parses a RequirementPriority from a string
    fn str_to_priority(s: &str) -> RequirementPriority {
        match s {
            "High" => RequirementPriority::High,
            "Medium" => RequirementPriority::Medium,
            "Low" => RequirementPriority::Low,
            _ => RequirementPriority::Medium,
        }
    }

    /// Converts a RequirementType to a string for storage
    fn type_to_str(req_type: &RequirementType) -> &'static str {
        match req_type {
            RequirementType::Functional => "Functional",
            RequirementType::NonFunctional => "NonFunctional",
            RequirementType::System => "System",
            RequirementType::User => "User",
            RequirementType::ChangeRequest => "ChangeRequest",
            RequirementType::Bug => "Bug",
            RequirementType::Epic => "Epic",
            RequirementType::Story => "Story",
            RequirementType::Task => "Task",
            RequirementType::Spike => "Spike",
            RequirementType::Sprint => "Sprint",
            RequirementType::Folder => "Folder",
            RequirementType::Meta => "Meta",
            RequirementType::Principle => "Principle",
            RequirementType::Vision => "Vision",
            RequirementType::Constraint => "Constraint",
            RequirementType::Decision => "Decision",
            RequirementType::Term => "Term",
            RequirementType::Doc => "Doc",
        }
    }

    /// Parses a RequirementType from a string
    fn str_to_type(s: &str) -> RequirementType {
        match s {
            "Functional" => RequirementType::Functional,
            "NonFunctional" => RequirementType::NonFunctional,
            "System" => RequirementType::System,
            "User" => RequirementType::User,
            "ChangeRequest" => RequirementType::ChangeRequest,
            "Bug" => RequirementType::Bug,
            "Epic" => RequirementType::Epic,
            "Story" => RequirementType::Story,
            "Task" => RequirementType::Task,
            "Spike" => RequirementType::Spike,
            "Sprint" => RequirementType::Sprint,
            "Folder" => RequirementType::Folder,
            "Meta" => RequirementType::Meta,
            "Doc" => RequirementType::Doc,
            _ => RequirementType::Functional,
        }
    }

    /// Parse a requirement from a database row
    fn row_to_requirement(row: &Row) -> Result<Requirement> {
        let id: Uuid = row.get("id");
        let spec_id: Option<String> = row.get("spec_id");
        let prefix_override: Option<String> = row.get("prefix_override");
        let title: String = row.get("title");
        let description: String = row.get("description");
        let status_str: String = row.get("status");
        let priority_str: String = row.get("priority");
        let owner: String = row.get("owner");
        let feature: String = row.get("feature");
        let created_at: chrono::DateTime<chrono::Utc> = row.get("created_at");
        let created_by: Option<String> = row.get("created_by");
        let modified_at: chrono::DateTime<chrono::Utc> = row.get("modified_at");
        let req_type_str: String = row.get("req_type");
        let dependencies_json: serde_json::Value = row.get("dependencies");
        let tags_json: serde_json::Value = row.get("tags");
        let relationships_json: serde_json::Value = row.get("relationships");
        let comments_json: serde_json::Value = row.get("comments");
        let history_json: serde_json::Value = row.get("history");
        let archived: bool = row.get("archived");
        // STORY-441 | ai:claude — archived_at is nullable in postgres.
        let archived_at: Option<chrono::DateTime<chrono::Utc>> =
            row.try_get("archived_at").unwrap_or(None);
        let custom_status: Option<String> = row.get("custom_status");
        let custom_priority: Option<String> = row.get("custom_priority");
        let custom_fields_json: serde_json::Value = row.get("custom_fields");
        let urls_json: serde_json::Value = row.get("urls");
        let trace_links_json: serde_json::Value = row.get("trace_links");
        let weight: Option<f64> = row.get::<_, Option<f32>>("weight").map(|v| v as f64);
        let attachments_json: serde_json::Value = row.get("attachments");
        let gitlab_issues_json: serde_json::Value = row.get("gitlab_issues");
        let implementation_info_json: Option<serde_json::Value> = row.get("implementation_info");
        let ai_evaluation_json: Option<serde_json::Value> = row.get("ai_evaluation");
        let version: i64 = row.get::<_, i32>("version") as i64;

        let status = Self::str_to_status(&status_str);
        let priority = Self::str_to_priority(&priority_str);
        let req_type = Self::str_to_type(&req_type_str);
        let dependencies: Vec<Uuid> = Self::from_json(&dependencies_json).unwrap_or_default();
        let tags: HashSet<String> = Self::from_json(&tags_json).unwrap_or_default();
        let relationships: Vec<Relationship> =
            Self::from_json(&relationships_json).unwrap_or_default();
        let comments: Vec<Comment> = Self::from_json(&comments_json).unwrap_or_default();
        let history: Vec<HistoryEntry> = Self::from_json(&history_json).unwrap_or_default();
        let custom_fields: HashMap<String, String> =
            Self::from_json(&custom_fields_json).unwrap_or_default();
        let urls: Vec<UrlLink> = Self::from_json(&urls_json).unwrap_or_default();
        let trace_links: Vec<TraceLink> = Self::from_json(&trace_links_json).unwrap_or_default();
        let attachments: Vec<Attachment> = Self::from_json(&attachments_json).unwrap_or_default();
        let gitlab_issues: Vec<GitLabIssueLink> =
            Self::from_json(&gitlab_issues_json).unwrap_or_default();
        let implementation_info: Option<ImplementationInfo> =
            implementation_info_json.and_then(|json| Self::from_json(&json).ok());
        let ai_evaluation = ai_evaluation_json.and_then(|json| Self::from_json(&json).ok());

        Ok(Requirement {
            id,
            spec_id,
            agreed_id: None,
            prefix_override,
            title,
            description,
            status,
            priority,
            owner,
            // trace:STORY-639 | ai:claude — postgres backend does not persist assignee.
            assignee: None,
            feature,
            created_at,
            created_by,
            modified_at,
            req_type,
            meta_subtype: None, // Loaded separately if needed
            dependencies,
            tags,
            weight: weight.map(|w| w as f32),
            relationships,
            comments,
            history,
            // STORY-582: postgres backend does not persist processing records.
            processing_record: Vec::new(),
            archived,
            archived_at,
            // STORY-584: the deferred view-flag is not persisted by the
            // Postgres backend (opt-in, behind the `postgres` feature) — the
            // git-canonical backend is the source of truth for it.
            // trace:STORY-584 | ai:claude
            deferred: false,
            deferred_at: None,
            deferred_until: None,
            // trace:TASK-1148 | ai:claude — narrative fields not carried by legacy backend
            risk_notes: None,
            test_coverage_notes: None,
            implementation_summary: None,
            execution_mode: None,
            // trace:STORY-634 | ai:claude — origin not carried by legacy backend
            origin: None,
            custom_status,
            custom_priority,
            custom_fields,
            urls,
            attachments,
            trace_links,
            gitlab_issues,
            // STORY-476: external refs are not persisted by the Postgres
            // backend (opt-in, behind the `postgres` feature).
            // trace:STORY-476 | ai:claude
            external_refs: Vec::new(),
            implementation_info,
            ai_evaluation,
            // STORY-332: punt metadata is not persisted by the Postgres
            // backend (opt-in, behind the `postgres` feature).
            attention_reason: None,
            // EPIC-28: orchestrator-shelving metadata is not persisted
            // by the Postgres backend (opt-in, behind the `postgres`
            // feature). trace:EPIC-28 | ai:claude
            failure_reason: None,
            // STORY-333: the human-only marker is not persisted by the
            // Postgres backend (opt-in, behind the `postgres` feature).
            // trace:STORY-333 | ai:claude
            human_only: false,
            // trace:STORY-522 | ai:claude
            decision_request: None,
            // trace:STORY-542 | ai:claude
            interface_changes: None,
            // trace:STORY-631 | ai:claude
            intent: None,
            version,
        })
    }

    /// Save a requirement to the database
    fn save_requirement<C: GenericClient>(&self, client: &mut C, req: &Requirement) -> Result<()> {
        let ai_eval_json = req.ai_evaluation.as_ref().map(Self::to_json).transpose()?;
        let impl_info_json = req
            .implementation_info
            .as_ref()
            .map(Self::to_json)
            .transpose()?;

        client.execute(
            "INSERT INTO requirements
             (id, spec_id, prefix_override, title, description, status, priority, owner, feature,
              created_at, created_by, modified_at, req_type, dependencies, tags, relationships,
              comments, history, archived, archived_at, custom_status, custom_priority, custom_fields, urls,
              trace_links, implementation_info, ai_evaluation, weight, attachments, gitlab_issues, version)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31)
             ON CONFLICT (id) DO UPDATE SET
              spec_id = EXCLUDED.spec_id,
              prefix_override = EXCLUDED.prefix_override,
              title = EXCLUDED.title,
              description = EXCLUDED.description,
              status = EXCLUDED.status,
              priority = EXCLUDED.priority,
              owner = EXCLUDED.owner,
              feature = EXCLUDED.feature,
              created_at = EXCLUDED.created_at,
              created_by = EXCLUDED.created_by,
              modified_at = EXCLUDED.modified_at,
              req_type = EXCLUDED.req_type,
              dependencies = EXCLUDED.dependencies,
              tags = EXCLUDED.tags,
              relationships = EXCLUDED.relationships,
              comments = EXCLUDED.comments,
              history = EXCLUDED.history,
              archived = EXCLUDED.archived,
              archived_at = EXCLUDED.archived_at,
              custom_status = EXCLUDED.custom_status,
              custom_priority = EXCLUDED.custom_priority,
              custom_fields = EXCLUDED.custom_fields,
              urls = EXCLUDED.urls,
              trace_links = EXCLUDED.trace_links,
              implementation_info = EXCLUDED.implementation_info,
              ai_evaluation = EXCLUDED.ai_evaluation,
              weight = EXCLUDED.weight,
              attachments = EXCLUDED.attachments,
              gitlab_issues = EXCLUDED.gitlab_issues,
              version = EXCLUDED.version",
            &[
                &req.id,
                &req.spec_id,
                &req.prefix_override,
                &req.title,
                &req.description,
                &Self::status_to_str(&req.status),
                &Self::priority_to_str(&req.priority),
                &req.owner,
                &req.feature,
                &req.created_at,
                &req.created_by,
                &req.modified_at,
                &Self::type_to_str(&req.req_type),
                &Self::to_json(&req.dependencies)?,
                &Self::to_json(&req.tags)?,
                &Self::to_json(&req.relationships)?,
                &Self::to_json(&req.comments)?,
                &Self::to_json(&req.history)?,
                &req.archived,
                &req.archived_at,
                &req.custom_status,
                &req.custom_priority,
                &Self::to_json(&req.custom_fields)?,
                &Self::to_json(&req.urls)?,
                &Self::to_json(&req.trace_links)?,
                &impl_info_json,
                &ai_eval_json,
                &(req.weight.map(|w| w as f64)),
                &Self::to_json(&req.attachments)?,
                &Self::to_json(&req.gitlab_issues)?,
                &(req.version as i32),
            ],
        )?;
        Ok(())
    }

    /// Save a user to the database
    fn save_user<C: GenericClient>(&self, client: &mut C, user: &User) -> Result<()> {
        client.execute(
            "INSERT INTO users (id, spec_id, name, email, handle, pin_hash, created_at, archived, version)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (id) DO UPDATE SET
              spec_id = EXCLUDED.spec_id,
              name = EXCLUDED.name,
              email = EXCLUDED.email,
              handle = EXCLUDED.handle,
              pin_hash = EXCLUDED.pin_hash,
              created_at = EXCLUDED.created_at,
              archived = EXCLUDED.archived,
              version = EXCLUDED.version",
            &[
                &user.id,
                &user.spec_id,
                &user.name,
                &user.email,
                &user.handle,
                &user.pin_hash,
                &user.created_at,
                &user.archived,
                &(user.version as i32),
            ],
        )?;
        Ok(())
    }

    /// Save metadata to the database
    fn save_metadata<C: GenericClient>(
        &self,
        client: &mut C,
        store: &RequirementsStore,
    ) -> Result<()> {
        client.execute(
            "INSERT INTO metadata
             (id, name, title, description, id_config, features, next_feature_number, next_spec_number,
              prefix_counters, relationship_definitions, reaction_definitions, meta_counters,
              type_definitions, allowed_prefixes, restrict_prefixes, ai_prompts, baselines, teams, store_version)
             VALUES (1, $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
             ON CONFLICT (id) DO UPDATE SET
              name = EXCLUDED.name,
              title = EXCLUDED.title,
              description = EXCLUDED.description,
              id_config = EXCLUDED.id_config,
              features = EXCLUDED.features,
              next_feature_number = EXCLUDED.next_feature_number,
              next_spec_number = EXCLUDED.next_spec_number,
              prefix_counters = EXCLUDED.prefix_counters,
              relationship_definitions = EXCLUDED.relationship_definitions,
              reaction_definitions = EXCLUDED.reaction_definitions,
              meta_counters = EXCLUDED.meta_counters,
              type_definitions = EXCLUDED.type_definitions,
              allowed_prefixes = EXCLUDED.allowed_prefixes,
              restrict_prefixes = EXCLUDED.restrict_prefixes,
              ai_prompts = EXCLUDED.ai_prompts,
              baselines = EXCLUDED.baselines,
              teams = EXCLUDED.teams,
              store_version = EXCLUDED.store_version",
            &[
                &store.name,
                &store.title,
                &store.description,
                &Self::to_json(&store.id_config)?,
                &Self::to_json(&store.features)?,
                &(store.next_feature_number as i32),
                &(store.next_spec_number as i32),
                &Self::to_json(&store.prefix_counters)?,
                &Self::to_json(&store.relationship_definitions)?,
                &Self::to_json(&store.reaction_definitions)?,
                &Self::to_json(&store.meta_counters)?,
                &Self::to_json(&store.type_definitions)?,
                &Self::to_json(&store.allowed_prefixes)?,
                &store.restrict_prefixes,
                &Self::to_json(&store.ai_prompts)?,
                &Self::to_json(&store.baselines)?,
                &Self::to_json(&store.teams)?,
                &(store.store_version as i32),
            ],
        )?;
        Ok(())
    }

    /// Load requirements from database
    fn load_requirements<C: GenericClient>(&self, client: &mut C) -> Result<Vec<Requirement>> {
        let rows = client.query(
            "SELECT id, spec_id, prefix_override, title, description, status, priority,
                    owner, feature, created_at, created_by, modified_at, req_type,
                    dependencies, tags, relationships, comments, history, archived, archived_at,
                    custom_status, custom_priority, custom_fields, urls, trace_links,
                    implementation_info, ai_evaluation, weight, attachments, gitlab_issues, version
             FROM requirements ORDER BY created_at",
            &[],
        )?;

        rows.iter().map(Self::row_to_requirement).collect()
    }

    /// Load users from database
    fn load_users<C: GenericClient>(&self, client: &mut C) -> Result<Vec<User>> {
        let rows = client.query(
            "SELECT id, spec_id, name, email, handle, pin_hash, created_at, archived, version FROM users",
            &[],
        )?;

        let mut users = Vec::new();
        for row in rows {
            let id: Uuid = row.get("id");
            let spec_id: Option<String> = row.get("spec_id");
            let name: String = row.get("name");
            let email: String = row.get("email");
            let handle: String = row.get("handle");
            let pin_hash: Option<String> = row.get("pin_hash");
            let created_at: chrono::DateTime<chrono::Utc> = row.get("created_at");
            let archived: bool = row.get("archived");
            let version: i64 = row.get::<_, i32>("version") as i64;

            users.push(User {
                id,
                spec_id,
                name,
                email,
                handle,
                pin_hash,
                created_at,
                archived,
                version,
            });
        }

        Ok(users)
    }

    /// Load metadata from database
    // why: private one-shot loader returning the metadata column tuple; a named alias would only be used here and the SELECT column order is the documentation.
    #[allow(clippy::type_complexity)]
    fn load_metadata<C: GenericClient>(
        &self,
        client: &mut C,
    ) -> Result<(
        String,
        String,
        String,
        IdConfiguration,
        u32,
        u32,
        HashMap<String, u32>,
        HashMap<String, u32>,
    )> {
        let row = client.query_opt(
            "SELECT name, title, description, id_config, next_feature_number, next_spec_number, prefix_counters, meta_counters
             FROM metadata WHERE id = 1",
            &[],
        )?;

        match row {
            Some(row) => {
                let name: String = row.get("name");
                let title: String = row.get("title");
                let description: String = row.get("description");
                let id_config_json: serde_json::Value = row.get("id_config");
                let next_feature_number: i32 = row.get("next_feature_number");
                let next_spec_number: i32 = row.get("next_spec_number");
                let prefix_counters_json: serde_json::Value = row.get("prefix_counters");
                let meta_counters_json: serde_json::Value = row.get("meta_counters");

                let id_config: IdConfiguration =
                    Self::from_json(&id_config_json).unwrap_or_default();
                let prefix_counters: HashMap<String, u32> =
                    Self::from_json(&prefix_counters_json).unwrap_or_default();
                let meta_counters: HashMap<String, u32> =
                    Self::from_json(&meta_counters_json).unwrap_or_default();

                Ok((
                    name,
                    title,
                    description,
                    id_config,
                    next_feature_number as u32,
                    next_spec_number as u32,
                    prefix_counters,
                    meta_counters,
                ))
            }
            None => Ok((
                String::new(),
                String::new(),
                String::new(),
                IdConfiguration::default(),
                1,
                1,
                HashMap::new(),
                HashMap::new(),
            )),
        }
    }

    /// Load features from database
    fn load_features<C: GenericClient>(&self, client: &mut C) -> Result<Vec<FeatureDefinition>> {
        let row = client.query_opt("SELECT features FROM metadata WHERE id = 1", &[])?;
        match row {
            Some(row) => {
                let json: serde_json::Value = row.get("features");
                Self::from_json(&json)
            }
            None => Ok(Vec::new()),
        }
    }

    /// Load type definitions from database
    fn load_type_definitions<C: GenericClient>(
        &self,
        client: &mut C,
    ) -> Result<Vec<CustomTypeDefinition>> {
        let row = client.query_opt("SELECT type_definitions FROM metadata WHERE id = 1", &[])?;
        match row {
            Some(row) => {
                let json: serde_json::Value = row.get("type_definitions");
                let defs: Vec<CustomTypeDefinition> = Self::from_json(&json)?;
                if defs.is_empty() {
                    Ok(crate::models::default_type_definitions())
                } else {
                    Ok(defs)
                }
            }
            None => Ok(crate::models::default_type_definitions()),
        }
    }

    /// Load relationship definitions from database
    fn load_relationship_definitions<C: GenericClient>(
        &self,
        client: &mut C,
    ) -> Result<Vec<RelationshipDefinition>> {
        let row = client.query_opt(
            "SELECT relationship_definitions FROM metadata WHERE id = 1",
            &[],
        )?;
        match row {
            Some(row) => {
                let json: serde_json::Value = row.get("relationship_definitions");
                let defs: Vec<RelationshipDefinition> = Self::from_json(&json)?;
                if defs.is_empty() {
                    Ok(RelationshipDefinition::defaults())
                } else {
                    Ok(defs)
                }
            }
            None => Ok(RelationshipDefinition::defaults()),
        }
    }

    /// Load reaction definitions from database
    fn load_reaction_definitions<C: GenericClient>(
        &self,
        client: &mut C,
    ) -> Result<Vec<ReactionDefinition>> {
        let row = client.query_opt(
            "SELECT reaction_definitions FROM metadata WHERE id = 1",
            &[],
        )?;
        match row {
            Some(row) => {
                let json: serde_json::Value = row.get("reaction_definitions");
                let defs: Vec<ReactionDefinition> = Self::from_json(&json)?;
                if defs.is_empty() {
                    Ok(crate::models::default_reaction_definitions())
                } else {
                    Ok(defs)
                }
            }
            None => Ok(crate::models::default_reaction_definitions()),
        }
    }

    /// Load allowed prefixes from database
    fn load_allowed_prefixes<C: GenericClient>(
        &self,
        client: &mut C,
    ) -> Result<(Vec<String>, bool)> {
        let row = client.query_opt(
            "SELECT allowed_prefixes, restrict_prefixes FROM metadata WHERE id = 1",
            &[],
        )?;

        match row {
            Some(row) => {
                let json: serde_json::Value = row.get("allowed_prefixes");
                let restrict: bool = row.get("restrict_prefixes");
                let prefixes: Vec<String> = Self::from_json(&json).unwrap_or_default();
                Ok((prefixes, restrict))
            }
            None => Ok((Vec::new(), false)),
        }
    }

    /// Load store_version from metadata
    fn load_store_version<C: GenericClient>(&self, client: &mut C) -> Result<i64> {
        let row = client.query_opt("SELECT store_version FROM metadata WHERE id = 1", &[])?;
        Ok(row
            .map(|r| r.get::<_, i32>("store_version") as i64)
            .unwrap_or(1))
    }

    /// Load ai_prompts from metadata
    fn load_ai_prompts<C: GenericClient>(
        &self,
        client: &mut C,
    ) -> Result<crate::models::AiPromptConfig> {
        let row = client.query_opt("SELECT ai_prompts FROM metadata WHERE id = 1", &[])?;
        match row {
            Some(row) => {
                let json: serde_json::Value = row.get("ai_prompts");
                Self::from_json(&json).or_else(|_| Ok(crate::models::AiPromptConfig::default()))
            }
            None => Ok(crate::models::AiPromptConfig::default()),
        }
    }

    /// Load baselines from metadata
    fn load_baselines<C: GenericClient>(
        &self,
        client: &mut C,
    ) -> Result<Vec<crate::models::Baseline>> {
        let row = client.query_opt("SELECT baselines FROM metadata WHERE id = 1", &[])?;
        match row {
            Some(row) => {
                let json: serde_json::Value = row.get("baselines");
                Self::from_json(&json)
            }
            None => Ok(Vec::new()),
        }
    }

    /// Load teams from metadata
    fn load_teams<C: GenericClient>(&self, client: &mut C) -> Result<Vec<crate::models::Team>> {
        let row = client.query_opt("SELECT teams FROM metadata WHERE id = 1", &[])?;
        match row {
            Some(row) => {
                let json: serde_json::Value = row.get("teams");
                Self::from_json(&json)
            }
            None => Ok(Vec::new()),
        }
    }

    // ==================== GitLab Sync State Operations (STORY-0325) ====================

    /// Save or update a GitLab sync state
    /// trace:STORY-0325 | ai:claude
    pub fn save_sync_state(&self, state: &crate::models::GitLabSyncState) -> Result<()> {
        let mut client = self
            .pool
            .get()
            .context("Failed to get connection from pool")?;
        client.execute(
            r#"INSERT INTO gitlab_sync_state
               (requirement_id, spec_id, gitlab_project_id, gitlab_issue_iid, gitlab_issue_id,
                linked_at, last_sync, aida_content_hash, gitlab_content_hash,
                link_origin, sync_status, last_error)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
               ON CONFLICT (requirement_id, gitlab_issue_iid) DO UPDATE SET
                spec_id = EXCLUDED.spec_id,
                last_sync = EXCLUDED.last_sync,
                aida_content_hash = EXCLUDED.aida_content_hash,
                gitlab_content_hash = EXCLUDED.gitlab_content_hash,
                sync_status = EXCLUDED.sync_status,
                last_error = EXCLUDED.last_error"#,
            &[
                &state.requirement_id,
                &state.spec_id,
                &(state.gitlab_project_id as i64),
                &(state.gitlab_issue_iid as i64),
                &(state.gitlab_issue_id as i64),
                &state.linked_at,
                &state.last_sync,
                &state.aida_content_hash,
                &state.gitlab_content_hash,
                &format!("{:?}", state.link_origin),
                &format!("{:?}", state.sync_status),
                &state.last_error,
            ],
        )?;
        Ok(())
    }

    /// Load sync state for a specific requirement and issue
    /// trace:STORY-0325 | ai:claude
    pub fn load_sync_state(
        &self,
        requirement_id: Uuid,
        issue_iid: u64,
    ) -> Result<Option<crate::models::GitLabSyncState>> {
        let mut client = self
            .pool
            .get()
            .context("Failed to get connection from pool")?;
        let row = client.query_opt(
            r#"SELECT requirement_id, spec_id, gitlab_project_id, gitlab_issue_iid, gitlab_issue_id,
                      linked_at, last_sync, aida_content_hash, gitlab_content_hash,
                      link_origin, sync_status, last_error
               FROM gitlab_sync_state WHERE requirement_id = $1 AND gitlab_issue_iid = $2"#,
            &[&requirement_id, &(issue_iid as i64)],
        )?;

        match row {
            Some(row) => Ok(Some(Self::row_to_sync_state(&row)?)),
            None => Ok(None),
        }
    }

    /// Load all sync states for a requirement
    /// trace:STORY-0325 | ai:claude
    pub fn load_sync_states_for_requirement(
        &self,
        requirement_id: Uuid,
    ) -> Result<Vec<crate::models::GitLabSyncState>> {
        let mut client = self
            .pool
            .get()
            .context("Failed to get connection from pool")?;
        let rows = client.query(
            r#"SELECT requirement_id, spec_id, gitlab_project_id, gitlab_issue_iid, gitlab_issue_id,
                      linked_at, last_sync, aida_content_hash, gitlab_content_hash,
                      link_origin, sync_status, last_error
               FROM gitlab_sync_state WHERE requirement_id = $1"#,
            &[&requirement_id],
        )?;

        rows.iter().map(Self::row_to_sync_state).collect()
    }

    /// Load all sync states
    /// trace:STORY-0325 | ai:claude
    pub fn load_all_sync_states(&self) -> Result<Vec<crate::models::GitLabSyncState>> {
        let mut client = self
            .pool
            .get()
            .context("Failed to get connection from pool")?;
        let rows = client.query(
            r#"SELECT requirement_id, spec_id, gitlab_project_id, gitlab_issue_iid, gitlab_issue_id,
                      linked_at, last_sync, aida_content_hash, gitlab_content_hash,
                      link_origin, sync_status, last_error
               FROM gitlab_sync_state ORDER BY last_sync DESC"#,
            &[],
        )?;

        rows.iter().map(Self::row_to_sync_state).collect()
    }

    /// Load sync states by status
    /// trace:STORY-0325 | ai:claude
    pub fn load_sync_states_by_status(
        &self,
        status: crate::models::SyncStatus,
    ) -> Result<Vec<crate::models::GitLabSyncState>> {
        let mut client = self
            .pool
            .get()
            .context("Failed to get connection from pool")?;
        let rows = client.query(
            r#"SELECT requirement_id, spec_id, gitlab_project_id, gitlab_issue_iid, gitlab_issue_id,
                      linked_at, last_sync, aida_content_hash, gitlab_content_hash,
                      link_origin, sync_status, last_error
               FROM gitlab_sync_state WHERE sync_status = $1 ORDER BY last_sync DESC"#,
            &[&format!("{:?}", status)],
        )?;

        rows.iter().map(Self::row_to_sync_state).collect()
    }

    /// Delete a sync state
    /// trace:STORY-0325 | ai:claude
    pub fn delete_sync_state(&self, requirement_id: Uuid, issue_iid: u64) -> Result<bool> {
        let mut client = self
            .pool
            .get()
            .context("Failed to get connection from pool")?;
        let rows_affected = client.execute(
            "DELETE FROM gitlab_sync_state WHERE requirement_id = $1 AND gitlab_issue_iid = $2",
            &[&requirement_id, &(issue_iid as i64)],
        )?;
        Ok(rows_affected > 0)
    }

    /// Helper to convert a row to GitLabSyncState
    fn row_to_sync_state(row: &postgres::Row) -> Result<crate::models::GitLabSyncState> {
        use crate::models::{GitLabSyncState, LinkOrigin, SyncStatus};

        let link_origin_str: String = row.get("link_origin");
        let sync_status_str: String = row.get("sync_status");

        Ok(GitLabSyncState {
            requirement_id: row.get("requirement_id"),
            spec_id: row.get("spec_id"),
            gitlab_project_id: row.get::<_, i64>("gitlab_project_id") as u64,
            gitlab_issue_iid: row.get::<_, i64>("gitlab_issue_iid") as u64,
            gitlab_issue_id: row.get::<_, i64>("gitlab_issue_id") as u64,
            linked_at: row.get("linked_at"),
            last_sync: row.get("last_sync"),
            aida_content_hash: row.get("aida_content_hash"),
            gitlab_content_hash: row.get("gitlab_content_hash"),
            link_origin: match link_origin_str.as_str() {
                "CreatedFromAida" => LinkOrigin::CreatedFromAida,
                "ImportedFromGitLab" => LinkOrigin::ImportedFromGitLab,
                _ => LinkOrigin::ManualLink,
            },
            sync_status: match sync_status_str.as_str() {
                "InSync" => SyncStatus::InSync,
                "AidaModified" => SyncStatus::AidaModified,
                "GitLabModified" => SyncStatus::GitLabModified,
                "Conflict" => SyncStatus::Conflict,
                "Error" => SyncStatus::Error,
                _ => SyncStatus::Untracked,
            },
            last_error: row.get("last_error"),
        })
    }
}

impl DatabaseBackend for PostgresBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::Postgres
    }

    fn path(&self) -> &Path {
        &self.connection_string
    }

    fn load(&self) -> Result<RequirementsStore> {
        let mut client = self
            .pool
            .get()
            .context("Failed to get connection from pool")?;

        let requirements = self.load_requirements(&mut *client)?;
        let users = self.load_users(&mut *client)?;
        let (
            name,
            title,
            description,
            id_config,
            next_feature_number,
            next_spec_number,
            prefix_counters,
            meta_counters,
        ) = self.load_metadata(&mut *client)?;
        let features = self.load_features(&mut *client)?;
        let type_definitions = self.load_type_definitions(&mut *client)?;
        let relationship_definitions = self.load_relationship_definitions(&mut *client)?;
        let reaction_definitions = self.load_reaction_definitions(&mut *client)?;
        let (allowed_prefixes, restrict_prefixes) = self.load_allowed_prefixes(&mut *client)?;
        let ai_prompts = self.load_ai_prompts(&mut *client)?;
        let baselines = self.load_baselines(&mut *client)?;
        let teams = self.load_teams(&mut *client)?;
        let store_version = self.load_store_version(&mut *client)?;

        Ok(RequirementsStore {
            name,
            title,
            description,
            requirements,
            users,
            teams,
            id_config,
            features,
            next_feature_number,
            next_spec_number,
            prefix_counters,
            relationship_definitions,
            reaction_definitions,
            meta_counters,
            type_definitions,
            allowed_prefixes,
            restrict_prefixes,
            ai_prompts,
            baselines,
            store_version,
            migrated_to: None,
            dispenser: None,
        })
    }

    fn save(&self, store: &RequirementsStore) -> Result<()> {
        let mut client = self
            .pool
            .get()
            .context("Failed to get connection from pool")?;

        // Use a transaction for atomicity
        let mut transaction = client.transaction()?;

        // Clear existing data
        transaction.execute("DELETE FROM requirements", &[])?;
        transaction.execute("DELETE FROM users", &[])?;

        // Save all requirements
        for req in &store.requirements {
            self.save_requirement(&mut transaction, req)?;
        }

        // Save all users
        for user in &store.users {
            self.save_user(&mut transaction, user)?;
        }

        // Save metadata
        self.save_metadata(&mut transaction, store)?;

        transaction.commit()?;
        Ok(())
    }

    fn update_atomically<F>(&self, update_fn: F) -> Result<RequirementsStore>
    where
        F: FnOnce(&mut RequirementsStore),
    {
        let mut client = self
            .pool
            .get()
            .context("Failed to get connection from pool")?;

        let mut transaction = client.transaction()?;

        // Load within transaction
        let requirements = self.load_requirements(&mut transaction)?;
        let users = self.load_users(&mut transaction)?;
        let (
            name,
            title,
            description,
            id_config,
            next_feature_number,
            next_spec_number,
            prefix_counters,
            meta_counters,
        ) = self.load_metadata(&mut transaction)?;
        let features = self.load_features(&mut transaction)?;
        let type_definitions = self.load_type_definitions(&mut transaction)?;
        let relationship_definitions = self.load_relationship_definitions(&mut transaction)?;
        let reaction_definitions = self.load_reaction_definitions(&mut transaction)?;
        let (allowed_prefixes, restrict_prefixes) = self.load_allowed_prefixes(&mut transaction)?;
        let ai_prompts = self.load_ai_prompts(&mut transaction)?;
        let baselines = self.load_baselines(&mut transaction)?;
        let teams = self.load_teams(&mut transaction)?;
        let store_version = self.load_store_version(&mut transaction)?;

        let mut store = RequirementsStore {
            name,
            title,
            description,
            requirements,
            users,
            teams,
            id_config,
            features,
            next_feature_number,
            next_spec_number,
            prefix_counters,
            relationship_definitions,
            reaction_definitions,
            meta_counters,
            type_definitions,
            allowed_prefixes,
            restrict_prefixes,
            ai_prompts,
            baselines,
            store_version,
            migrated_to: None,
            dispenser: None,
        };

        // Apply changes
        update_fn(&mut store);

        // Clear and save
        transaction.execute("DELETE FROM requirements", &[])?;
        transaction.execute("DELETE FROM users", &[])?;

        for req in &store.requirements {
            self.save_requirement(&mut transaction, req)?;
        }

        for user in &store.users {
            self.save_user(&mut transaction, user)?;
        }

        self.save_metadata(&mut transaction, &store)?;

        transaction.commit()?;
        Ok(store)
    }

    fn get_requirement(&self, id: &Uuid) -> Result<Option<Requirement>> {
        let mut client = self
            .pool
            .get()
            .context("Failed to get connection from pool")?;

        let row = client.query_opt(
            "SELECT id, spec_id, prefix_override, title, description, status, priority,
                    owner, feature, created_at, created_by, modified_at, req_type,
                    dependencies, tags, relationships, comments, history, archived, archived_at,
                    custom_status, custom_priority, custom_fields, urls, trace_links,
                    implementation_info, ai_evaluation, version
             FROM requirements WHERE id = $1",
            &[id],
        )?;

        match row {
            Some(row) => Ok(Some(Self::row_to_requirement(&row)?)),
            None => Ok(None),
        }
    }

    fn get_requirement_by_spec_id(&self, spec_id: &str) -> Result<Option<Requirement>> {
        let spec_id = crate::object_store::canonical_spec_id(spec_id);
        let mut client = self
            .pool
            .get()
            .context("Failed to get connection from pool")?;

        let row = client.query_opt(
            "SELECT id, spec_id, prefix_override, title, description, status, priority,
                    owner, feature, created_at, created_by, modified_at, req_type,
                    dependencies, tags, relationships, comments, history, archived, archived_at,
                    custom_status, custom_priority, custom_fields, urls, trace_links,
                    implementation_info, ai_evaluation, version
             FROM requirements WHERE spec_id = $1",
            &[&spec_id],
        )?;

        match row {
            Some(row) => Ok(Some(Self::row_to_requirement(&row)?)),
            None => Ok(None),
        }
    }

    fn update_requirement(&self, requirement: &Requirement) -> Result<()> {
        let mut client = self
            .pool
            .get()
            .context("Failed to get connection from pool")?;
        self.save_requirement(&mut *client, requirement)
    }

    fn update_requirement_versioned(&self, requirement: &Requirement) -> Result<UpdateResult> {
        let mut client = self
            .pool
            .get()
            .context("Failed to get connection from pool")?;

        // Get current version from database
        let current_version: Option<i32> = client
            .query_opt(
                "SELECT version FROM requirements WHERE id = $1",
                &[&requirement.id],
            )?
            .map(|row| row.get("version"));

        match current_version {
            Some(db_version) => {
                // Check for version conflict
                if db_version as i64 != requirement.version {
                    return Ok(UpdateResult::Conflict(VersionConflict {
                        id: requirement.id,
                        expected_version: requirement.version,
                        current_version: db_version as i64,
                        display_id: requirement
                            .spec_id
                            .clone()
                            .unwrap_or_else(|| requirement.id.to_string()),
                    }));
                }

                // Version matches - update with incremented version
                let mut updated_req = requirement.clone();
                updated_req.version = db_version as i64 + 1;
                self.save_requirement(&mut *client, &updated_req)?;
                Ok(UpdateResult::Success)
            }
            None => {
                // Requirement doesn't exist - create it
                self.save_requirement(&mut *client, requirement)?;
                Ok(UpdateResult::Success)
            }
        }
    }

    fn exists(&self) -> bool {
        // For PostgreSQL, we check if we can connect and if the schema exists
        if let Ok(mut client) = self.pool.get() {
            client
                .query_opt(
                    "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_name = 'requirements')",
                    &[],
                )
                .ok()
                .and_then(|row| row.map(|r| r.get::<_, bool>(0)))
                .unwrap_or(false)
        } else {
            false
        }
    }

    fn create_if_not_exists(&self) -> Result<()> {
        // Schema is created in init_schema during construction
        // Just verify connection works
        let _client = self
            .pool
            .get()
            .context("Failed to get connection from pool")?;
        Ok(())
    }

    // =========================================================================
    // Queue Operations (STORY-0366)
    // =========================================================================
    // trace:STORY-0366 | ai:claude

    fn queue_list(&self, user_id: &str, include_completed: bool) -> Result<Vec<QueueEntry>> {
        let mut client = self.pool.get().context("Failed to get connection")?;
        let rows = if include_completed {
            client.query(
                "SELECT q.user_id, q.requirement_id, q.position, q.added_by, q.note, q.added_at \
                 FROM queue_entries q \
                 WHERE q.user_id = $1 \
                 ORDER BY q.position ASC",
                &[&user_id],
            )?
        } else {
            client.query(
                "SELECT q.user_id, q.requirement_id, q.position, q.added_by, q.note, q.added_at \
                 FROM queue_entries q \
                 LEFT JOIN requirements r ON q.requirement_id = r.id \
                 WHERE q.user_id = $1 AND (r.status IS NULL OR r.status != 'Completed') \
                 ORDER BY q.position ASC",
                &[&user_id],
            )?
        };

        let entries = rows
            .iter()
            .map(|row| QueueEntry {
                user_id: row.get(0),
                requirement_id: row.get(1),
                position: row.get::<_, i32>(2) as i64,
                added_by: row.get(3),
                note: row.get(4),
                added_at: row.get(5),
                // Postgres queue_entries schema predates role routing
                // (EPIC-1-001) and scope/session routing (STORY-57).
                // Always None; git-canonical mode supports them.
                for_role: None,
                for_scope: None,
                for_session: None,
                added_by_machine: None,
            })
            .collect();

        Ok(entries)
    }

    fn queue_add(&self, entry: QueueEntry) -> Result<()> {
        let mut client = self.pool.get().context("Failed to get connection")?;

        let position = if entry.position == i64::MAX {
            let row = client.query_one(
                "SELECT COALESCE(MAX(position), 0) FROM queue_entries WHERE user_id = $1",
                &[&entry.user_id],
            )?;
            let max_pos: i32 = row.get(0);
            (max_pos as i64) + 1000
        } else {
            entry.position
        };

        client.execute(
            "INSERT INTO queue_entries (user_id, requirement_id, position, added_by, note, added_at) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (user_id, requirement_id) DO UPDATE SET position = $3, note = $5",
            &[
                &entry.user_id,
                &entry.requirement_id,
                &(position as i32),
                &entry.added_by,
                &entry.note,
                &entry.added_at,
            ],
        )?;
        Ok(())
    }

    fn queue_remove(&self, user_id: &str, requirement_id: &Uuid) -> Result<()> {
        let mut client = self.pool.get().context("Failed to get connection")?;
        client.execute(
            "DELETE FROM queue_entries WHERE user_id = $1 AND requirement_id = $2",
            &[&user_id, requirement_id],
        )?;
        Ok(())
    }

    fn queue_reorder(&self, user_id: &str, items: &[(Uuid, i64)]) -> Result<()> {
        let mut client = self.pool.get().context("Failed to get connection")?;
        let mut tx = client.transaction()?;
        for (req_id, position) in items {
            tx.execute(
                "UPDATE queue_entries SET position = $1 WHERE user_id = $2 AND requirement_id = $3",
                &[&(*position as i32), &user_id, req_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn queue_clear(&self, user_id: &str, completed_only: bool) -> Result<()> {
        let mut client = self.pool.get().context("Failed to get connection")?;
        if completed_only {
            client.execute(
                "DELETE FROM queue_entries WHERE user_id = $1 AND requirement_id IN \
                 (SELECT id FROM requirements WHERE status = 'Completed')",
                &[&user_id],
            )?;
        } else {
            client.execute("DELETE FROM queue_entries WHERE user_id = $1", &[&user_id])?;
        }
        Ok(())
    }
}
