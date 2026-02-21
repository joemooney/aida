# AIDA — AI Design Assistant

A requirements management system built in Rust with CLI, native GUI, web dashboard, and AI integration via Claude Code.

## Project Structure

Cargo workspace with six crates plus a React dashboard:

```
aida/
├── aida-core/             # Shared library — models, storage, business logic
├── aida-cli/              # CLI tool (aida binary)
├── aida-gui/              # Native + WASM GUI (egui, dual-target)
├── aida-server/           # gRPC + REST server for headless/remote operation
├── aida-web/              # Lightweight WASM browser client
├── aida-generate-types/   # TypeScript type generation from Rust structs
├── aida-web-react/        # React dashboard (Vite + Tailwind + React Query)
├── proto/                 # Protocol Buffers definitions
├── shared/                # Shared TypeScript types (generated)
└── docs/                  # User documentation and implementation plans
```

## Quick Start

```bash
# Build everything
cargo build --workspace

# Run CLI
aida list                              # List requirements
aida add --title "..." --description "..." --status draft
aida show FR-0042                      # Show requirement details
aida search "authentication"           # Search requirements

# Start the server (gRPC on 50051, REST on 8080)
cargo run -p aida-server -- --database requirements.db --rest-port 8080

# Run the React dashboard (dev server on port 5173)
cd aida-web-react && npm install && npm run dev

# Launch the native GUI
cargo run -p aida-gui

# Initialize AIDA in a new project
aida init
```

## Features

### Requirements Management
- Full CRUD with SPEC-ID system (human-friendly IDs like FR-0042 alongside UUIDs)
- Types: Functional, Non-Functional, System, User, Bug, Epic, Story, Task, Spike, Sprint, Folder, Meta
- Relationships: Parent/Child, Verifies, References, Duplicate, Custom
- Threaded comments with reactions, full change history tracking
- Feature-based organization, tags, custom fields, custom type workflows

### Storage Backends
- **YAML** — Human-readable, git-friendly (default)
- **SQLite** — Concurrent access with optimistic locking
- **PostgreSQL** — Enterprise-grade with native JSONB and connection pooling
- Migrate between any backends: `aida db migrate --from yaml --to sqlite`

### Interfaces
- **CLI** (`aida`) — Full-featured with interactive prompts, search, export/import
- **Native GUI** (`aida-gui`) — egui-based with multiple views, drag-and-drop, AI evaluation
- **WASM GUI** — Same codebase as native, runs in browser via trunk
- **React Dashboard** (`aida-web-react/`) — Modern web UI on port 5173
  - Dashboard with metrics charts
  - Kanban board with drag-and-drop status changes
  - List view with sorting and filtering
  - Sprint planning with backlog management and charts (burndown, burn-up, velocity)
  - Skills browser — view, search, and edit skills/commands with markdown preview
  - Dark/light theme toggle

### Server
- **gRPC + gRPC-Web** on port 50051 — full API for native and browser clients
- **REST API** on port 8080 — JSON endpoints for the React dashboard
- Multi-project support with isolated databases per project
- Single-project legacy mode for simple setups

### AI Integration — Claude Code
AIDA scaffolds Claude Code configuration into projects via `aida init`:
- **15 skills** (`/aida-req`, `/aida-implement`, `/aida-commit`, `/aida-test`, `/aida-sprint`, etc.)
- **MCP Server** (`aida mcp-serve`) — native Claude Code tool integration
- Dynamic context injection, template system, commit hooks
- Meta requirements for customizable AI prompts stored in the database

### GitLab Integration
- Bidirectional issue sync with label mapping
- Background polling with conflict detection
- CLI and GUI support

## Technology Stack

| Layer | Technology |
|-------|-----------|
| Language | Rust |
| CLI | clap, inquire |
| Native GUI | egui (cross-platform) |
| Web Dashboard | React 19, Vite 8, Tailwind CSS 4, @tanstack/react-query |
| Server | axum (REST), tonic (gRPC), tokio |
| Storage | serde_yaml, rusqlite, postgres + r2d2 |
| WASM | trunk, tonic-web-wasm-client |
| Protocols | Protocol Buffers (prost), JSON-RPC 2.0 (MCP) |

## Documentation

- **OVERVIEW.md** — Detailed project overview with all features and use cases
- **CLAUDE.md** — AI assistant instructions and development workflow
- **docs/user-guide.md** — Comprehensive user documentation
- **docs/plans/** — Archived implementation plans

## Development

```bash
# Run tests
cargo test --workspace

# Build with remote client support
cargo build -p aida-cli --features remote
cargo build -p aida-gui --features remote

# Build WASM client
make web-build && make web-serve    # Serves on port 8088

# Database migration
aida db info                         # Show database statistics
aida db migrate --from yaml --to sqlite
aida db migrate --from sqlite --to postgres --output "postgres://user:pass@host:5432/db"

# Tree export/import
aida export --format tree --id FOLDER-001 -o templates.json
aida import templates.json --parent FOLDER-002 --on-conflict skip
```
