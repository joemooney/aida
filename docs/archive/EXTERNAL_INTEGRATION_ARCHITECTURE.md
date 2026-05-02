# External Issue Tracker Integration Architecture

This document outlines the architecture for integrating AIDA with external issue tracking systems: GitLab Issues, GitHub Issues, and Jira.

## Goals

1. **Bidirectional Sync**: Sync requirements between AIDA and external issue trackers
2. **Traceability**: Maintain links between AIDA requirements and external issues
3. **Flexibility**: Support different sync strategies (import-only, export-only, bidirectional)
4. **Conflict Resolution**: Handle concurrent modifications gracefully
5. **Minimal Coupling**: Allow using AIDA standalone or with any combination of integrations

## Integration Model Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                          AIDA Core                              │
│  ┌─────────────────┐   ┌─────────────────┐   ┌──────────────┐  │
│  │  Requirements   │   │   Relationships │   │   History    │  │
│  │     Store       │   │     & Links     │   │   Tracking   │  │
│  └────────┬────────┘   └────────┬────────┘   └──────┬───────┘  │
│           │                     │                    │          │
│           └─────────────────────┴────────────────────┘          │
│                                 │                               │
│                    ┌────────────┴────────────┐                  │
│                    │   Integration Manager   │                  │
│                    └────────────┬────────────┘                  │
└─────────────────────────────────┼───────────────────────────────┘
                                  │
        ┌─────────────────────────┼─────────────────────────┐
        │                         │                         │
        ▼                         ▼                         ▼
┌───────────────┐         ┌───────────────┐         ┌───────────────┐
│    GitLab     │         │    GitHub     │         │     Jira      │
│   Connector   │         │   Connector   │         │   Connector   │
└───────┬───────┘         └───────┬───────┘         └───────┬───────┘
        │                         │                         │
        ▼                         ▼                         ▼
┌───────────────┐         ┌───────────────┐         ┌───────────────┐
│ GitLab Issues │         │ GitHub Issues │         │  Jira Issues  │
│    API        │         │    REST API   │         │    REST API   │
└───────────────┘         └───────────────┘         └───────────────┘
```

## Core Components

### 1. Integration Manager

Central coordinator for all external integrations.

```rust
pub struct IntegrationManager {
    connectors: Vec<Box<dyn IssueConnector>>,
    sync_config: SyncConfiguration,
    link_store: ExternalLinkStore,
}

pub struct SyncConfiguration {
    /// How often to poll for changes (if not using webhooks)
    poll_interval: Duration,
    /// Default sync direction for new connections
    default_direction: SyncDirection,
    /// Field mapping configuration
    field_mappings: FieldMappingConfig,
    /// Conflict resolution strategy
    conflict_strategy: ConflictStrategy,
}

pub enum SyncDirection {
    /// Import from external to AIDA only
    ImportOnly,
    /// Export from AIDA to external only
    ExportOnly,
    /// Two-way synchronization
    Bidirectional,
}

pub enum ConflictStrategy {
    /// AIDA always wins
    AidaWins,
    /// External always wins
    ExternalWins,
    /// Most recent modification wins
    LastWriteWins,
    /// Manual resolution required
    ManualResolve,
}
```

### 2. Issue Connector Trait

Common interface for all external issue trackers.

```rust
#[async_trait]
pub trait IssueConnector: Send + Sync {
    /// Unique identifier for this connector type
    fn connector_type(&self) -> ConnectorType;

    /// Human-readable name
    fn name(&self) -> &str;

    /// Test connection and authentication
    async fn test_connection(&self) -> Result<ConnectionStatus>;

    /// Fetch all issues from the external system
    async fn fetch_issues(&self, filter: IssueFilter) -> Result<Vec<ExternalIssue>>;

    /// Fetch a single issue by ID
    async fn fetch_issue(&self, external_id: &str) -> Result<Option<ExternalIssue>>;

    /// Create a new issue in the external system
    async fn create_issue(&self, issue: &NewExternalIssue) -> Result<ExternalIssue>;

    /// Update an existing issue
    async fn update_issue(&self, external_id: &str, updates: &IssueUpdates) -> Result<ExternalIssue>;

    /// Fetch comments for an issue
    async fn fetch_comments(&self, external_id: &str) -> Result<Vec<ExternalComment>>;

    /// Add a comment to an issue
    async fn add_comment(&self, external_id: &str, comment: &str) -> Result<ExternalComment>;

    /// Get webhook configuration (if supported)
    fn webhook_config(&self) -> Option<WebhookConfig>;

    /// Handle incoming webhook payload
    async fn handle_webhook(&self, payload: &[u8]) -> Result<Vec<WebhookEvent>>;
}

pub enum ConnectorType {
    GitLab,
    GitHub,
    Jira,
}
```

### 3. External Link Store

Tracks relationships between AIDA requirements and external issues.

```rust
pub struct ExternalLink {
    /// AIDA requirement UUID
    pub requirement_id: Uuid,
    /// External issue identifier
    pub external_id: String,
    /// Which connector this link belongs to
    pub connector_type: ConnectorType,
    /// External system URL (e.g., GitLab instance URL)
    pub instance_url: String,
    /// Project/repository identifier
    pub project_id: String,
    /// When the link was created
    pub created_at: DateTime<Utc>,
    /// Last successful sync
    pub last_synced: Option<DateTime<Utc>>,
    /// Sync direction for this specific link
    pub sync_direction: SyncDirection,
    /// Sync status
    pub status: LinkStatus,
}

pub enum LinkStatus {
    Synced,
    PendingSync,
    ConflictDetected { aida_modified: DateTime<Utc>, external_modified: DateTime<Utc> },
    Error { message: String },
}
```

## Platform-Specific Implementations

### GitLab Connector

```rust
pub struct GitLabConnector {
    base_url: String,           // e.g., "https://gitlab.com" or self-hosted
    access_token: String,       // Personal access token or OAuth
    project_path: String,       // e.g., "group/subgroup/project"
    client: reqwest::Client,
}

impl GitLabConnector {
    /// GitLab-specific features
    pub async fn fetch_milestones(&self) -> Result<Vec<GitLabMilestone>>;
    pub async fn fetch_labels(&self) -> Result<Vec<GitLabLabel>>;
    pub async fn link_merge_request(&self, issue_id: &str, mr_iid: u64) -> Result<()>;
}
```

**GitLab API Endpoints Used:**
- `GET /api/v4/projects/:id/issues` - List issues
- `POST /api/v4/projects/:id/issues` - Create issue
- `PUT /api/v4/projects/:id/issues/:iid` - Update issue
- `GET /api/v4/projects/:id/issues/:iid/notes` - Get comments
- `POST /api/v4/projects/:id/issues/:iid/notes` - Add comment

**GitLab Webhooks:**
- Issue events: create, update, close, reopen
- Note events: new comments

### GitHub Connector

```rust
pub struct GitHubConnector {
    access_token: String,       // Personal access token or GitHub App
    owner: String,              // Organization or username
    repo: String,               // Repository name
    client: reqwest::Client,
}

impl GitHubConnector {
    /// GitHub-specific features
    pub async fn fetch_milestones(&self) -> Result<Vec<GitHubMilestone>>;
    pub async fn fetch_labels(&self) -> Result<Vec<GitHubLabel>>;
    pub async fn link_pull_request(&self, issue_number: u64, pr_number: u64) -> Result<()>;
    pub async fn convert_to_discussion(&self, issue_number: u64) -> Result<()>;
}
```

**GitHub API Endpoints Used:**
- `GET /repos/:owner/:repo/issues` - List issues
- `POST /repos/:owner/:repo/issues` - Create issue
- `PATCH /repos/:owner/:repo/issues/:number` - Update issue
- `GET /repos/:owner/:repo/issues/:number/comments` - Get comments
- `POST /repos/:owner/:repo/issues/:number/comments` - Add comment

**GitHub Webhooks:**
- Issues: opened, edited, closed, reopened
- Issue comments: created, edited

### Jira Connector

```rust
pub struct JiraConnector {
    base_url: String,           // e.g., "https://company.atlassian.net"
    email: String,              // User email
    api_token: String,          // API token
    project_key: String,        // e.g., "PROJ"
    client: reqwest::Client,
}

impl JiraConnector {
    /// Jira-specific features
    pub async fn fetch_issue_types(&self) -> Result<Vec<JiraIssueType>>;
    pub async fn fetch_statuses(&self) -> Result<Vec<JiraStatus>>;
    pub async fn transition_issue(&self, issue_key: &str, transition_id: &str) -> Result<()>;
    pub async fn fetch_sprints(&self) -> Result<Vec<JiraSprint>>;
    pub async fn add_to_sprint(&self, issue_key: &str, sprint_id: u64) -> Result<()>;
}
```

**Jira API Endpoints Used:**
- `GET /rest/api/3/search` - Search issues with JQL
- `POST /rest/api/3/issue` - Create issue
- `PUT /rest/api/3/issue/:key` - Update issue
- `GET /rest/api/3/issue/:key/comment` - Get comments
- `POST /rest/api/3/issue/:key/comment` - Add comment
- `POST /rest/api/3/issue/:key/transitions` - Change status

**Jira Webhooks:**
- issue_created, issue_updated, issue_deleted
- comment_created, comment_updated

## Field Mapping

Each platform has different field names and structures. The field mapping layer handles translation.

```rust
pub struct FieldMappingConfig {
    /// Map AIDA fields to external fields
    mappings: HashMap<AidaField, Vec<ExternalFieldMapping>>,
    /// Custom field handlers
    custom_handlers: Vec<Box<dyn CustomFieldHandler>>,
}

#[derive(Clone)]
pub struct ExternalFieldMapping {
    pub connector_type: ConnectorType,
    pub external_field: String,
    pub transform: Option<FieldTransform>,
}

pub enum FieldTransform {
    /// Direct copy
    Identity,
    /// Map values (e.g., status names)
    ValueMap(HashMap<String, String>),
    /// Custom transformation function
    Custom(String), // Name of registered handler
}
```

### Default Field Mappings

| AIDA Field | GitLab | GitHub | Jira |
|------------|--------|--------|------|
| `title` | `title` | `title` | `summary` |
| `description` | `description` | `body` | `description` |
| `status` | state + labels | state + labels | `status.name` |
| `priority` | labels | labels | `priority.name` |
| `type` | labels | labels | `issuetype.name` |
| `assigned_to` | `assignees` | `assignees` | `assignee` |
| `created_at` | `created_at` | `created_at` | `created` |
| `modified_at` | `updated_at` | `updated_at` | `updated` |
| `created_by` | `author` | `user` | `reporter` |
| `comments` | notes | comments | comments |

## Synchronization Process

### Initial Import

```
1. User configures connector with credentials
2. Test connection and validate access
3. Fetch all issues from external system
4. For each external issue:
   a. Check if already linked to AIDA requirement
   b. If not linked, create new AIDA requirement
   c. Store external link
   d. Apply field mappings
5. Report import summary
```

### Continuous Sync (Polling)

```
1. Get last sync timestamp for connector
2. Fetch issues modified since last sync
3. For each modified issue:
   a. Find linked AIDA requirement
   b. Compare modification timestamps
   c. If external is newer, update AIDA
   d. If AIDA is newer, update external (if bidirectional)
   e. If conflict, apply conflict strategy
4. Fetch AIDA requirements modified since last sync
5. For linked requirements, push changes to external
6. Update sync timestamps
```

### Webhook-Based Sync

```
1. Receive webhook payload
2. Validate webhook signature
3. Parse event type and data
4. Find linked AIDA requirement
5. Apply changes or mark for manual review
6. Optionally trigger reverse sync if bidirectional
```

## Configuration Storage

Integration configurations stored in `requirements.yaml`:

```yaml
integrations:
  gitlab:
    enabled: true
    instance_url: "https://gitlab.example.com"
    project_path: "team/requirements-project"
    sync_direction: bidirectional
    poll_interval_minutes: 15
    # Credentials stored separately in secure storage

  github:
    enabled: false
    owner: "organization"
    repo: "requirements"
    sync_direction: import_only

  jira:
    enabled: true
    instance_url: "https://company.atlassian.net"
    project_key: "REQ"
    sync_direction: export_only
    field_mappings:
      type:
        Requirement: "Story"
        Bug: "Bug"
        Feature: "Epic"
```

Credentials are stored separately using the system keyring or encrypted file.

## Security Considerations

1. **Credential Storage**: Use system keyring (via `keyring` crate) or encrypted storage
2. **API Token Scopes**: Request minimal required permissions
3. **Webhook Validation**: Verify signatures for all webhook payloads
4. **Rate Limiting**: Respect API rate limits with backoff strategies
5. **Audit Logging**: Log all sync operations and changes
6. **Access Control**: Respect external system permissions

## Implementation Phases

### Phase 1: Read-Only Import
- Implement connector trait
- GitLab connector with basic issue fetch
- One-time import command
- External link tracking

### Phase 2: Bidirectional Sync
- Add export capability
- Implement polling-based sync
- Conflict detection and resolution
- Sync status in GUI

### Phase 3: Real-Time Sync
- Webhook endpoints
- Real-time push notifications in GUI
- Background sync worker

### Phase 4: Extended Features
- GitHub and Jira connectors
- Custom field mapping UI
- Bulk operations
- Sync history and rollback

## GUI Integration

The integration features would appear in AIDA GUI as:

1. **Settings Panel**: Configure connectors, credentials, and sync options
2. **Requirement Links**: Show external issue links on requirement detail view
3. **Sync Status**: Icon/badge showing sync state for each requirement
4. **Import Dialog**: Wizard for initial import from external systems
5. **Conflict Resolution**: Dialog for handling sync conflicts
6. **Activity Feed**: Integration events in timeline view

## CLI Integration

```bash
# Configure a connector
aida integration add gitlab --url https://gitlab.com --project group/project

# Test connection
aida integration test gitlab

# Import issues
aida integration import gitlab

# Sync changes
aida integration sync

# Check sync status
aida integration status

# Link existing requirement to external issue
aida link REQ-001 --gitlab issue_id
```

## Error Handling

| Error Type | Handling |
|------------|----------|
| Auth failure | Prompt for new credentials, disable connector |
| Network timeout | Retry with exponential backoff |
| Rate limited | Pause sync, respect retry-after header |
| Field mapping error | Log warning, skip field, continue sync |
| Conflict | Apply strategy or queue for manual resolution |
| Invalid external data | Log error, skip issue, continue sync |

## Testing Strategy

1. **Unit Tests**: Field mapping, data transformation
2. **Integration Tests**: Mock API responses for each platform
3. **E2E Tests**: Against real test projects (GitLab CI, GitHub Actions)
4. **Load Tests**: Sync large numbers of issues

## Dependencies

```toml
[dependencies]
reqwest = { version = "0.11", features = ["json"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
keyring = "2"  # Credential storage
hmac = "0.12"  # Webhook signature validation
sha2 = "0.10"  # Webhook signature validation
async-trait = "0.1"
```

## Future Considerations

1. **Notion Integration**: Support Notion databases as issue source
2. **Azure DevOps**: Work items integration
3. **Linear**: Modern issue tracker support
4. **Slack/Teams**: Notifications for sync events
5. **Custom Webhooks**: Allow custom HTTP endpoints for other tools
6. **GraphQL Support**: For platforms that prefer GraphQL (GitHub, GitLab)
