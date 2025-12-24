pub mod ai;
pub mod db;
pub mod export;
pub mod import;
pub mod integrations;
pub mod meta;
pub mod models;
#[cfg(feature = "native")]
pub mod project;
#[cfg(feature = "native")]
pub mod registry;
#[cfg(feature = "native")]
pub mod report;
#[cfg(feature = "native")]
pub mod scaffolding;
#[cfg(feature = "native")]
pub mod storage;
#[cfg(feature = "native")]
pub mod templates;

// Re-export commonly used types
pub use ai::{
    AiClient, AiMode, BackgroundEvaluator, EvaluationResponse, EvaluationResult, EvaluatorConfig,
    EvaluatorStatus, IssueReport, StoredAiEvaluation, SuggestedImprovement,
};
pub use models::{
    default_reaction_definitions,
    default_type_definitions,
    // AI prompt configuration types
    AiActionPromptConfig,
    AiPromptConfig,
    AiTypePromptConfig,
    // Traceability types
    ArtifactType,
    TraceLink,
    ConfidenceLevel,
    ImplementationInfo,
    // GitLab integration types
    GitLabIssueLink,
    GitLabLinkType,
    GitLabSyncState,
    LinkOrigin,
    SyncStatus,
    // Baseline types
    Baseline,
    BaselineComparison,
    BaselineRequirementDiff,
    Cardinality,
    Comment,
    // Comment reaction types
    CommentReaction,
    CustomFieldDefinition,
    // Custom type definition types
    CustomFieldType,
    CustomTypeDefinition,
    FeatureDefinition,
    FieldChange,
    HistoryEntry,
    IdConfigValidation,
    IdConfiguration,
    // New ID system types
    IdFormat,
    NumberingStrategy,
    ReactionDefinition,
    Relationship,
    // Relationship definition types
    RelationshipDefinition,
    RelationshipType,
    RelationshipValidation,
    Requirement,
    RequirementPriority,
    RequirementSnapshot,
    RequirementStatus,
    RequirementType,
    RequirementTypeDefinition,
    MetaSubtype,
    RequirementsStore,
    // URL link type
    UrlLink,
    User,
    // Team type
    Team,
    META_PREFIX_FEATURE,
    // Meta-type prefixes
    META_PREFIX_USER,
    META_PREFIX_VIEW,
    META_PREFIX_TEAM,
};
#[cfg(feature = "native")]
pub use project::{check_migration_status, determine_requirements_path, MigrationCheck};
#[cfg(feature = "native")]
pub use registry::{get_config_dir, get_registry_path, get_templates_dir, Registry};
#[cfg(feature = "native")]
pub use scaffolding::{
    ProjectType, ScaffoldArtifact, ScaffoldConfig, ScaffoldError, ScaffoldPreview, Scaffolder,
};
#[cfg(feature = "native")]
pub use storage::{
    AddResult, ConflictInfo, ConflictResolution, EditLock, FieldConflict, LockFileInfo, SaveResult,
    SessionInfo, Storage, StorageError,
};
pub use db::{
    BackendType, DatabaseBackend, DatabaseConfig, UpdateResult, VersionConflict,
};
#[cfg(feature = "native")]
pub use db::{
    YamlBackend, SqliteBackend, create_backend, open_or_create,
    migrate_yaml_to_sqlite, migrate_sqlite_to_yaml, export_to_json, import_from_json,
};
#[cfg(feature = "postgres")]
pub use db::PostgresBackend;
#[cfg(all(feature = "native", feature = "postgres"))]
pub use db::{migrate_to_postgres, migrate_from_postgres};
pub use import::{
    ImportConfig, ImportIssue, ImportIssueType, ImportMergeMode, ImportSummary, ImportValidation,
    IssueResolution, RawImportStore, create_backup, execute_import, validate_import_content,
    validate_import_file,
};
pub use meta::{
    get_prompt_template, needs_meta_seeding, seed_meta_requirements,
    DEFAULT_EVALUATION_PROMPT, DEFAULT_DUPLICATES_PROMPT, DEFAULT_RELATIONSHIPS_PROMPT,
    DEFAULT_IMPROVE_PROMPT, DEFAULT_GENERATE_CHILDREN_PROMPT,
};
#[cfg(feature = "native")]
pub use report::{
    AiIntegrationReport, AiPromptsSection, FileStatus, PromptCustomization, ReportFormat,
    ReportGenerator, ScaffoldStatus, TraceabilityStats, TypePromptCustomization,
    check_scaffold_status,
};
#[cfg(feature = "gitlab")]
pub use integrations::gitlab::{
    ClientError as GitLabClientError, ConfigError as GitLabConfigError,
    ConflictStrategy, FieldSyncDirection, FieldSyncRules, GitLabClient, GitLabConfig,
    GitLabIssue, GitLabLabel, GitLabMilestone, GitLabProject, GitLabUser,
    IssueFilter, IssueState, LabelConfig, PollingConfig, SyncConfig, SyncMode,
    CreateIssueRequest, UpdateIssueRequest, CreateNoteRequest,
};
