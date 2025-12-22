# Requirements Manager - Project Overview

A professional requirements management system built in Rust, providing both CLI and GUI interfaces for managing software requirements with rich features including relationships, comments, history tracking, and multi-project support.

## Vision

Create a lightweight, file-based requirements management tool that is:
- Version-control friendly (YAML storage)
- Flexible enough for different project needs
- Usable from both command line and graphical interface
- Capable of tracking requirement relationships and history

## Project Structure

This is a Cargo workspace with five crates:

```
aida/
├── aida-core/           # Shared library - models, storage, business logic
├── aida-cli/            # CLI tool (aida binary)
├── aida-gui/            # GUI application (native + WASM dual-target, egui-based)
├── aida-server/         # gRPC server for headless/remote operation
├── aida-web/            # Lightweight WASM browser client (alternative)
├── proto/               # Protocol Buffers definitions
├── docs/                # User documentation (markdown + HTML)
└── helper/              # Helper scripts for documentation generation
```

## Key Features

### Dual Interface
- **CLI (`req`)**: Full-featured command-line interface for scripting and quick operations
- **GUI (`req-gui`)**: Modern egui-based graphical interface with tabbed views

### SPEC-ID System
Human-friendly identifiers (SPEC-001, SPEC-002) alongside internal UUIDs. Configurable ID formats with feature-based prefixes. ID prefix filtering and management with optional admin-controlled restriction.

### Requirement Management
- Full CRUD operations (Create, Read, Update, Delete)
- Type-specific status states (e.g., Draft, Approved, Completed, Rejected for standard types)
- Priority levels: High, Medium, Low
- Types: Functional, Non-Functional, System, User, Change Request, Bug, Epic, Story, Task, Spike, Sprint, Folder, Meta
  - Standard requirement types (Functional, Non-Functional, System, User, Change Request) with type-specific workflows
  - Agile types (Epic, Story, Task, Bug, Spike, Sprint) for project management
  - Organizational types (Folder for hierarchy, Meta for storing templates/configuration)
- Feature-based organization with numbered prefixes
- Tag support for flexible categorization
- Custom fields support for type-specific data (e.g., Impact, Requested By for Change Requests)

### Relationships
Define connections between requirements:
- **Parent/Child**: Hierarchical relationships
- **Verifies/VerifiedBy**: Test/verification traceability
- **References**: General reference links
- **Duplicate**: Mark duplicate requirements
- **Custom**: User-defined relationship types

### Comments & History
- Threaded comment system with replies
- Configurable emoji reactions on comments
- Full change history tracking for requirements
- User attribution with handles for @mentions

### Custom Type Definitions
- Type-specific status workflows (e.g., Change Request has: Draft → Submitted → Under Review → Approved → In Progress → Implemented → Verified → Closed)
- Custom fields per type with multiple field types (Text, TextArea, Select, Boolean, Date, User, Requirement, Number)
- Built-in type definitions for Functional, NonFunctional, System, User, and ChangeRequest types
- Settings UI for viewing type definitions

### Multi-Project Support
- Central registry (~/.requirements.config) for managing multiple projects
- Environment variable support (REQ_DB_NAME, REQ_FEATURE, REQ_REGISTRY_PATH)
- Project resolution with priority ordering

### Headless Server Mode (FR-0227)
- **gRPC Server (`aida-server`)**: Headless server exposing full API via gRPC
- Protocol Buffers schema defining all requirement operations
- Remote CLI operations via `--server` flag or `AIDA_SERVER` environment variable
- Server commands: `aida server status`, `aida server list`, `aida server get <ID>`, `aida server ping`
- Configurable port (default 50051), host, database path, and logging
- **GUI Remote Client**: Connect GUI to remote server with `aida-gui --server <addr>`
  - Requires `--features remote` at build time
  - StorageBackend abstraction for transparent local/remote switching
- **gRPC-Web Support**: Server supports gRPC-Web protocol for browser clients
- **REST API**: HTTP/JSON endpoints for external integration (port 8080)

### Unified Storage Architecture (FR-0278)
- **StorageClient Trait**: Unified interface for both local and remote storage access
  - `load()`, `save()`, `create_requirement()`, `update_requirement()`, `delete_requirement()`
  - `add_comment()`, `add_relationship()`, `get_server_status()`
- **GrpcStorageClient**: gRPC-based implementation of StorageClient
  - Connects to aida-server via tonic gRPC client
  - Converts between Rust types and Protocol Buffer messages
  - Blocking async operations using tokio runtime
- **EmbeddedServer**: Native desktop wrapper for local storage (native feature)
  - Spawns aida-server as subprocess on localhost
  - Auto-discovers available port
  - Graceful shutdown on drop (SIGTERM)
- **Architecture Benefits**:
  - Consistent storage interface across native/web platforms
  - Reduced conditional compilation in business logic
  - Server handles all database operations (YAML/SQLite)

### WASM Browser Client (FR-0273)
- **Dual-Target GUI (`aida-gui`)**: Same codebase for native desktop and WASM browser
  - Full-featured web client with nearly identical UI to desktop
  - Uses conditional compilation to gate native-only features
  - Native-only: threads, file system, AI evaluation, edit locks
  - Web: uses gRPC-Web protocol via `tonic-web-wasm-client`
  - Build: `make web-build` or `cd aida-gui && trunk build`
  - Serve: `make web-serve` (port 8088)
- **Lightweight Client (`aida-web`)**: Alternative simplified browser client
  - Separate crate for minimal WASM bundle size
  - Build: `make web-build-lite`
  - Serve: `make web-serve-lite`
- Both clients:
  - Built with `trunk` (Rust WASM build tool)
  - Use `eframe`/`egui` for UI (same framework as native GUI)
  - Connect to server via gRPC-Web protocol

### Shared UI Components
- **Reusable egui components** in `aida-gui/src/ui/` for native and WASM
  - `formatters.rs`: Text formatters for status/priority/type/timestamps
  - `badges.rs`: Colored badge/dot rendering
  - `list_item.rs`: Requirement list item rendering
  - `requirement_form.rs`: Form components with combo boxes
  - `comment_list.rs`: Comment rendering and input
  - `detail_view.rs`: Full requirement detail view
- `aida-web` re-exports proto types from `aida-gui` for type compatibility
- Consistent UI rendering between native desktop and browser clients

### GUI-Specific Features
- Multiple view perspectives (Flat, Parent/Child, Verification, References)
- Two-level filtering (Root/Children) for hierarchical views
- User settings (name, email, handle, font size)
- Zoom controls (Ctrl+MouseWheel, keyboard shortcuts)
- Collapsible comment trees
- Tabbed interface (Description, Comments, Links, History)
- **Personal Work Queue**: User-managed priority inbox
  - Rankings 1-100 (lower = higher priority)
  - Same-rank items use requirement priority as tiebreaker
  - Hotkeys: `q t` (top), `q m` (middle), `q b` (bottom), `q d` (remove)
  - Queue view: `q v` or `v q`
  - Reorder: `Ctrl+Up/Down` or `Ctrl+k/j` in queue view
  - Stored per-user in settings (~/.config/aida/aida_gui_settings.yaml)

### GitLab Integration (STORY-0321 - STORY-0327)
Bidirectional integration with GitLab for issue tracking:

- **Configuration** (~/.config/aida/gitlab.toml):
  - GitLab URL (gitlab.com or self-hosted)
  - Project ID and Personal Access Token
  - Label mappings for types, priorities, and statuses
  - Polling interval (default 5 minutes)
  - Sync mode and conflict resolution settings

- **Issue Linking**:
  - View existing GitLab issues (`aida gitlab issues`)
  - Create new GitLab issue from requirement (GUI: "Create Issue" button)
  - Link to existing issue (GUI: "Link Issue" button)
  - Automatic bidirectional links (AIDA → GitLab, GitLab → AIDA)

- **Label Mapping**:
  - Map requirement types to GitLab labels (e.g., Story → type::story)
  - Map priorities to labels (e.g., High → priority::high)
  - Map statuses to labels (e.g., InProgress → status::in-progress)
  - CLI: `aida gitlab labels --validate --create-missing`

- **Sync State Tracking**:
  - Content hashing (SHA256) for change detection
  - Sync status: InSync, AidaModified, GitLabModified, Conflict
  - CLI: `aida gitlab status [--diverged]`
  - CLI: `aida gitlab refresh [ID]` for manual sync

- **Background Polling** (GUI):
  - Automatic periodic polling for GitLab changes
  - Status bar indicator: GL:✓ (in-sync) / GL:⚠ (diverged)
  - Toast notifications for detected changes
  - Configurable poll interval

## Technology Stack

- **Language**: Rust
- **GUI Framework**: egui (cross-platform, native and WASM)
- **Storage**: YAML (serde_yaml), SQLite (rusqlite), PostgreSQL (postgres, r2d2)
- **CLI Framework**: clap
- **Interactive Prompts**: inquire
- **gRPC/RPC**: tonic, prost (Protocol Buffers)
- **gRPC-Web**: tonic-web (server), tonic-web-wasm-client (browser)
- **WASM Build Tool**: trunk
- **Async Runtime**: tokio (native), browser-native (WASM)

## Data Storage

Requirements are stored using a pluggable backend system:

### YAML Backend (Default)
- Human-readable YAML format (`requirements.yaml`)
- Git-friendly for version control
- Includes metadata, relationships, comments, and history

### SQLite Backend
- High-performance database storage (`.db` files)
- WAL mode for better concurrent access
- Efficient single-record CRUD operations
- Complex fields (relationships, comments, history) stored as JSON
- **Optimistic Locking (REQ-0231)**: Per-record version columns prevent concurrent edit conflicts
  - Each requirement/user has a `version` field incremented on update
  - Updates with stale versions are rejected with conflict details
  - Store-level `store_version` for detecting any external modifications

### PostgreSQL Backend (FR-0316)
- Enterprise-grade database for multi-user/team deployments
- Connection pooling via r2d2 (max 10 connections)
- Native JSONB storage for complex fields (relationships, comments, history)
- Optimistic locking with version columns
- Connection string format: `postgres://user:password@host:port/database`

### Migration & Export
- Migrate between YAML, SQLite, and PostgreSQL formats
- JSON import/export for interoperability
- **Tree Export/Import**: Export requirement hierarchies to portable JSON files
  - Export a requirement and all descendants: `aida export --format tree --id FR-0001 -o tree.json`
  - Import into another database: `aida import tree.json [--parent FOLDER-001] [--on-conflict skip|rename|replace]`
  - GUI: Menu > "🌳 Export Tree..." and "🌳 Import Tree..."
  - Preserves all fields, comments, custom data, and parent-child relationships
  - UUIDs and spec_ids are regenerated on import to avoid conflicts
  - Use cases: Share templates, backup hierarchies, create reusable libraries

### Meta Requirements
- **Meta type**: Store AI prompts, skills, and configuration as browsable requirements
- **MetaSubtype** categorization: Prompt, Skill, Command, Template, Config
- Stateless type (no status workflow) with prefix "META"
- Enables editing and versioning of AI prompts within the requirements database

## Getting Started

```bash
# Build
cargo build --workspace --release

# CLI usage
aida list                         # List requirements
aida add --interactive            # Add requirement interactively
aida show FR-0001                 # Show requirement details
aida rel add --from FR-0001 --to FR-0002 --type parent  # Add relationship

# GUI usage
aida-gui                          # Launch graphical interface

# Server mode
aida-server --port 50051          # Start gRPC server
aida --server localhost:50051 server list  # Remote list
aida --server localhost:50051 server ping  # Check connectivity

# WASM browser client
make web-deps                     # Install trunk and wasm32 target
make web-build                    # Build WASM client
make web-serve                    # Serve on http://localhost:8088

# Build CLI with remote feature
cargo build -p aida-cli --features remote

# GUI with remote server
cargo build -p aida-gui --features remote  # Build GUI with remote support
aida-gui --server localhost:50051          # Connect to remote server

# Database migration
aida db info                              # Show database info and statistics
aida db migrate --from yaml --to sqlite   # Migrate YAML to SQLite
aida db migrate --from sqlite --to yaml   # Export SQLite back to YAML
aida db migrate --from sqlite --to postgres --output "postgres://user:pass@host:5432/db"

# Use PostgreSQL directly
aida --file "postgres://user:pass@localhost:5432/aida" list

# Open user guide
aida user-guide                   # Open in browser (light mode)
aida user-guide --dark            # Open in browser (dark mode)
```

## Documentation

- **README.md**: Quick start and project structure
- **CLAUDE.md**: AI assistant instructions and technical details
- **docs/user-guide.md**: Comprehensive user documentation
- **docs/user-guide.html**: Pre-generated HTML (light mode)
- **docs/user-guide-dark.html**: Pre-generated HTML (dark mode)
