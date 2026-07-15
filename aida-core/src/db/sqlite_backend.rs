//! **ARCHIVED — legacy backend, do not use in new code.**
//!
//! Pre-EPIC-1-001 SQLite-canonical storage. Kept compilable so the
//! archived `aida db migrate` and `aida db export-git` commands can
//! still run one-shot migrations off the legacy path. The kernel's
//! canonical store is the orphan-branch git backend
//! (`db::cached_git_backend::CachedGitBackend`). This file should not
//! gain new features; eventual full removal is tracked by FR-1-076.
//!
//! trace:FR-1-076 | ai:claude

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

use crate::models::{
    Attachment, Comment, CustomTypeDefinition, FeatureDefinition, GitLabIssueLink, HistoryEntry,
    IdConfiguration, ImplementationInfo, QueueEntry, ReactionDefinition, Relationship,
    RelationshipDefinition, Requirement, RequirementPriority, RequirementStatus, RequirementType,
    RequirementsStore, TraceLink, UrlLink, User,
};

use super::traits::{BackendType, DatabaseBackend};

/// Current schema version - updated to 8 for requirement weight/attachments/gitlab issues
const SCHEMA_VERSION: i32 = 8;

/// SQLite backend implementation
pub struct SqliteBackend {
    path: PathBuf,
    conn: Mutex<Connection>,
}

impl SqliteBackend {
    /// Creates a new SQLite backend
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&path)?;

        // Enable WAL mode for better concurrent access
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

        let backend = Self {
            path,
            conn: Mutex::new(conn),
        };

        backend.init_schema()?;
        Ok(backend)
    }

    /// Initialize the database schema
    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Check current schema version
        let current_version: i32 = conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);

        if current_version == 0 {
            // Create initial schema
            conn.execute_batch(include_str!("schema.sql"))?;
        } else if current_version < SCHEMA_VERSION {
            // Handle migrations
            Self::migrate_schema(&conn, current_version)?;
        }

        Ok(())
    }

    /// Migrate schema from old version to current
    fn migrate_schema(conn: &Connection, from_version: i32) -> Result<()> {
        if from_version < 2 {
            // Migration from v1 to v2: Add version columns for optimistic locking
            conn.execute_batch(
                r#"
                -- Add version column to requirements for optimistic locking
                ALTER TABLE requirements ADD COLUMN version INTEGER NOT NULL DEFAULT 1;

                -- Add custom_priority column if missing
                ALTER TABLE requirements ADD COLUMN custom_priority TEXT;

                -- Add ai_evaluation column if missing
                ALTER TABLE requirements ADD COLUMN ai_evaluation TEXT;

                -- Add version column to users
                ALTER TABLE users ADD COLUMN version INTEGER NOT NULL DEFAULT 1;

                -- Add new metadata columns
                ALTER TABLE metadata ADD COLUMN ai_prompts TEXT NOT NULL DEFAULT '{}';
                ALTER TABLE metadata ADD COLUMN baselines TEXT NOT NULL DEFAULT '[]';
                ALTER TABLE metadata ADD COLUMN teams TEXT NOT NULL DEFAULT '[]';
                ALTER TABLE metadata ADD COLUMN store_version INTEGER NOT NULL DEFAULT 1;

                -- Update schema version
                UPDATE schema_version SET version = 2;
                "#,
            )
            .unwrap_or_else(|e| {
                // Some columns may already exist, ignore those errors
                eprintln!("Note: Some migration columns may already exist: {}", e);
            });

            // Ensure schema version is updated even if some ALTERs failed
            let _ = conn.execute("UPDATE schema_version SET version = 2", []);
        }

        if from_version < 3 {
            // Migration from v2 to v3: Add trace_links and implementation_info columns
            // trace:REQ-0245 | ai:claude:high
            conn.execute_batch(
                r#"
                -- Add trace_links column for code-to-requirement traceability
                ALTER TABLE requirements ADD COLUMN trace_links TEXT NOT NULL DEFAULT '[]';

                -- Add implementation_info column for implementation metadata
                ALTER TABLE requirements ADD COLUMN implementation_info TEXT;

                -- Update schema version
                UPDATE schema_version SET version = 3;
                "#,
            )
            .unwrap_or_else(|e| {
                // Some columns may already exist, ignore those errors
                eprintln!("Note: Some v3 migration columns may already exist: {}", e);
            });

            // Ensure schema version is updated even if some ALTERs failed
            let _ = conn.execute("UPDATE schema_version SET version = 3", []);
        }

        if from_version < 4 {
            // Migration from v3 to v4: Add pin_hash column to users table
            // trace:AUTH-0001 | ai:claude:high
            conn.execute_batch(
                r#"
                -- Add pin_hash column for simple user authentication
                ALTER TABLE users ADD COLUMN pin_hash TEXT;

                -- Update schema version
                UPDATE schema_version SET version = 4;
                "#,
            )
            .unwrap_or_else(|e| {
                // Column may already exist, ignore those errors
                eprintln!("Note: Some v4 migration columns may already exist: {}", e);
            });

            // Ensure schema version is updated even if ALTER failed
            let _ = conn.execute("UPDATE schema_version SET version = 4", []);
        }

        if from_version < 5 {
            // Migration from v4 to v5: Add gitlab_sync_state table
            // trace:STORY-0325 | ai:claude
            conn.execute_batch(
                r#"
                -- GitLab sync state table for tracking sync between AIDA and GitLab
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

                -- Update schema version
                UPDATE schema_version SET version = 5;
                "#,
            ).unwrap_or_else(|e| {
                eprintln!("Note: Some v5 migration may already exist: {}", e);
            });

            // Ensure schema version is updated
            let _ = conn.execute("UPDATE schema_version SET version = 5", []);
        }

        // Migrate from version 5 to version 6 (add meta_subtype column)
        if from_version < 6 {
            conn.execute_batch(
                r#"
                -- Add meta_subtype column for Meta requirements
                ALTER TABLE requirements ADD COLUMN meta_subtype TEXT;

                -- Update schema version
                UPDATE schema_version SET version = 6;
                "#,
            )
            .unwrap_or_else(|e| {
                eprintln!("Note: Some v6 migration may already exist: {}", e);
            });

            // Ensure schema version is updated
            let _ = conn.execute("UPDATE schema_version SET version = 6", []);
        }

        // Migrate from version 6 to version 7 (add queue_entries table)
        // trace:STORY-0366 | ai:claude
        if from_version < 7 {
            conn.execute_batch(
                r#"
                -- Queue entries table for personal work queue per user
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

                -- Update schema version
                UPDATE schema_version SET version = 7;
                "#,
            ).unwrap_or_else(|e| {
                eprintln!("Note: Some v7 migration may already exist: {}", e);
            });

            // Ensure schema version is updated
            let _ = conn.execute("UPDATE schema_version SET version = 7", []);
        }

        // Migrate from version 7 to version 8 (persist additional requirement fields)
        if from_version < 8 {
            conn.execute_batch(
                r#"
                ALTER TABLE requirements ADD COLUMN weight REAL;
                ALTER TABLE requirements ADD COLUMN attachments TEXT NOT NULL DEFAULT '[]';
                ALTER TABLE requirements ADD COLUMN gitlab_issues TEXT NOT NULL DEFAULT '[]';

                UPDATE schema_version SET version = 8;
                "#,
            )
            .unwrap_or_else(|e| {
                eprintln!("Note: Some v8 migration columns may already exist: {}", e);
            });

            let _ = conn.execute("UPDATE schema_version SET version = 8", []);
        }

        Ok(())
    }

    /// Serializes complex types to JSON for storage
    fn to_json<T: serde::Serialize>(value: &T) -> Result<String> {
        serde_json::to_string(value).context("Failed to serialize to JSON")
    }

    /// Deserializes complex types from JSON storage
    fn from_json<T: serde::de::DeserializeOwned>(json: &str) -> Result<T> {
        serde_json::from_str(json).context("Failed to deserialize from JSON")
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

    /// Load requirements from database
    fn load_requirements(&self, conn: &Connection) -> Result<Vec<Requirement>> {
        // Check if new columns exist (for schema migration compatibility)
        let has_trace_links = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('requirements') WHERE name='trace_links'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .unwrap_or(0)
            > 0;

        let query = if has_trace_links {
            "SELECT id, spec_id, prefix_override, title, description, status, priority,
                    owner, feature, created_at, created_by, modified_at, req_type,
                    dependencies, tags, relationships, comments, history, archived,
                    custom_status, custom_priority, custom_fields, urls, trace_links,
                    implementation_info, ai_evaluation, weight, attachments, gitlab_issues, version
             FROM requirements ORDER BY created_at"
        } else {
            "SELECT id, spec_id, prefix_override, title, description, status, priority,
                    owner, feature, created_at, created_by, modified_at, req_type,
                    dependencies, tags, relationships, comments, history, archived,
                    custom_status, custom_priority, custom_fields, urls, NULL as trace_links,
                    NULL as implementation_info, ai_evaluation, NULL as weight, '[]' as attachments, '[]' as gitlab_issues, version
             FROM requirements ORDER BY created_at"
        };

        let mut stmt = conn.prepare(query)?;

        let rows = stmt.query_map([], |row| {
            let id_str: String = row.get(0)?;
            let spec_id: Option<String> = row.get(1)?;
            let prefix_override: Option<String> = row.get(2)?;
            let title: String = row.get(3)?;
            let description: String = row.get(4)?;
            let status_str: String = row.get(5)?;
            let priority_str: String = row.get(6)?;
            let owner: String = row.get(7)?;
            let feature: String = row.get(8)?;
            let created_at_str: String = row.get(9)?;
            let created_by: Option<String> = row.get(10)?;
            let modified_at_str: String = row.get(11)?;
            let req_type_str: String = row.get(12)?;
            let dependencies_json: String = row.get(13)?;
            let tags_json: String = row.get(14)?;
            let relationships_json: String = row.get(15)?;
            let comments_json: String = row.get(16)?;
            let history_json: String = row.get(17)?;
            let archived: bool = row.get(18)?;
            let custom_status: Option<String> = row.get(19)?;
            let custom_priority: Option<String> = row.get(20)?;
            let custom_fields_json: String = row.get(21)?;
            let urls_json: String = row.get(22)?;
            let trace_links_json: Option<String> = row.get(23)?;
            let implementation_info_json: Option<String> = row.get(24)?;
            let ai_evaluation_json: Option<String> = row.get(25)?;
            let weight: Option<f32> = row.get(26)?;
            let attachments_json: String = row.get(27)?;
            let gitlab_issues_json: String = row.get(28)?;
            let version: i64 = row.get(29)?;

            Ok((
                id_str,
                spec_id,
                prefix_override,
                title,
                description,
                status_str,
                priority_str,
                owner,
                feature,
                created_at_str,
                created_by,
                modified_at_str,
                req_type_str,
                dependencies_json,
                tags_json,
                relationships_json,
                comments_json,
                history_json,
                archived,
                custom_status,
                custom_priority,
                custom_fields_json,
                urls_json,
                trace_links_json,
                implementation_info_json,
                ai_evaluation_json,
                weight,
                attachments_json,
                gitlab_issues_json,
                version,
            ))
        })?;

        let mut requirements = Vec::new();
        for row_result in rows {
            let (
                id_str,
                spec_id,
                prefix_override,
                title,
                description,
                status_str,
                priority_str,
                owner,
                feature,
                created_at_str,
                created_by,
                modified_at_str,
                req_type_str,
                dependencies_json,
                tags_json,
                relationships_json,
                comments_json,
                history_json,
                archived,
                custom_status,
                custom_priority,
                custom_fields_json,
                urls_json,
                trace_links_json,
                implementation_info_json,
                ai_evaluation_json,
                weight,
                attachments_json,
                gitlab_issues_json,
                version,
            ) = row_result?;

            let id = Uuid::parse_str(&id_str).unwrap_or_else(|_| Uuid::now_v7());
            let status = Self::str_to_status(&status_str);
            let priority = Self::str_to_priority(&priority_str);
            let req_type = Self::str_to_type(&req_type_str);
            let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now());
            let modified_at = chrono::DateTime::parse_from_rfc3339(&modified_at_str)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now());
            let dependencies: Vec<Uuid> = Self::from_json(&dependencies_json).unwrap_or_default();
            let tags: HashSet<String> = Self::from_json(&tags_json).unwrap_or_default();
            let relationships: Vec<Relationship> =
                Self::from_json(&relationships_json).unwrap_or_default();
            let comments: Vec<Comment> = Self::from_json(&comments_json).unwrap_or_default();
            let history: Vec<HistoryEntry> = Self::from_json(&history_json).unwrap_or_default();
            let custom_fields: HashMap<String, String> =
                Self::from_json(&custom_fields_json).unwrap_or_default();
            let urls: Vec<UrlLink> = Self::from_json(&urls_json).unwrap_or_default();
            let trace_links: Vec<TraceLink> = trace_links_json
                .and_then(|json| Self::from_json(&json).ok())
                .unwrap_or_default();
            let implementation_info: Option<ImplementationInfo> =
                implementation_info_json.and_then(|json| Self::from_json(&json).ok());
            let ai_evaluation = ai_evaluation_json.and_then(|json| Self::from_json(&json).ok());
            let attachments: Vec<Attachment> =
                Self::from_json(&attachments_json).unwrap_or_default();
            let gitlab_issues: Vec<GitLabIssueLink> =
                Self::from_json(&gitlab_issues_json).unwrap_or_default();

            requirements.push(Requirement {
                id,
                spec_id,
                agreed_id: None,
                prefix_override,
                title,
                description,
                status,
                priority,
                owner,
                // trace:STORY-639 | ai:claude — legacy backend does not persist assignee.
                assignee: None,
                feature,
                created_at,
                created_by,
                modified_at,
                req_type,
                meta_subtype: None, // Loaded separately if needed
                dependencies,
                tags,
                weight,
                relationships,
                comments,
                history,
                archived,
                // STORY-441: legacy centralized SQLite backend is deprecated
                // and does not persist archived_at; treat reads as unset.
                archived_at: None,
                // STORY-584: legacy centralized SQLite backend is deprecated and
                // does not persist the deferred view-flag; treat reads as unset.
                deferred: false,
                deferred_at: None,
                deferred_until: None,
                // trace:TASK-1148 | ai:claude — narrative fields not carried by legacy backend
                risk_notes: None,
                test_coverage_notes: None,
                implementation_summary: None,
                custom_status,
                custom_priority,
                custom_fields,
                urls,
                attachments,
                trace_links,
                gitlab_issues,
                // STORY-582: legacy centralized SQLite backend is deprecated
                // and does not persist processing records. trace:STORY-582
                processing_record: Vec::new(),
                // STORY-476: legacy centralized SQLite backend is deprecated
                // and does not persist external refs. trace:STORY-476 | ai:claude
                external_refs: Vec::new(),
                implementation_info,
                ai_evaluation,
                // STORY-332: the legacy centralized SQLite backend is
                // deprecated and does not persist punt metadata.
                attention_reason: None,
                // EPIC-28: the legacy centralized SQLite backend is
                // deprecated and does not persist orchestrator-shelving
                // metadata. trace:EPIC-28 | ai:claude
                failure_reason: None,
                // STORY-333: the legacy centralized SQLite backend is
                // deprecated and does not persist the human-only marker;
                // un-pickability is a git-canonical concern.
                // trace:STORY-333 | ai:claude
                human_only: false,
                // STORY-522: legacy centralized backend — not persisted.
                // trace:STORY-522 | ai:claude
                decision_request: None,
                // STORY-542: legacy centralized backend — not persisted.
                // trace:STORY-542 | ai:claude
                interface_changes: None,
                // trace:STORY-631 | ai:claude
                intent: None,
                version,
            });
        }

        Ok(requirements)
    }

    /// Load users from database
    fn load_users(&self, conn: &Connection) -> Result<Vec<User>> {
        let mut stmt = conn.prepare(
            "SELECT id, spec_id, name, email, handle, pin_hash, created_at, archived, version FROM users"
        )?;

        let rows = stmt.query_map([], |row| {
            let id_str: String = row.get(0)?;
            let spec_id: Option<String> = row.get(1)?;
            let name: String = row.get(2)?;
            let email: String = row.get(3)?;
            let handle: String = row.get(4)?;
            let pin_hash: Option<String> = row.get(5)?;
            let created_at_str: String = row.get(6)?;
            let archived: bool = row.get(7)?;
            let version: i64 = row.get(8)?;
            Ok((
                id_str,
                spec_id,
                name,
                email,
                handle,
                pin_hash,
                created_at_str,
                archived,
                version,
            ))
        })?;

        let mut users = Vec::new();
        for row_result in rows {
            let (id_str, spec_id, name, email, handle, pin_hash, created_at_str, archived, version): (String, Option<String>, String, String, String, Option<String>, String, bool, i64) = row_result?;
            let id = Uuid::parse_str(&id_str).unwrap_or_else(|_| Uuid::now_v7());
            let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now());

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
    fn load_metadata(
        &self,
        conn: &Connection,
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
        let row = conn.query_row(
            "SELECT name, title, description, id_config, next_feature_number, next_spec_number, prefix_counters, meta_counters
             FROM metadata WHERE id = 1",
            [],
            |row| {
                let name: String = row.get(0)?;
                let title: String = row.get(1)?;
                let description: String = row.get(2)?;
                let id_config_json: String = row.get(3)?;
                let next_feature_number: u32 = row.get(4)?;
                let next_spec_number: u32 = row.get(5)?;
                let prefix_counters_json: String = row.get(6)?;
                let meta_counters_json: String = row.get(7)?;
                Ok((name, title, description, id_config_json, next_feature_number, next_spec_number, prefix_counters_json, meta_counters_json))
            }
        ).optional()?;

        match row {
            Some((
                name,
                title,
                description,
                id_config_json,
                next_feature_number,
                next_spec_number,
                prefix_counters_json,
                meta_counters_json,
            )) => {
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
                    next_feature_number,
                    next_spec_number,
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
    fn load_features(&self, conn: &Connection) -> Result<Vec<FeatureDefinition>> {
        let json: String = conn
            .query_row("SELECT features FROM metadata WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|_| "[]".to_string());
        Self::from_json(&json)
    }

    /// Load type definitions from database
    fn load_type_definitions(&self, conn: &Connection) -> Result<Vec<CustomTypeDefinition>> {
        let json: String = conn
            .query_row(
                "SELECT type_definitions FROM metadata WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "[]".to_string());
        let defs: Vec<CustomTypeDefinition> = Self::from_json(&json)?;
        if defs.is_empty() {
            Ok(crate::models::default_type_definitions())
        } else {
            Ok(defs)
        }
    }

    /// Load relationship definitions from database
    fn load_relationship_definitions(
        &self,
        conn: &Connection,
    ) -> Result<Vec<RelationshipDefinition>> {
        let json: String = conn
            .query_row(
                "SELECT relationship_definitions FROM metadata WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "[]".to_string());
        let defs: Vec<RelationshipDefinition> = Self::from_json(&json)?;
        if defs.is_empty() {
            Ok(RelationshipDefinition::defaults())
        } else {
            Ok(defs)
        }
    }

    /// Load reaction definitions from database
    fn load_reaction_definitions(&self, conn: &Connection) -> Result<Vec<ReactionDefinition>> {
        let json: String = conn
            .query_row(
                "SELECT reaction_definitions FROM metadata WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "[]".to_string());
        let defs: Vec<ReactionDefinition> = Self::from_json(&json)?;
        if defs.is_empty() {
            Ok(crate::models::default_reaction_definitions())
        } else {
            Ok(defs)
        }
    }

    /// Load allowed prefixes from database
    fn load_allowed_prefixes(&self, conn: &Connection) -> Result<(Vec<String>, bool)> {
        let row = conn
            .query_row(
                "SELECT allowed_prefixes, restrict_prefixes FROM metadata WHERE id = 1",
                [],
                |row| {
                    let json: String = row.get(0)?;
                    let restrict: bool = row.get(1)?;
                    Ok((json, restrict))
                },
            )
            .optional()?;

        match row {
            Some((json, restrict)) => {
                let prefixes: Vec<String> = Self::from_json(&json).unwrap_or_default();
                Ok((prefixes, restrict))
            }
            None => Ok((Vec::new(), false)),
        }
    }

    /// Save a requirement to the database (for full store save)
    fn save_requirement(&self, conn: &Connection, req: &Requirement) -> Result<()> {
        let ai_eval_json = req.ai_evaluation.as_ref().map(Self::to_json).transpose()?;
        let impl_info_json = req
            .implementation_info
            .as_ref()
            .map(Self::to_json)
            .transpose()?;
        let attachments_json = Self::to_json(&req.attachments)?;
        let gitlab_issues_json = Self::to_json(&req.gitlab_issues)?;

        // Check if new columns exist and use appropriate query
        let has_trace_links = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('requirements') WHERE name='trace_links'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .unwrap_or(0)
            > 0;

        if has_trace_links {
            conn.execute(
                "INSERT OR REPLACE INTO requirements
                 (id, spec_id, prefix_override, title, description, status, priority, owner, feature,
                  created_at, created_by, modified_at, req_type, dependencies, tags, relationships,
                  comments, history, archived, custom_status, custom_priority, custom_fields, urls,
                  trace_links, implementation_info, ai_evaluation, weight, attachments, gitlab_issues, version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)",
                params![
                    req.id.to_string(),
                    req.spec_id,
                    req.prefix_override,
                    req.title,
                    req.description,
                    Self::status_to_str(&req.status),
                    Self::priority_to_str(&req.priority),
                    req.owner,
                    req.feature,
                    req.created_at.to_rfc3339(),
                    req.created_by,
                    req.modified_at.to_rfc3339(),
                    Self::type_to_str(&req.req_type),
                    Self::to_json(&req.dependencies)?,
                    Self::to_json(&req.tags)?,
                    Self::to_json(&req.relationships)?,
                    Self::to_json(&req.comments)?,
                    Self::to_json(&req.history)?,
                    req.archived,
                    req.custom_status,
                    req.custom_priority,
                    Self::to_json(&req.custom_fields)?,
                    Self::to_json(&req.urls)?,
                    Self::to_json(&req.trace_links)?,
                    impl_info_json,
                    ai_eval_json,
                    req.weight,
                    attachments_json,
                    gitlab_issues_json,
                    req.version,
                ],
            )?;
        } else {
            // Fallback for old schema without trace_links
            conn.execute(
                "INSERT OR REPLACE INTO requirements
                 (id, spec_id, prefix_override, title, description, status, priority, owner, feature,
                  created_at, created_by, modified_at, req_type, dependencies, tags, relationships,
                  comments, history, archived, custom_status, custom_priority, custom_fields, urls,
                  ai_evaluation, version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
                params![
                    req.id.to_string(),
                    req.spec_id,
                    req.prefix_override,
                    req.title,
                    req.description,
                    Self::status_to_str(&req.status),
                    Self::priority_to_str(&req.priority),
                    req.owner,
                    req.feature,
                    req.created_at.to_rfc3339(),
                    req.created_by,
                    req.modified_at.to_rfc3339(),
                    Self::type_to_str(&req.req_type),
                    Self::to_json(&req.dependencies)?,
                    Self::to_json(&req.tags)?,
                    Self::to_json(&req.relationships)?,
                    Self::to_json(&req.comments)?,
                    Self::to_json(&req.history)?,
                    req.archived,
                    req.custom_status,
                    req.custom_priority,
                    Self::to_json(&req.custom_fields)?,
                    Self::to_json(&req.urls)?,
                    ai_eval_json,
                    req.version,
                ],
            )?;
        }
        Ok(())
    }

    /// Save a user to the database
    fn save_user(&self, conn: &Connection, user: &User) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO users (id, spec_id, name, email, handle, pin_hash, created_at, archived, version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                user.id.to_string(),
                user.spec_id,
                user.name,
                user.email,
                user.handle,
                user.pin_hash,
                user.created_at.to_rfc3339(),
                user.archived,
                user.version,
            ],
        )?;
        Ok(())
    }

    /// Save metadata to the database
    fn save_metadata(&self, conn: &Connection, store: &RequirementsStore) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO metadata
             (id, name, title, description, id_config, features, next_feature_number, next_spec_number,
              prefix_counters, relationship_definitions, reaction_definitions, meta_counters,
              type_definitions, allowed_prefixes, restrict_prefixes, ai_prompts, baselines, teams, store_version)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                store.name,
                store.title,
                store.description,
                Self::to_json(&store.id_config)?,
                Self::to_json(&store.features)?,
                store.next_feature_number,
                store.next_spec_number,
                Self::to_json(&store.prefix_counters)?,
                Self::to_json(&store.relationship_definitions)?,
                Self::to_json(&store.reaction_definitions)?,
                Self::to_json(&store.meta_counters)?,
                Self::to_json(&store.type_definitions)?,
                Self::to_json(&store.allowed_prefixes)?,
                store.restrict_prefixes,
                Self::to_json(&store.ai_prompts)?,
                Self::to_json(&store.baselines)?,
                Self::to_json(&store.teams)?,
                store.store_version,
            ],
        )?;
        Ok(())
    }

    /// Load store_version from metadata
    fn load_store_version(&self, conn: &Connection) -> Result<i64> {
        let version: i64 = conn
            .query_row(
                "SELECT store_version FROM metadata WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(1);
        Ok(version)
    }

    /// Load ai_prompts from metadata
    fn load_ai_prompts(&self, conn: &Connection) -> Result<crate::models::AiPromptConfig> {
        let json: String = conn
            .query_row("SELECT ai_prompts FROM metadata WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|_| "{}".to_string());
        Self::from_json(&json).or_else(|_| Ok(crate::models::AiPromptConfig::default()))
    }

    /// Load baselines from metadata
    fn load_baselines(&self, conn: &Connection) -> Result<Vec<crate::models::Baseline>> {
        let json: String = conn
            .query_row("SELECT baselines FROM metadata WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|_| "[]".to_string());
        Self::from_json(&json)
    }

    /// Load teams from metadata
    fn load_teams(&self, conn: &Connection) -> Result<Vec<crate::models::Team>> {
        let json: String = conn
            .query_row("SELECT teams FROM metadata WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|_| "[]".to_string());
        Self::from_json(&json)
    }

    // ==================== GitLab Sync State Operations (STORY-0325) ====================

    /// Save or update a GitLab sync state
    /// trace:STORY-0325 | ai:claude
    pub fn save_sync_state(&self, state: &crate::models::GitLabSyncState) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"INSERT OR REPLACE INTO gitlab_sync_state
               (requirement_id, spec_id, gitlab_project_id, gitlab_issue_iid, gitlab_issue_id,
                linked_at, last_sync, aida_content_hash, gitlab_content_hash,
                link_origin, sync_status, last_error)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"#,
            rusqlite::params![
                state.requirement_id.to_string(),
                state.spec_id,
                state.gitlab_project_id as i64,
                state.gitlab_issue_iid as i64,
                state.gitlab_issue_id as i64,
                state.linked_at.to_rfc3339(),
                state.last_sync.to_rfc3339(),
                state.aida_content_hash,
                state.gitlab_content_hash,
                format!("{:?}", state.link_origin),
                format!("{:?}", state.sync_status),
                state.last_error,
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
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            r#"SELECT requirement_id, spec_id, gitlab_project_id, gitlab_issue_iid, gitlab_issue_id,
                      linked_at, last_sync, aida_content_hash, gitlab_content_hash,
                      link_origin, sync_status, last_error
               FROM gitlab_sync_state WHERE requirement_id = ?1 AND gitlab_issue_iid = ?2"#,
            rusqlite::params![requirement_id.to_string(), issue_iid as i64],
            Self::row_to_sync_state,
        );

        match result {
            Ok(state) => Ok(Some(state)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Load all sync states for a requirement
    /// trace:STORY-0325 | ai:claude
    pub fn load_sync_states_for_requirement(
        &self,
        requirement_id: Uuid,
    ) -> Result<Vec<crate::models::GitLabSyncState>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"SELECT requirement_id, spec_id, gitlab_project_id, gitlab_issue_iid, gitlab_issue_id,
                      linked_at, last_sync, aida_content_hash, gitlab_content_hash,
                      link_origin, sync_status, last_error
               FROM gitlab_sync_state WHERE requirement_id = ?1"#,
        )?;

        let states = stmt
            .query_map([requirement_id.to_string()], |row| {
                Self::row_to_sync_state(row)
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(states)
    }

    /// Load all sync states
    /// trace:STORY-0325 | ai:claude
    pub fn load_all_sync_states(&self) -> Result<Vec<crate::models::GitLabSyncState>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"SELECT requirement_id, spec_id, gitlab_project_id, gitlab_issue_iid, gitlab_issue_id,
                      linked_at, last_sync, aida_content_hash, gitlab_content_hash,
                      link_origin, sync_status, last_error
               FROM gitlab_sync_state ORDER BY last_sync DESC"#,
        )?;

        let states = stmt
            .query_map([], Self::row_to_sync_state)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(states)
    }

    /// Load sync states by status (e.g., all diverged items)
    /// trace:STORY-0325 | ai:claude
    pub fn load_sync_states_by_status(
        &self,
        status: crate::models::SyncStatus,
    ) -> Result<Vec<crate::models::GitLabSyncState>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"SELECT requirement_id, spec_id, gitlab_project_id, gitlab_issue_iid, gitlab_issue_id,
                      linked_at, last_sync, aida_content_hash, gitlab_content_hash,
                      link_origin, sync_status, last_error
               FROM gitlab_sync_state WHERE sync_status = ?1 ORDER BY last_sync DESC"#,
        )?;

        let states = stmt
            .query_map([format!("{:?}", status)], |row| {
                Self::row_to_sync_state(row)
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(states)
    }

    /// Delete a sync state
    /// trace:STORY-0325 | ai:claude
    pub fn delete_sync_state(&self, requirement_id: Uuid, issue_iid: u64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows_affected = conn.execute(
            "DELETE FROM gitlab_sync_state WHERE requirement_id = ?1 AND gitlab_issue_iid = ?2",
            rusqlite::params![requirement_id.to_string(), issue_iid as i64],
        )?;
        Ok(rows_affected > 0)
    }

    /// Helper to convert a row to GitLabSyncState
    fn row_to_sync_state(row: &rusqlite::Row) -> rusqlite::Result<crate::models::GitLabSyncState> {
        use crate::models::{GitLabSyncState, LinkOrigin, SyncStatus};

        let req_id_str: String = row.get(0)?;
        let linked_at_str: String = row.get(5)?;
        let last_sync_str: String = row.get(6)?;
        let link_origin_str: String = row.get(9)?;
        let sync_status_str: String = row.get(10)?;

        Ok(GitLabSyncState {
            requirement_id: Uuid::parse_str(&req_id_str).unwrap_or_default(),
            spec_id: row.get(1)?,
            gitlab_project_id: row.get::<_, i64>(2)? as u64,
            gitlab_issue_iid: row.get::<_, i64>(3)? as u64,
            gitlab_issue_id: row.get::<_, i64>(4)? as u64,
            linked_at: chrono::DateTime::parse_from_rfc3339(&linked_at_str)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now()),
            last_sync: chrono::DateTime::parse_from_rfc3339(&last_sync_str)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now()),
            aida_content_hash: row.get(7)?,
            gitlab_content_hash: row.get(8)?,
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
            last_error: row.get(11)?,
        })
    }
}

impl DatabaseBackend for SqliteBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::Sqlite
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn load(&self) -> Result<RequirementsStore> {
        let conn = self.conn.lock().unwrap();

        let requirements = self.load_requirements(&conn)?;
        let users = self.load_users(&conn)?;
        let (
            name,
            title,
            description,
            id_config,
            next_feature_number,
            next_spec_number,
            prefix_counters,
            meta_counters,
        ) = self.load_metadata(&conn)?;
        let features = self.load_features(&conn)?;
        let type_definitions = self.load_type_definitions(&conn)?;
        let relationship_definitions = self.load_relationship_definitions(&conn)?;
        let reaction_definitions = self.load_reaction_definitions(&conn)?;
        let (allowed_prefixes, restrict_prefixes) = self.load_allowed_prefixes(&conn)?;
        let ai_prompts = self.load_ai_prompts(&conn)?;
        let baselines = self.load_baselines(&conn)?;
        let teams = self.load_teams(&conn)?;
        let store_version = self.load_store_version(&conn)?;

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
            migrated_to: None, // SQLite is never a migrated-from source
            dispenser: None,
        })
    }

    fn save(&self, store: &RequirementsStore) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Use a transaction for atomicity
        conn.execute("BEGIN TRANSACTION", [])?;

        // Clear existing data
        conn.execute("DELETE FROM requirements", [])?;
        conn.execute("DELETE FROM users", [])?;

        // Save all requirements
        for req in &store.requirements {
            self.save_requirement(&conn, req)?;
        }

        // Save all users
        for user in &store.users {
            self.save_user(&conn, user)?;
        }

        // Save metadata
        self.save_metadata(&conn, store)?;

        conn.execute("COMMIT", [])?;
        Ok(())
    }

    fn update_atomically<F>(&self, update_fn: F) -> Result<RequirementsStore>
    where
        F: FnOnce(&mut RequirementsStore),
    {
        let conn = self.conn.lock().unwrap();

        conn.execute("BEGIN EXCLUSIVE TRANSACTION", [])?;

        // Load within transaction
        drop(conn);
        let mut store = self.load()?;

        // Apply changes
        update_fn(&mut store);

        // Save within transaction
        let conn = self.conn.lock().unwrap();

        // Clear existing data
        conn.execute("DELETE FROM requirements", [])?;
        conn.execute("DELETE FROM users", [])?;

        // Save all requirements
        for req in &store.requirements {
            self.save_requirement(&conn, req)?;
        }

        // Save all users
        for user in &store.users {
            self.save_user(&conn, user)?;
        }

        // Save metadata
        self.save_metadata(&conn, &store)?;

        conn.execute("COMMIT", [])?;
        Ok(store)
    }

    // Override for more efficient single-requirement operations

    fn get_requirement(&self, id: &Uuid) -> Result<Option<Requirement>> {
        let conn = self.conn.lock().unwrap();

        // Check if new columns exist (for schema migration compatibility)
        let has_trace_links = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('requirements') WHERE name='trace_links'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .unwrap_or(0)
            > 0;

        let query = if has_trace_links {
            "SELECT id, spec_id, prefix_override, title, description, status, priority,
                    owner, feature, created_at, created_by, modified_at, req_type,
                    dependencies, tags, relationships, comments, history, archived,
                    custom_status, custom_priority, custom_fields, urls, trace_links,
                    implementation_info, ai_evaluation, weight, attachments, gitlab_issues, version
             FROM requirements WHERE id = ?1"
        } else {
            "SELECT id, spec_id, prefix_override, title, description, status, priority,
                    owner, feature, created_at, created_by, modified_at, req_type,
                    dependencies, tags, relationships, comments, history, archived,
                    custom_status, custom_priority, custom_fields, urls, NULL as trace_links,
                    NULL as implementation_info, ai_evaluation, NULL as weight, '[]' as attachments, '[]' as gitlab_issues, version
             FROM requirements WHERE id = ?1"
        };

        let result = conn
            .query_row(query, [id.to_string()], |row| {
                let id_str: String = row.get(0)?;
                let spec_id: Option<String> = row.get(1)?;
                let prefix_override: Option<String> = row.get(2)?;
                let title: String = row.get(3)?;
                let description: String = row.get(4)?;
                let status_str: String = row.get(5)?;
                let priority_str: String = row.get(6)?;
                let owner: String = row.get(7)?;
                let feature: String = row.get(8)?;
                let created_at_str: String = row.get(9)?;
                let created_by: Option<String> = row.get(10)?;
                let modified_at_str: String = row.get(11)?;
                let req_type_str: String = row.get(12)?;
                let dependencies_json: String = row.get(13)?;
                let tags_json: String = row.get(14)?;
                let relationships_json: String = row.get(15)?;
                let comments_json: String = row.get(16)?;
                let history_json: String = row.get(17)?;
                let archived: bool = row.get(18)?;
                let custom_status: Option<String> = row.get(19)?;
                let custom_priority: Option<String> = row.get(20)?;
                let custom_fields_json: String = row.get(21)?;
                let urls_json: String = row.get(22)?;
                let trace_links_json: Option<String> = row.get(23)?;
                let implementation_info_json: Option<String> = row.get(24)?;
                let ai_evaluation_json: Option<String> = row.get(25)?;
                let weight: Option<f32> = row.get(26)?;
                let attachments_json: String = row.get(27)?;
                let gitlab_issues_json: String = row.get(28)?;
                let version: i64 = row.get(29)?;

                Ok((
                    id_str,
                    spec_id,
                    prefix_override,
                    title,
                    description,
                    status_str,
                    priority_str,
                    owner,
                    feature,
                    created_at_str,
                    created_by,
                    modified_at_str,
                    req_type_str,
                    dependencies_json,
                    tags_json,
                    relationships_json,
                    comments_json,
                    history_json,
                    archived,
                    custom_status,
                    custom_priority,
                    custom_fields_json,
                    urls_json,
                    trace_links_json,
                    implementation_info_json,
                    ai_evaluation_json,
                    weight,
                    attachments_json,
                    gitlab_issues_json,
                    version,
                ))
            })
            .optional()?;

        match result {
            Some((
                id_str,
                spec_id,
                prefix_override,
                title,
                description,
                status_str,
                priority_str,
                owner,
                feature,
                created_at_str,
                created_by,
                modified_at_str,
                req_type_str,
                dependencies_json,
                tags_json,
                relationships_json,
                comments_json,
                history_json,
                archived,
                custom_status,
                custom_priority,
                custom_fields_json,
                urls_json,
                trace_links_json,
                implementation_info_json,
                ai_evaluation_json,
                weight,
                attachments_json,
                gitlab_issues_json,
                version,
            )) => {
                let id = Uuid::parse_str(&id_str).unwrap_or_else(|_| Uuid::now_v7());
                let status = Self::str_to_status(&status_str);
                let priority = Self::str_to_priority(&priority_str);
                let req_type = Self::str_to_type(&req_type_str);
                let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now());
                let modified_at = chrono::DateTime::parse_from_rfc3339(&modified_at_str)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now());
                let dependencies: Vec<Uuid> =
                    Self::from_json(&dependencies_json).unwrap_or_default();
                let tags: HashSet<String> = Self::from_json(&tags_json).unwrap_or_default();
                let relationships: Vec<Relationship> =
                    Self::from_json(&relationships_json).unwrap_or_default();
                let comments: Vec<Comment> = Self::from_json(&comments_json).unwrap_or_default();
                let history: Vec<HistoryEntry> = Self::from_json(&history_json).unwrap_or_default();
                let custom_fields: HashMap<String, String> =
                    Self::from_json(&custom_fields_json).unwrap_or_default();
                let urls: Vec<UrlLink> = Self::from_json(&urls_json).unwrap_or_default();
                let trace_links: Vec<TraceLink> = trace_links_json
                    .and_then(|json| Self::from_json(&json).ok())
                    .unwrap_or_default();
                let implementation_info: Option<ImplementationInfo> =
                    implementation_info_json.and_then(|json| Self::from_json(&json).ok());
                let ai_evaluation = ai_evaluation_json.and_then(|json| Self::from_json(&json).ok());
                let attachments: Vec<Attachment> =
                    Self::from_json(&attachments_json).unwrap_or_default();
                let gitlab_issues: Vec<GitLabIssueLink> =
                    Self::from_json(&gitlab_issues_json).unwrap_or_default();

                Ok(Some(Requirement {
                    id,
                    spec_id,
                    agreed_id: None,
                    prefix_override,
                    title,
                    description,
                    status,
                    priority,
                    owner,
                    // trace:STORY-639 | ai:claude — legacy backend does not persist assignee.
                    assignee: None,
                    feature,
                    created_at,
                    created_by,
                    modified_at,
                    req_type,
                    meta_subtype: None, // Loaded separately if needed
                    dependencies,
                    tags,
                    weight,
                    relationships,
                    comments,
                    history,
                    // STORY-582: legacy centralized backend — not persisted.
                    processing_record: Vec::new(),
                    archived,
                    // STORY-441: legacy centralized backend — not persisted.
                    archived_at: None,
                    // STORY-584: legacy centralized backend — not persisted.
                    deferred: false,
                    deferred_at: None,
                    deferred_until: None,
                    // trace:TASK-1148 | ai:claude — narrative fields not carried by legacy backend
                    risk_notes: None,
                    test_coverage_notes: None,
                    implementation_summary: None,
                    custom_status,
                    custom_priority,
                    custom_fields,
                    urls,
                    attachments,
                    trace_links,
                    gitlab_issues,
                    // STORY-476: legacy centralized backend — not persisted.
                    // trace:STORY-476 | ai:claude
                    external_refs: Vec::new(),
                    implementation_info,
                    ai_evaluation,
                    // STORY-332: legacy centralized backend — not persisted.
                    attention_reason: None,
                    // EPIC-28: legacy centralized backend — not persisted.
                    // trace:EPIC-28 | ai:claude
                    failure_reason: None,
                    // STORY-333: legacy centralized backend — not persisted.
                    // trace:STORY-333 | ai:claude
                    human_only: false,
                    // trace:STORY-522 | ai:claude
                    decision_request: None,
                    // STORY-542: legacy centralized backend — not persisted.
                    // trace:STORY-542 | ai:claude
                    interface_changes: None,
                    // trace:STORY-631 | ai:claude
                    intent: None,
                    version,
                }))
            }
            None => Ok(None),
        }
    }

    fn update_requirement(&self, requirement: &Requirement) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        self.save_requirement(&conn, requirement)
    }

    fn update_requirement_versioned(
        &self,
        requirement: &Requirement,
    ) -> Result<super::traits::UpdateResult> {
        use super::traits::{UpdateResult, VersionConflict};

        let conn = self.conn.lock().unwrap();

        // Get current version from database
        let current_version: Option<i64> = conn
            .query_row(
                "SELECT version FROM requirements WHERE id = ?1",
                [requirement.id.to_string()],
                |row| row.get(0),
            )
            .optional()?;

        match current_version {
            Some(db_version) => {
                // Check for version conflict
                if db_version != requirement.version {
                    return Ok(UpdateResult::Conflict(VersionConflict {
                        id: requirement.id,
                        expected_version: requirement.version,
                        current_version: db_version,
                        display_id: requirement
                            .spec_id
                            .clone()
                            .unwrap_or_else(|| requirement.id.to_string()),
                    }));
                }

                // Version matches - update with incremented version
                let mut updated_req = requirement.clone();
                updated_req.version = db_version + 1;
                self.save_requirement(&conn, &updated_req)?;

                Ok(UpdateResult::Success)
            }
            None => {
                anyhow::bail!("Requirement not found: {}", requirement.id)
            }
        }
    }

    fn get_store_version(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        self.load_store_version(&conn)
    }

    fn delete_requirement(&self, id: &Uuid) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let rows_affected =
            conn.execute("DELETE FROM requirements WHERE id = ?1", [id.to_string()])?;
        if rows_affected == 0 {
            anyhow::bail!("Requirement not found: {}", id)
        }
        Ok(())
    }

    fn get_user(&self, id: &Uuid) -> Result<Option<User>> {
        let conn = self.conn.lock().unwrap();

        conn.query_row(
            "SELECT id, spec_id, name, email, handle, pin_hash, created_at, archived, version FROM users WHERE id = ?1",
            [id.to_string()],
            |row| {
                let id_str: String = row.get(0)?;
                let spec_id: Option<String> = row.get(1)?;
                let name: String = row.get(2)?;
                let email: String = row.get(3)?;
                let handle: String = row.get(4)?;
                let pin_hash: Option<String> = row.get(5)?;
                let created_at_str: String = row.get(6)?;
                let archived: bool = row.get(7)?;
                let version: i64 = row.get(8)?;

                let id = Uuid::parse_str(&id_str).unwrap_or_else(|_| Uuid::now_v7());
                let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now());

                Ok(User {
                    id,
                    spec_id,
                    name,
                    email,
                    handle,
                    pin_hash,
                    created_at,
                    archived,
                    version,
                })
            }
        ).optional().map_err(|e| e.into())
    }

    fn update_user(&self, user: &User) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        self.save_user(&conn, user)
    }

    fn delete_user(&self, id: &Uuid) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let rows_affected = conn.execute("DELETE FROM users WHERE id = ?1", [id.to_string()])?;
        if rows_affected == 0 {
            anyhow::bail!("User not found: {}", id)
        }
        Ok(())
    }

    // =========================================================================
    // Queue Operations (STORY-0366)
    // =========================================================================
    // trace:STORY-0366 | ai:claude

    fn queue_list(&self, user_id: &str, include_completed: bool) -> Result<Vec<QueueEntry>> {
        let conn = self.conn.lock().unwrap();
        let sql = if include_completed {
            "SELECT q.user_id, q.requirement_id, q.position, q.added_by, q.note, q.added_at \
             FROM queue_entries q \
             WHERE q.user_id = ?1 \
             ORDER BY q.position ASC"
        } else {
            "SELECT q.user_id, q.requirement_id, q.position, q.added_by, q.note, q.added_at \
             FROM queue_entries q \
             LEFT JOIN requirements r ON q.requirement_id = r.id \
             WHERE q.user_id = ?1 AND (r.status IS NULL OR r.status != 'Completed') \
             ORDER BY q.position ASC"
        };

        let mut stmt = conn.prepare(sql)?;
        let entries = stmt
            .query_map([user_id], |row| {
                let user_id: String = row.get(0)?;
                let req_id_str: String = row.get(1)?;
                let position: i64 = row.get(2)?;
                let added_by: String = row.get(3)?;
                let note: Option<String> = row.get(4)?;
                let added_at_str: String = row.get(5)?;

                let requirement_id =
                    Uuid::parse_str(&req_id_str).unwrap_or_else(|_| Uuid::now_v7());
                let added_at = chrono::DateTime::parse_from_rfc3339(&added_at_str)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now());

                Ok(QueueEntry {
                    user_id,
                    requirement_id,
                    position,
                    added_by,
                    note,
                    added_at,
                    // Legacy SQLite-canonical mode predates role routing
                    // (EPIC-1-001) and scope/session routing (STORY-57).
                    // Always None here; columns don't exist in this
                    // schema. Git-canonical mode supports them.
                    for_role: None,
                    for_scope: None,
                    for_session: None,
                    added_by_machine: None,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    fn queue_add(&self, entry: QueueEntry) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Auto-assign position if i64::MAX (sentinel for "append to bottom")
        let position = if entry.position == i64::MAX {
            let max_pos: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(position), 0) FROM queue_entries WHERE user_id = ?1",
                    [&entry.user_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            max_pos + 1000
        } else {
            entry.position
        };

        conn.execute(
            "INSERT OR REPLACE INTO queue_entries (user_id, requirement_id, position, added_by, note, added_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                entry.user_id,
                entry.requirement_id.to_string(),
                position,
                entry.added_by,
                entry.note,
                entry.added_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn queue_remove(&self, user_id: &str, requirement_id: &Uuid) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM queue_entries WHERE user_id = ?1 AND requirement_id = ?2",
            params![user_id, requirement_id.to_string()],
        )?;
        Ok(())
    }

    fn queue_reorder(&self, user_id: &str, items: &[(Uuid, i64)]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        for (req_id, position) in items {
            tx.execute(
                "UPDATE queue_entries SET position = ?1 WHERE user_id = ?2 AND requirement_id = ?3",
                params![position, user_id, req_id.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn queue_clear(&self, user_id: &str, completed_only: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        if completed_only {
            conn.execute(
                "DELETE FROM queue_entries WHERE user_id = ?1 AND requirement_id IN \
                 (SELECT id FROM requirements WHERE status = 'Completed')",
                [user_id],
            )?;
        } else {
            conn.execute("DELETE FROM queue_entries WHERE user_id = ?1", [user_id])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_sqlite_backend_create_and_load() {
        let temp_file = NamedTempFile::with_suffix(".db").unwrap();
        let backend = SqliteBackend::new(temp_file.path()).unwrap();

        backend.create_if_not_exists().unwrap();

        let store = backend.load().unwrap();
        assert!(store.requirements.is_empty());
        assert!(store.users.is_empty());
    }

    #[test]
    fn test_sqlite_backend_save_and_load() {
        let temp_file = NamedTempFile::with_suffix(".db").unwrap();
        let backend = SqliteBackend::new(temp_file.path()).unwrap();

        let mut store = RequirementsStore::new();
        store.name = "Test DB".to_string();
        store.title = "Test Database".to_string();

        backend.save(&store).unwrap();

        let loaded = backend.load().unwrap();
        assert_eq!(loaded.name, "Test DB");
        assert_eq!(loaded.title, "Test Database");
    }

    #[test]
    fn test_sqlite_backend_requirement_crud() {
        let temp_file = NamedTempFile::with_suffix(".db").unwrap();
        let backend = SqliteBackend::new(temp_file.path()).unwrap();

        // Create initial store
        backend.save(&RequirementsStore::new()).unwrap();

        // Add requirement
        let req = Requirement::new("Test Req".to_string(), "Test Description".to_string());
        let req = backend.add_requirement(req).unwrap();

        // Get by ID
        let loaded = backend.get_requirement(&req.id).unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().title, "Test Req");

        // Delete
        backend.delete_requirement(&req.id).unwrap();
        let loaded = backend.get_requirement(&req.id).unwrap();
        assert!(loaded.is_none());
    }
}
