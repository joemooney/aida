pub mod ai;
pub mod analytics;
pub mod conflict;
#[cfg(all(unix, feature = "native"))]
pub mod daemon;
pub mod db;
pub mod dispenser;
pub mod export;
#[cfg(feature = "native")]
pub mod git_ops;
pub mod hlc;
pub mod import;
pub mod integrations;
pub mod meta;
pub mod models;
pub mod node;
pub mod object_store;
pub mod oplog;
pub mod telemetry;
#[cfg(feature = "native")]
pub mod workspace;
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
pub use dispenser::{Dispenser, DispenserState, IdMode, MemoryDispenser};
#[cfg(feature = "native")]
pub use dispenser::{FileDispenser, SqliteDispenser};
pub use hlc::{Hlc, HlcTimestamp};
pub use node::{
    AgreedCounters, DeploymentMode, NodeConfig, NodeRegistry, NodeRegistryEntry, UserRegistry,
    UserRegistryEntry, WorkspaceConfig,
};
pub use ai::{
    AiClient, AiMode, BackgroundEvaluator, EvaluationResponse, EvaluationResult, EvaluatorConfig,
    EvaluatorStatus, IssueReport, StoredAiEvaluation, SuggestedImprovement,
};
#[cfg(feature = "postgres")]
pub use db::PostgresBackend;
#[cfg(feature = "native")]
pub use db::{
    create_backend, export_to_json, import_from_json, migrate_sqlite_to_yaml,
    migrate_yaml_to_sqlite, open_or_create, GitBackend, SqliteBackend, YamlBackend,
};
#[cfg(all(feature = "native", feature = "postgres"))]
pub use db::{migrate_from_postgres, migrate_to_postgres};
pub use db::{BackendType, DatabaseBackend, DatabaseConfig, UpdateResult, VersionConflict};
pub use import::{
    create_backup, execute_import, validate_import_content, validate_import_file, ImportConfig,
    ImportIssue, ImportIssueType, ImportMergeMode, ImportSummary, ImportValidation,
    IssueResolution, RawImportStore,
};
#[cfg(feature = "github")]
pub use integrations::github::{
    ClientError as GitHubClientError, ConfigError as GitHubConfigError, GitHubClient, GitHubConfig,
    GitHubIssue, GitHubLabel, GitHubRepo, GitHubUser,
    CreateIssueRequest as GitHubCreateIssueRequest,
    UpdateIssueRequest as GitHubUpdateIssueRequest,
    IssueFilter as GitHubIssueFilter,
    LabelConfig as GitHubLabelConfig,
};
#[cfg(feature = "gitlab")]
pub use integrations::gitlab::{
    ClientError as GitLabClientError, ConfigError as GitLabConfigError, ConflictStrategy,
    CreateIssueRequest, CreateNoteRequest, FieldSyncDirection, FieldSyncRules, GitLabClient,
    GitLabConfig, GitLabIssue, GitLabLabel, GitLabMilestone, GitLabProject, GitLabUser,
    IssueFilter, IssueState, LabelConfig, PollingConfig, SyncConfig, SyncMode, UpdateIssueRequest,
};
pub use meta::{
    get_prompt_template, needs_meta_seeding, seed_meta_requirements, DEFAULT_DUPLICATES_PROMPT,
    DEFAULT_EVALUATION_PROMPT, DEFAULT_GENERATE_CHILDREN_PROMPT, DEFAULT_IMPROVE_PROMPT,
    DEFAULT_RELATIONSHIPS_PROMPT,
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
    // Baseline types
    Baseline,
    BaselineComparison,
    BaselineRequirementDiff,
    Cardinality,
    Comment,
    // Comment reaction types
    CommentReaction,
    ConfidenceLevel,
    CustomFieldDefinition,
    // Custom type definition types
    CustomFieldType,
    CustomTypeDefinition,
    FeatureDefinition,
    FieldChange,
    // GitLab integration types
    GitLabIssueLink,
    GitLabLinkType,
    GitLabSyncState,
    HistoryEntry,
    IdConfigValidation,
    IdConfiguration,
    // New ID system types
    IdFormat,
    ImplementationInfo,
    LinkOrigin,
    MetaSubtype,
    NumberingStrategy,
    // Queue types
    QueueEntry,
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
    RequirementsStore,
    SyncStatus,
    // Team type
    Team,
    TraceLink,
    // URL link types
    UrlLink,
    UrlOpenMode,
    User,
    META_PREFIX_FEATURE,
    META_PREFIX_TEAM,
    // Meta-type prefixes
    META_PREFIX_USER,
    META_PREFIX_VIEW,
};
#[cfg(feature = "native")]
pub use project::{check_migration_status, determine_requirements_path, MigrationCheck};
#[cfg(feature = "native")]
pub use registry::{get_config_dir, get_registry_path, get_templates_dir, Registry};
#[cfg(feature = "native")]
pub use report::{
    check_scaffold_status, AiIntegrationReport, AiPromptsSection, FileStatus, PromptCustomization,
    ReportFormat, ReportGenerator, ScaffoldStatus, TraceabilityStats, TypePromptCustomization,
};
#[cfg(feature = "native")]
pub use scaffolding::{
    ProjectType, ScaffoldArtifact, ScaffoldConfig, ScaffoldError, ScaffoldPreview, Scaffolder,
};
#[cfg(feature = "native")]
pub use storage::{
    AddResult, ConflictInfo, ConflictResolution, EditLock, FieldConflict, LockFileInfo, SaveResult,
    SessionInfo, Storage, StorageError,
};
#[cfg(feature = "native")]
pub use templates::{
    get_embedded_templates, get_template_categories, get_templates_by_category, TemplateInfo,
    TemplateLoader, TemplateSource,
};
