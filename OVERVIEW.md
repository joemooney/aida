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

### Multi-Project Support (FR-0227)
- **Server-Side Multi-Project Mode** (`aida-server --data-dir <path>`):
  - Each project gets its own isolated SQLite database file
  - ProjectManager manages multiple databases with lazy loading
  - Project registry stored in `projects.json`
  - Automatic migration of legacy `requirements.db` to "default" project
  - Default data directory: `/data` (Docker) or `~/.aida` (local)
- **REST API for Project Management** (port 8080):
  - `GET /api/projects` - List all projects
  - `POST /api/projects` - Create new project (name, description)
  - `GET /api/projects/:name` - Get project info
  - `DELETE /api/projects/:name` - Delete project and its database
- **Request Routing via Headers**:
  - `X-Project` header routes REST requests to correct backend
  - `x-project` gRPC metadata routes gRPC requests
- **Web Client Project Selection**:
  - URL parameter: `?project=name&server=https://api.example.com`
  - Project selector UI when no project specified
  - GrpcStorageClient adds x-project header to all requests
- **Legacy Compatibility**:
  - Single-project mode: `aida-server --database <path>` (no project header required)
  - Environment variable: `AIDA_DATABASE_URL` for single database

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

### React Dashboard (`aida-web-react/`)
- **Stack**: React 19 + Vite 8 + Tailwind CSS 4 + @tanstack/react-query
- **Dev server**: Port 5173, with Vite dev proxy forwarding `/api` to REST API on port 8080
- **Features**:
  - Dashboard metrics (requirement counts by status, priority, type)
  - Kanban board with drag-and-drop (via @dnd-kit)
  - List view with filtering and sorting
  - Detail panel for viewing/editing requirements
  - Full-text search
  - Dark/light theme toggle using CSS custom properties
- **Source organization**: 35 files across `api/`, `lib/`, `hooks/`, `components/` (ui, layout, kanban, list, detail, dashboard)
- **Shared types**: TypeScript types in `shared/types.ts` generated from Rust structs via ts-rs
- **Design choices**: URL-based filter and detail state, optimistic updates for drag-and-drop status changes

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

### AI Integration — Claude Code Scaffolding
AIDA scaffolds Claude Code configuration into projects via `aida init`:
- **15 Skills**: `/aida-req`, `/aida-implement`, `/aida-capture`, `/aida-evaluate`, `/aida-commit`, `/aida-plan`, `/aida-sync`, `/aida-docs`, `/aida-release`, `/aida-test`, `/aida-review`, `/aida-onboard`, `/aida-sprint`, `/aida-search`, `/aida-standup`
- **YAML Frontmatter**: All skills have Claude Code frontmatter (`name`, `description`, `allowed-tools`, `disable-model-invocation`)
- **Dynamic Context Injection**: Skills use `!`command`` to inject live project data at load time
- **Template System**: 4-tier priority (project `.aida/templates/` → org `~/.config/aida/org-templates/` → user `~/.config/aida/templates/` → embedded)
- **MCP Server**: `aida mcp-serve` exposes requirements as native Claude Code tools over JSON-RPC 2.0 stdio
- **Hooks**: `aida-stop-check.sh` (warn about untraced edits), `aida-session-context.sh` (inject project context)
- **Generated artifacts**: `CLAUDE.md`, `.claude/skills/`, `.claude/commands/`, `settings.json`, `.mcp.json`
- **Scaffold version**: 2.0.0

### MCP Server (`aida mcp-serve`)
Model Context Protocol server for Claude Code integration:
- **Tools**: `list_requirements`, `show_requirement`, `add_requirement`, `update_requirement`, `search_requirements`, `add_comment`, `list_features`
- **Resources**: `aida://project/summary`, `aida://requirements/tree`
- JSON-RPC 2.0 over stdio, configured via `.mcp.json`

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
- **Meta Seeding**: `seed_meta_requirements()` creates default AI prompt templates in new databases
- **Prompt Fallback**: AI prompts check database for META requirements first, then fall back to embedded defaults
  - Customize prompts by editing META requirements in GUI/CLI
  - Prompt names: "Evaluate Requirement", "Find Duplicates", "Suggest Relationships", "Improve Description", "Generate Children"

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

## Use Cases & Tutorials

### Use Case 1: Sharing Requirement Templates Between Projects

**Scenario**: You have a well-structured set of security requirements in Project A that you want to reuse in Project B.

**Step-by-step**:

1. **Create a template folder in Project A**:
   ```bash
   aida add --title "Security Requirements Template" --type folder --prefix SEC
   # Creates SEC-001
   ```

2. **Add template requirements as children**:
   ```bash
   aida add --title "Authentication Required" --type functional --description "All API endpoints must require authentication"
   aida rel add --from FR-001 --to SEC-001 --type parent

   aida add --title "Input Validation" --type functional --description "All user inputs must be validated and sanitized"
   aida rel add --from FR-002 --to SEC-001 --type parent
   ```

3. **Export the template tree**:
   ```bash
   aida export --format tree --id SEC-001 -o security-template.json
   ```

4. **Import into Project B**:
   ```bash
   cd /path/to/project-b
   aida import security-template.json --on-conflict rename
   ```

5. **Verify the import**:
   ```bash
   aida list --type folder
   # Shows the imported SEC folder with new SPEC-IDs
   ```

**GUI Alternative**:
- Select the folder in Project A → Menu → "🌳 Export Tree..." → Save file
- Open Project B → Menu → "🌳 Import Tree..." → Select file → Choose conflict strategy → Import

---

### Use Case 2: Customizing AI Prompts for Domain-Specific Evaluation

**Scenario**: You're working on a medical device project and need the AI to evaluate requirements against FDA regulations.

**Step-by-step**:

1. **Locate the META prompts folder**:
   ```bash
   aida list --type meta
   # Shows META-001 "AI Prompts" folder
   ```

2. **Find the evaluation prompt**:
   ```bash
   aida show META-002
   # Shows "Evaluate Requirement" prompt with default template
   ```

3. **Edit the prompt to add domain-specific criteria**:
   ```bash
   aida edit META-002 --description "$(cat <<'EOF'
   Template for evaluating requirement quality.
   Placeholders: {global_context}, {project_context}, {req_context}, {related_context}, {additional_instructions}, {type_extra}

   ---

   You are an expert requirements analyst evaluating a software requirement for a medical device.

   {global_context}
   {project_context}
   {req_context}
   {related_context}
   {additional_instructions}{type_extra}

   ## Task
   Evaluate this requirement considering:
   1. Clarity: Is the requirement unambiguous?
   2. Completeness: Does it have sufficient detail?
   3. Testability: Can this requirement be verified?
   4. Consistency: Does it align with related requirements?
   5. Feasibility: Is it realistic?
   6. **FDA Compliance**: Does it support 21 CFR Part 820 requirements?
   7. **Risk Assessment**: Are safety implications addressed?

   ## Response Format
   Respond ONLY with valid JSON in this exact format:
   {
     "quality_score": <1-10>,
     "issues": [...],
     "strengths": [...],
     "suggested_improvements": {...},
     "fda_compliance_notes": "<any FDA-related observations>"
   }
   EOF
   )"
   ```

4. **Test the custom prompt**:
   - Open GUI → Select a requirement → Click "🤖 AI" → "Evaluate"
   - The AI will now use your FDA-specific evaluation criteria

**GUI Alternative**:
- Settings → Show Meta requirements (check the filter)
- Navigate to META-002 "Evaluate Requirement"
- Edit the description with your custom prompt template
- Save changes

---

### Use Case 3: Setting Up a New Project with Meta Seeding

**Scenario**: You're starting a new project and want the default AI prompts automatically created.

**Step-by-step**:

1. **Initialize a new AIDA database**:
   ```bash
   mkdir my-new-project && cd my-new-project
   aida init
   # Creates requirements.yaml with default configuration
   ```

2. **Verify META requirements were seeded**:
   ```bash
   aida list --type meta
   # Shows:
   # META-001  AI Prompts (folder)
   # META-002  Evaluate Requirement
   # META-003  Find Duplicates
   # META-004  Suggest Relationships
   # META-005  Improve Description
   # META-006  Generate Children
   ```

3. **View a prompt template**:
   ```bash
   aida show META-002
   # Displays the full evaluation prompt with all placeholders
   ```

4. **The AI system automatically uses these**:
   - When you run AI evaluation, it checks META requirements first
   - If not found, falls back to embedded defaults
   - Edit any META prompt to customize behavior project-wide

---

### Use Case 4: Migrating Requirements Between Storage Backends

**Scenario**: Your project has grown and you need better performance than YAML provides.

**Step-by-step**:

1. **Check current database info**:
   ```bash
   aida db info
   # Shows: Backend: YAML, Path: requirements.yaml, Requirements: 500
   ```

2. **Migrate to SQLite**:
   ```bash
   aida db migrate --from yaml --to sqlite
   # Creates requirements.db
   ```

3. **Update your project configuration**:
   ```bash
   aida db add --name "my-project" --path ./requirements.db
   aida db default my-project
   ```

4. **For team environments, migrate to PostgreSQL**:
   ```bash
   aida db migrate --from sqlite --to postgres \
     --output "postgres://user:pass@localhost:5432/aida_prod"
   ```

5. **Use PostgreSQL directly**:
   ```bash
   aida --file "postgres://user:pass@localhost:5432/aida_prod" list
   ```

---

### Use Case 5: GitLab Integration Workflow

**Scenario**: Your team uses GitLab for issue tracking and you want bidirectional sync.

**Step-by-step**:

1. **Create GitLab configuration**:
   ```bash
   mkdir -p ~/.config/aida
   cat > ~/.config/aida/gitlab.toml << 'EOF'
   [gitlab]
   url = "https://gitlab.com"
   project_id = 12345678
   token = "glpat-xxxxxxxxxxxxx"

   [labels]
   type_mapping = { Story = "type::story", Bug = "type::bug", Task = "type::task" }
   priority_mapping = { High = "priority::high", Medium = "priority::medium", Low = "priority::low" }
   status_mapping = { InProgress = "status::in-progress", Completed = "status::done" }

   [polling]
   enabled = true
   interval_seconds = 300
   EOF
   ```

2. **Validate and create missing labels**:
   ```bash
   aida gitlab labels --validate --create-missing
   ```

3. **Link a requirement to an existing GitLab issue**:
   - Open GUI → Select requirement → Links tab → "Link Issue" button
   - Enter GitLab issue number → Link

4. **Create a new GitLab issue from a requirement**:
   - Open GUI → Select requirement → "Create Issue" button
   - Labels are automatically applied based on type/priority/status

5. **Monitor sync status**:
   ```bash
   aida gitlab status --diverged
   # Shows requirements that have changed on either side
   ```

6. **Manual refresh**:
   ```bash
   aida gitlab refresh STORY-042
   # Re-syncs the requirement with GitLab
   ```

**In GUI**:
- Status bar shows `GL:✓` (in-sync) or `GL:⚠` (diverged)
- Toast notifications appear when changes are detected
- Background polling runs automatically

---

### Use Case 6: Building a Reusable Requirements Library

**Scenario**: Your organization wants to maintain a library of reusable requirement templates.

**Step-by-step**:

1. **Create a dedicated library database**:
   ```bash
   mkdir -p ~/aida-library && cd ~/aida-library
   aida init
   ```

2. **Organize by domain**:
   ```bash
   # Create category folders
   aida add --title "Authentication Templates" --type folder --prefix AUTH
   aida add --title "Security Templates" --type folder --prefix SEC
   aida add --title "Accessibility Templates" --type folder --prefix A11Y
   ```

3. **Add template requirements with detailed descriptions**:
   ```bash
   aida add --title "OAuth 2.0 Integration" --type functional \
     --description "## Overview
   The system shall support OAuth 2.0 authentication.

   ## Acceptance Criteria
   - Support Authorization Code flow
   - Support refresh tokens
   - Token expiration handling
   - Secure token storage

   ## Notes
   Customize the provider list for your project."

   aida rel add --from FR-001 --to AUTH-001 --type parent
   ```

4. **Export individual templates or entire categories**:
   ```bash
   # Export entire category
   aida export --format tree --id AUTH-001 -o auth-templates.json

   # Export single template
   aida export --format tree --id FR-001 -o oauth-template.json
   ```

5. **Import into project with conflict handling**:
   ```bash
   cd /path/to/project
   aida import ~/aida-library/auth-templates.json --on-conflict skip
   ```

**Best Practices**:
- Use `--on-conflict skip` to avoid duplicating existing templates
- Use `--on-conflict rename` when you want to keep both versions
- Use `--parent FOLDER-001` to organize imported templates

---

## Documentation

- **README.md**: Quick start and project structure
- **CLAUDE.md**: AI assistant instructions and technical details
- **docs/user-guide.md**: Comprehensive user documentation
- **docs/user-guide.html**: Pre-generated HTML (light mode)
- **docs/user-guide-dark.html**: Pre-generated HTML (dark mode)
