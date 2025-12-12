# Requirements Manager - Project Overview

A professional requirements management system built in Rust, providing both CLI and GUI interfaces for managing software requirements with rich features including relationships, comments, history tracking, and multi-project support.

## Vision

Create a lightweight, file-based requirements management tool that is:
- Version-control friendly (YAML storage)
- Flexible enough for different project needs
- Usable from both command line and graphical interface
- Capable of tracking requirement relationships and history

## Project Structure

This is a Cargo workspace with four crates:

```
aida/
├── aida-core/           # Shared library - models, storage, business logic
├── aida-cli/            # CLI tool (aida binary)
├── aida-gui/            # GUI application (aida-gui binary, egui-based)
├── aida-server/         # gRPC server for headless/remote operation
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
- Types: Functional, Non-Functional, System, User, Change Request (with type-specific workflows)
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

## Technology Stack

- **Language**: Rust
- **GUI Framework**: egui (cross-platform)
- **Storage**: YAML (serde_yaml), SQLite (rusqlite)
- **CLI Framework**: clap
- **Interactive Prompts**: inquire
- **gRPC/RPC**: tonic, prost (Protocol Buffers)
- **Async Runtime**: tokio

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

### Migration & Export
- Migrate between YAML and SQLite formats
- JSON import/export for interoperability

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

# Build CLI with remote feature
cargo build -p aida-cli --features remote

# GUI with remote server
cargo build -p aida-gui --features remote  # Build GUI with remote support
aida-gui --server localhost:50051          # Connect to remote server

# Database migration
aida db info                              # Show database info and statistics
aida db migrate --from yaml --to sqlite   # Migrate YAML to SQLite
aida db migrate --from sqlite --to yaml   # Export SQLite back to YAML

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
