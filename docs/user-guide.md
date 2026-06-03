# AIDA User Guide

An AI-native requirements management system with CLI, web dashboard, and desktop app.

**Related Documentation:**
- [Getting Started](getting-started.md) - Quick setup and first steps
- [Developer's Guide](DEVELOPER_GUIDE.md) - For developers maintaining and extending AIDA
- [Administrator's Guide](admin-guide.md) - For project configuration and administration

## Table of Contents

- [Getting Started](#getting-started)
- [CLI Usage](#cli-usage)
- [Web Dashboard](#web-dashboard)
- [Working with Requirements](#working-with-requirements)
  - [Meta Requirements](#meta-requirements)
- [Features and Organization](#features-and-organization)
- [Multi-Project Support](#multi-project-support)
- [Storage Backends](#storage-backends)
  - [Tree Export/Import](#tree-exportimport)
- [Keyboard Shortcuts](#keyboard-shortcuts)
- [Settings](#settings)
- [Use Cases & Tutorials](#use-cases--tutorials)

---

## Getting Started

For a detailed walkthrough, see the [Getting Started guide](getting-started.md).

### Installation

Build the project from source:

```bash
cargo build --workspace --release
```

This creates two binaries in `target/release/`:
- `aida` — command-line interface (also embeds the MCP server: `aida mcp-serve`)
- `aida-server` — REST API + gRPC server (port 8080)

### Quick Start

1. **Initialize your project:**
   ```bash
   git init               # if not already a git repo
   aida init
   ```
   Default is **distributed git-canonical mode**: creates orphan branch `aida-store` with worktree at `.aida-store/`, SQLite cache at `.aida/cache.db`, scaffolds Claude Code config (`.claude/skills/`, `.claude/commands/`, `.mcp.json`, `CLAUDE.md`, `AGENTS.md`), and creates `docs/plans/`. Pass `--centralized` for the deprecated SQLite-canonical mode.

2. **Add your first requirement:**
   ```bash
   aida add --title "User login" --type story --status draft
   ```

3. **List all requirements:**
   ```bash
   aida list                  # Cache-backed (sub-ms)
   ```

4. **Launch the web dashboard:**
   ```bash
   # Terminal 1: REST API
   cd aida-server && cargo run
   # Terminal 2: React dashboard
   cd aida-web-react && pnpm dev
   # Opens at http://localhost:5173
   ```

---

## CLI Usage

### Basic Commands

| Command | Description |
|---------|-------------|
| `aida list` | List all requirements |
| `aida add` | Add a new requirement |
| `aida show <ID>` | Show requirement details |
| `aida edit <ID>` | Edit a requirement |
| `aida del <ID>` | Delete a requirement |

### Adding Requirements

**Interactive mode:**
```bash
aida add --interactive
```

**Command line mode:**
```bash
aida add --title "Feature name" \
        --description "Detailed description" \
        --priority High \
        --status Draft \
        --type Functional \
        --feature "Authentication"
```

**With custom ID prefix:**
```bash
aida add --title "Security audit" \
        --prefix SEC \
        --description "Perform security audit"
```

### Filtering and Searching

```bash
# Filter by status
aida list --status Approved

# Filter by priority
aida list --priority High

# Filter by feature
aida list --feature "Authentication"

# Search by text
aida list --search "login"
```

### Working with Relationships

```bash
# Add a parent-child relationship
aida rel add --from SPEC-001 --to SPEC-002 --type parent

# Add bidirectional relationship
aida rel add --from SPEC-001 --to SPEC-002 --type verifies -b

# List relationships
aida rel list SPEC-001
```

### Relationship Definitions

Manage custom relationship types with constraints:

```bash
# List all relationship definitions
aida rel-def list

# Show details for a relationship definition
aida rel-def show parent

# Add a custom relationship type
aida rel-def add --name "blocks" \
    --display-name "Blocks" \
    --description "This requirement blocks another" \
    --inverse "blocked_by" \
    --cardinality n:n \
    --color "#ff6b6b"

# Edit a relationship definition
aida rel-def edit parent --source-types "Functional,System"

# Remove a custom relationship definition
aida rel-def remove blocks
```

**Built-in relationship types:**
- `parent` / `child` - Hierarchical decomposition (N:1 / 1:N)
- `verifies` / `verified_by` - Test relationships (N:N)
- `depends_on` / `dependency_of` - Dependencies (N:N)
- `implements` / `implemented_by` - Implementation links (N:N)
- `references` - General reference (N:N, no inverse)
- `duplicate` - Marks duplicates (symmetric)

### Feature Management

```bash
# List features
aida feature list

# Rename a feature
aida feature rename "Old Name" "New Name"

# Move requirements between features
aida feature move SPEC-001 "New Feature"
```

### Database Management

The store is the orphan-branch `aida-store` worktree (distributed mode). There is
no project registry — each repo has one store, attached on init/clone. Manage it
with:

```bash
# Where is the active store?
aida db path

# Store statistics + info
aida db info

# Sync the git-backed store with the remote
aida db sync --pull --push

# Health
aida db status              # changes, sync state, conflicts
aida db check --collisions # audit for two specs claiming one short id

# Distributed-ID housekeeping
aida db merge-gate         # assign agreed short IDs at merge-to-trunk
aida db block claim        # reserve a block of agreed IDs for offline trace comments
aida db reconcile-status   # replay missed Done → Completed auto-bumps
```

### Opening the User Guide

```bash
# Open in default browser (light mode)
aida user-guide

# Open in dark mode
aida user-guide --dark
```

---

## Web Dashboard

The React web dashboard (`aida-web-react/`) is the primary UI for most users. It connects to the REST API server (`aida-server`) running on port 8080.

### Starting the Web Dashboard

```bash
# Terminal 1: Start the REST API server
cd aida-server && cargo run       # Runs on http://localhost:8080

# Terminal 2: Start the React dev server
cd aida-web-react && pnpm dev     # Opens at http://localhost:5173
```

### Views

| View | Shortcut | Description |
|------|----------|-------------|
| **Dashboard** | `g+d` | Project-wide status cards, active sprint summary, queue widget |
| **Kanban Board** | `g+b` | Drag-and-drop columns by status, tag filter dropdown |
| **List View** | `g+l` | Flat/tree toggle, advanced query builder, drag-to-queue, drag-to-reparent |
| **My Queue** | `g+q` | Personal focus inbox with drag-to-reorder, owner-scoped |
| **My Activity** | `g+a` | Planned vs. actual work reconciliation with time range filtering |
| **Sprint Planning** | `g+s` | Drag-and-drop backlog/sprint assignment, burndown/velocity charts |
| **Timeline** | `g+t` | Chronological event feed (history, comments, creation) |
| **Skills Browser** | `g+x` | View and edit Claude Code skills and commands |
| **Chat** | `g+c` | AI-powered Q&A with streaming responses and auto-linked spec IDs |
| **Settings** | — | Store metadata, type definitions, admin controls |

### Search and Filtering

**Structured search** in the search bar supports field-specific queries:
- `owner:joe` — filter by owner
- `tag:frontend` — filter by tag
- `status:approved` — filter by status

**Advanced query builder** (press `f` or click the filter icon):
- AND/OR grouping via react-querybuilder
- Sprint, tags, and custom field support
- Saved queries persisted in localStorage
- URL-persisted via `?aq=` parameter

### Keyboard Shortcuts (Web Dashboard)

Press `?` to see all shortcuts. Key bindings:

| Shortcut | Action |
|----------|--------|
| `g+d/b/l/q/a/s/t/c/x` | Switch views (chord navigation) |
| `j/k` | Navigate rows up/down |
| `Enter` | Open detail panel |
| `/` | Focus search bar |
| `f` | Toggle advanced filter |
| `s/p/o` | Quick pickers for status/priority/owner |
| `q` | Add to queue |
| `?` | Help modal |

### Description Rendering

Markdown descriptions render with:
- Auto-linked spec IDs (e.g., `FR-0042` becomes a clickable link)
- `::color[text]` colored text syntax (20 colors)
- Syntax-highlighted code blocks (Prism oneDark theme)
- Tables, task lists, and standard GFM features

The description editor includes expand/collapse, live preview, and a markdown help toolbar.

### AI Features

- **Chat**: Ask questions about requirements in natural language. Requires `ANTHROPIC_API_KEY` (set via environment variable or Admin settings). Model configurable via `AIDA_CHAT_MODEL`.
- **AI Evaluate**: One-click quality evaluation via the sparkles button in the detail header. Results (score, strengths, issues, suggestions) display inline and persist on the requirement.

---

## Working with Requirements

### Requirement Fields

| Field | Description |
|-------|-------------|
| **SPEC-ID** | Auto-generated identifier (e.g., SPEC-001) |
| **Title** | Short descriptive name |
| **Description** | Detailed explanation (supports Markdown) |
| **Status** | Type-specific status (see Type Definitions below) |
| **Priority** | High, Medium, or Low |
| **Type** | Functional, NonFunctional, System, User, or ChangeRequest |
| **Feature** | Grouping category |
| **Owner** | Person responsible |
| **Tags** | Comma-separated labels |
| **ID Prefix** | Optional custom prefix override (uppercase letters only) |
| **Custom Fields** | Type-specific additional fields (e.g., Impact, Requested By) |

### Custom ID Prefixes

By default, requirement IDs are generated based on the feature and/or type configuration. You can override this by specifying a custom prefix:

- **CLI**: Use `--prefix SEC` when adding a requirement
- **GUI**: Enter the prefix in the "ID Prefix" field (e.g., `SEC`, `PERF`, `API`)

Custom prefixes must contain only uppercase letters (A-Z). Leave blank to use the default prefix derived from feature/type settings.

**Examples:**
- `SEC-001` - Security requirement
- `PERF-001` - Performance requirement
- `API-001` - API requirement

When using "Per Prefix" numbering strategy, each custom prefix gets its own counter. With "Global Sequential" numbering, all requirements share the same counter regardless of prefix.

### Prefix Management

The system tracks all ID prefixes used in the project. You can manage prefixes in **Settings** > **Admin** > **ID Prefix Management**:

**Features:**
- **Prefix filtering**: Filter the requirement list by ID prefix (e.g., show only SEC-xxx or API-xxx requirements)
- **Allowed prefixes list**: Explicitly define which prefixes are permitted in the project
- **Restrict prefixes**: When enabled, users must select from the allowed prefixes list instead of entering custom ones
- **Auto-collection**: New prefixes used are automatically added to the allowed list (unless restriction is enabled)

**Use Cases:**
- Enforce consistent naming conventions across the team
- Quickly filter to see only security, performance, or API-related requirements
- Save filter combinations as view presets for quick access

### Markdown Support

Requirement descriptions support Markdown formatting. When viewing a requirement, the description is rendered with full Markdown support including:

- **Headers** (`# H1`, `## H2`, etc.)
- **Bold** and *italic* text
- Bullet and numbered lists
- Code blocks with syntax highlighting
- Links and images
- Tables
- Task lists

When editing a requirement, click the **👁 Preview** button to see how your Markdown will render. Click **✏ Edit** to return to the text editor.

### Status Workflow

Default statuses for standard requirement types:

```
Draft -> Approved -> Completed
              |
              v
          Rejected
```

- **Draft**: Initial state, work in progress
- **Approved**: Reviewed and accepted
- **Completed**: Implementation finished
- **Rejected**: Not accepted or deprecated

**Note:** Some types (like Change Request) have their own custom status workflows. See Type Definitions below.

### Type Definitions

The system supports configurable requirement types with type-specific statuses and custom fields.

**Built-in Types:**

| Type | Prefix | Statuses | Custom Fields |
|------|--------|----------|---------------|
| **Functional** | FUNC | Draft, Approved, Completed, Rejected | - |
| **NonFunctional** | NFUNC | Draft, Approved, Completed, Rejected | - |
| **System** | SYS | Draft, Approved, Completed, Rejected | - |
| **User** | USER | Draft, Approved, Completed, Rejected | - |
| **ChangeRequest** | CR | Draft, Submitted, Under Review, Approved, Rejected, In Progress, Implemented, Verified, Closed | Impact, Requested By, Target Release, Justification |

**Change Request Workflow:**

```
Draft -> Submitted -> Under Review -> Approved -> In Progress -> Implemented -> Verified -> Closed
                            |
                            v
                        Rejected
```

**Custom Fields:**

When creating or editing a requirement with custom fields, additional form fields appear below the standard fields. Field types include:

- **Text**: Single-line text input
- **Text Area**: Multi-line text input
- **Select**: Dropdown with predefined options
- **Boolean**: Checkbox
- **Date**: Date input (YYYY-MM-DD format)
- **User Reference**: Dropdown to select a user from the system
- **Requirement Reference**: Dropdown to select another requirement
- **Number**: Numeric input

**Managing Type Definitions:**

In the GUI, go to **Settings** > **Types** tab to manage type definitions:

**Viewing Types:**
- Each type is shown in a collapsible section with 📦 (built-in) or 📝 (custom) icons
- Expand a type to see its internal name, prefix, description, statuses, and custom fields

**Editing Types:**
- Click the ✏ button to edit any type (including built-in types)
- Modify the display name, description, and ID prefix
- Add or remove statuses (validation prevents removing statuses that are in use)
- Add, edit, or remove custom fields (validation prevents removing fields that are in use)

**Adding New Types:**
- Click "➕ Add New Type" to create a custom requirement type
- Define internal name, display name, description, and ID prefix
- Configure the available statuses (at least one required)
- Add custom fields with various types (Text, Select, Boolean, Date, etc.)

**Resetting Types:**
- Built-in types show a ↺ button to reset them to their default configuration
- "Reset All to Defaults" restores all built-in types to their original state

**Deleting Types:**
- Custom types (not built-in) can be deleted using the 🗑 button
- Types in use by existing requirements cannot be deleted

### Meta Requirements

Meta requirements are a special type used to store AI prompts, configuration, and templates as browsable requirements within your database. This allows you to customize AI behavior on a per-project basis.

**Meta Subtypes:**

| Subtype | Description |
|---------|-------------|
| **Prompt** | AI prompt templates for evaluation, improvement, etc. |
| **Skill** | AI skill definitions |
| **Command** | Slash command definitions |
| **Template** | Reusable templates |
| **Config** | Project configuration |

**Default AI Prompts:**

When you create a new database, AIDA automatically seeds it with default META requirements:

| SPEC-ID | Title | Purpose |
|---------|-------|---------|
| META-001 | AI Prompts | Folder containing all prompt templates |
| META-002 | Evaluate Requirement | Template for quality evaluation |
| META-003 | Find Duplicates | Template for duplicate detection |
| META-004 | Suggest Relationships | Template for relationship suggestions |
| META-005 | Improve Description | Template for description improvement |
| META-006 | Generate Children | Template for child requirement generation |

**Customizing AI Prompts:**

1. Enable Meta visibility: In the filter panel, check "Show Meta"
2. Navigate to the prompt you want to customize (e.g., META-002 "Evaluate Requirement")
3. Edit the description to modify the prompt template
4. Save changes

The AI system will automatically use your customized prompts. If a META prompt is not found or is empty, the system falls back to embedded defaults.

**Prompt Placeholders:**

When editing prompts, you can use these placeholders:

| Placeholder | Description |
|-------------|-------------|
| `{global_context}` | Global AI context from settings |
| `{project_context}` | Project-specific context |
| `{req_context}` | Current requirement details |
| `{related_context}` | Related requirements |
| `{additional_instructions}` | Extra instructions from type config |
| `{type_extra}` | Type-specific instructions |
| `{all_reqs}` | All requirements (for duplicate detection) |
| `{rel_types}` | Available relationship types |
| `{examples}` | Example requirements |
| `{existing_children}` | Current child requirements |
| `{req_type}` | Current requirement type |

**CLI Commands:**
```bash
# List all META requirements
aida list --type meta

# View a specific prompt
aida show META-002

# Edit a prompt
aida edit META-002 --description "Your custom prompt template here"
```

### Relationship Types

The system includes built-in relationship types with configurable constraints:

| Type | Inverse | Cardinality | Description |
|------|---------|-------------|-------------|
| **Parent** | Child | N:1 | Hierarchical decomposition |
| **Child** | Parent | 1:N | Child of parent requirement |
| **Verifies** | Verified By | N:N | Test/verification relationship |
| **Verified By** | Verifies | N:N | Verified by test requirement |
| **Depends On** | Dependency Of | N:N | Dependency relationship |
| **Dependency Of** | Depends On | N:N | Inverse dependency |
| **Implements** | Implemented By | N:N | Implementation relationship |
| **Implemented By** | Implements | N:N | Inverse implementation |
| **References** | - | N:N | General reference link |
| **Duplicate** | (symmetric) | N:N | Marks as duplicate |

**Cardinality meanings:**
- **1:1** - One source to one target
- **1:N** - One source to many targets
- **N:1** - Many sources to one target
- **N:N** - Many sources to many targets

Custom relationship types can be created with:
- Type constraints (limit which requirement types can participate)
- Cardinality rules
- Inverse relationship definitions
- Visualization colors

---

## Features and Organization

Requirements are organized into numbered features for better management.

### Feature Naming

Features are automatically numbered:
- `1-Authentication`
- `2-User-Management`
- `3-Reporting`

### Default Feature

Requirements without a specified feature go to "Uncategorized". Set a default feature using the `AIDA_FEATURE` environment variable:

```bash
export AIDA_FEATURE="Authentication"
aida add --title "New auth requirement"  # Automatically uses Authentication
```

---

## One store per repo (no project registry)

AIDA tracks **one requirement store per git repository** — the orphan-branch
`aida-store` worktree created by `aida init`. The old multi-project registry
(`~/.aida.config`, `aida db add/default`, `aida list --project`) was removed in
the kernel/module audit; there is nothing to register. To work on a different
project, `cd` into its repo.

### Store resolution order

1. **Distributed mode (default):** `.aida/config.toml` with `mode = "distributed"`
   → the git-canonical store at the configured path (default `.aida-store/`).
2. `--file <path>` explicit override (directory → git store, `.db` → legacy SQLite,
   `.yaml` → legacy YAML, `postgres://…` → PostgreSQL).
3. Legacy fallbacks (deprecated, warn at use): a local `requirements.db` /
   `requirements.yaml` in the current directory.

On a fresh clone the first store-reading command auto-attaches the `.aida-store/`
worktree from origin and rebuilds the cache, so reads work with no manual step
(writing new spec IDs still needs a node id — `aida init` or `aida node acquire`).

---

## Storage backends

`aida init` creates a **git-canonical distributed store** by default — the only
recommended mode. The legacy single-file backends remain only for the deprecated
`--centralized` path and print a warning.

### Git-canonical (default, recommended)

The orphan `aida-store` branch is the **writer of record**: one YAML file per
requirement under `objects/<TYPE>/000/<SPEC-ID>.yaml`. A SQLite cache at
`.aida/cache.db` (gitignored, auto-rebuilt) is a rebuildable read projection that
makes `list`/`search`/`filter` fast.

**Why it's the default:**
- **Portable + vendor-neutral** — the data lives in git, clones cleanly, no SaaS.
- **Diffable history** — every change is a commit on the orphan branch, plus a
  structured `history:` array inside each spec's YAML.
- **Offline-safe distributed IDs** — node-aware IDs (`FR-JM-048`) never collide
  across clones; promoted to short agreed IDs (`FR-048`) at merge-gate.
- **Cache is disposable** — `aida cache rebuild` reprojects it from git anytime.

### Legacy SQLite / YAML (deprecated)

Single-file `requirements.db` / `requirements.yaml` backends still load for the
deprecated `aida init --centralized` path. Don't start new projects on them.

### PostgreSQL (opt-in, `postgres` feature)

A server-backed shared projection for team deployments. Build with
`cargo build --features postgres`, then point clients at it:

```bash
aida --file "postgres://user:pass@localhost:5432/aida" list
```

### Exporting and Importing

You can export your requirements to JSON format for backup or interoperability:

- **JSON export** - Portable format for sharing between systems
- **Migration** - Convert between YAML and SQLite formats

#### Tree Export/Import

Export requirement hierarchies (a requirement and all its descendants) to portable JSON files for sharing between projects:

**CLI Export**:
```bash
# Export a requirement tree to JSON
aida export --format tree --id FOLDER-001 -o templates.json

# The exported file includes:
# - All descendant requirements (children, grandchildren, etc.)
# - Comments and custom fields
# - Internal parent-child relationships
# - Notes about external relationships for manual resolution
```

**CLI Import**:
```bash
# Import into current database
aida import templates.json

# Import under a specific parent
aida import templates.json --parent FOLDER-002

# Handle conflicts with existing requirements
aida import templates.json --on-conflict skip     # Skip if title exists
aida import templates.json --on-conflict rename   # Add "(imported)" suffix
aida import templates.json --on-conflict replace  # Replace existing
```

**GUI Export**:
1. Select the requirement you want to export (typically a Folder)
2. Go to Menu → "🌳 Export Tree..."
3. Choose save location and filename
4. Click Export

**GUI Import**:
1. Go to Menu → "🌳 Import Tree..."
2. Select the JSON file to import
3. Optionally select a parent requirement
4. Choose conflict strategy (Skip, Rename, or Replace)
5. Click Import

**Important Notes**:
- UUIDs and SPEC-IDs are regenerated on import to avoid conflicts
- External relationships (to requirements outside the tree) are noted but not created
- Use this feature to create reusable template libraries

For detailed migration procedures and storage administration, see the [Administrator's Guide](admin-guide.md).

---

## Keyboard Shortcuts

### Desktop App Shortcuts

| Shortcut | Action |
|----------|--------|
| **Arrow Up/Down** | Navigate requirements list |
| **j/k** | Navigate requirements list (vim-style) |
| **Enter** | Edit selected requirement |
| **Double-click** | Edit requirement |
| **Space** | Expand/collapse tree node (in tree views) |
| **e** | Edit selected requirement |
| **n** | New sibling requirement |
| **Shift+N** | New child requirement |
| **f** | Open feature picker |
| **s** | Open status picker |
| **p** | Open priority picker |
| **o** | Open owner picker |
| **Shift+S** | Open sprint picker |
| **d** | Delete with confirmation |
| **Shift+D** | Delete immediately |
| **a** | Toggle archive status |
| **c** | Add comment |
| **Shift+L** | Toggle links panel |
| **Ctrl+S** | Save (in Add/Edit forms) |
| **Ctrl+T** | Cycle through themes |
| **Ctrl+MouseWheel** | Zoom in/out |
| **Ctrl+Shift++** | Zoom in |
| **Ctrl+-** | Zoom out |
| **Ctrl+0** | Reset zoom to base size |
| **/** | Focus search box (vim-style) |
| **Escape** | Clear search/close dialogs |

---

## Settings

### User Profile

Access settings via the gear icon (top-right) in the GUI.

| Setting | Description |
|---------|-------------|
| **Name** | Your full name (used in comments/history) |
| **Email** | Your email address |
| **Handle** | Nickname for @mentions in comments |

### Appearance

| Setting | Description |
|---------|-------------|
| **Theme** | Color scheme (Dark, Light, High Contrast Dark, Solarized Dark, Nord) |
| **Base Font Size** | Default font size (8-32pt) |
| **Default View** | Preferred perspective (Flat List, Parent/Child, etc.) |

### Keyboard Shortcuts

The Keybindings tab shows all customizable keyboard shortcuts. Click "Change" next to any action to set a new key combination. Press Escape to cancel. Click "Reset to Defaults" to restore default bindings.

| Action | Default Key | Default Context |
|--------|-------------|-----------------|
| Navigate Up | Up Arrow | Requirements List |
| Navigate Down | Down Arrow | Requirements List |
| Navigate Up (Vim) | k | Requirements List |
| Navigate Down (Vim) | j | Requirements List |
| Edit Requirement | Enter / e | Requirements List |
| Toggle Expand/Collapse | Space | Requirements List |
| New Sibling | n | Requirements List |
| New Child | Shift+N | Requirements List |
| Open Feature Picker | f | Requirements List |
| Open Status Picker | s | Requirements List |
| Open Priority Picker | p | Requirements List |
| Open Owner Picker | o | Requirements List |
| Delete with Confirm | d | Requirements List |
| Delete Immediate | Shift+D | Requirements List |
| Toggle Archive | a | Requirements List |
| Add Comment | c | Requirements List |
| Zoom In | Ctrl+Shift+Plus | Global |
| Zoom Out | Ctrl+Minus | Global |
| Reset Zoom | Ctrl+0 | Global |
| Cycle Theme | Ctrl+T | Global |
| Save | Ctrl+S | Form |

**Context/Scope:**

Each keybinding has a context that determines where it is active:

| Context | Description |
|---------|-------------|
| **Global** | Works anywhere in the application |
| **Requirements List** | Only when focused on the requirements list (not when typing in text fields) |
| **Detail View** | Only when viewing requirement details |
| **Form** | Only when in add/edit form |

You can change the context for any keybinding using the dropdown in the Settings > Keys tab. This allows you to, for example:
- Make navigation keys work globally
- Restrict certain shortcuts to specific views
- Prevent shortcuts from interfering with text input

User settings are stored in: `~/.aida_gui_settings.yaml`

### Project Settings

Configure how requirement IDs are formatted and numbered. These settings are stored in the project's requirements database.

| Setting | Description |
|---------|-------------|
| **ID Format** | Single Level (PREFIX-NNN) or Two Level (FEATURE-TYPE-NNN) |
| **Numbering** | Global Sequential, Per Prefix, or Per Feature+Type |
| **Digits** | Number of digits in the numeric portion (1-6) |

**ID Format Options:**
- **Single Level**: `AUTH-001`, `FR-002` - Simple prefix with number
- **Two Level**: `AUTH-FR-001`, `PAY-NFR-001` - Feature prefix, type prefix, then number

**Numbering Options:**
- **Global Sequential**: All requirements share one counter (AUTH-001, FR-002, PAY-003)
- **Per Prefix**: Each prefix has its own counter (AUTH-001, FR-001, PAY-001)
- **Per Feature+Type**: Each feature+type combination has its own counter (only for Two Level format)

**Migrating Existing IDs:**

When you change ID configuration settings, you can optionally migrate existing requirement IDs to the new format using the "Migrate Existing IDs" button. The migration has the following constraints:

- **Digit count validation**: You cannot reduce the number of digits below the maximum currently in use. For example, if you have `SPEC-1234` (4 digits), you cannot change to 3 digits.
- **Format change requirement**: To change between Single Level and Two Level formats, you must have Global Sequential numbering selected.
- **Safe operation**: Requirements that cannot be migrated (e.g., would exceed digit limit) are skipped with a warning.

The migration advisor shows:
- Number of requirements that will be affected
- Any validation errors that prevent migration
- Warnings about potential issues

### User Management

Users are managed in Settings > Admin. Each user gets a unique `$USER-XXX` identifier (e.g., `$USER-001`).

**Adding Users:**
1. Go to Settings > Admin
2. Click "➕ Add User"
3. Enter name, email, and handle
4. The system automatically assigns a `$USER-XXX` ID

**User Fields:**
| Field | Description |
|-------|-------------|
| **ID** | Auto-generated `$USER-XXX` identifier |
| **Name** | User's full name |
| **Email** | User's email address |
| **Handle** | Username for @mentions (without @) |
| **Status** | Active or Archived |

**User-Requirement Relationships:**
Users can be linked to requirements through special relationship types:

| Relationship | Description |
|--------------|-------------|
| **Created By** | User who created the requirement |
| **Assigned To** | User responsible for implementing |
| **Tested By** | User(s) who tested/verified the requirement |
| **Closed By** | User who closed/completed the requirement |

These relationships can be added through the Links tab when viewing a requirement.

### Environment Variables

| Variable | Description |
|----------|-------------|
| `AIDA_DB_NAME` | Default project name |
| `AIDA_FEATURE` | Default feature for new requirements |
| `AIDA_REGISTRY_PATH` | Custom registry file location |

---

## Use Cases & Tutorials

### Use Case 1: Sharing Requirement Templates Between Projects

**Scenario**: You have a well-structured set of security requirements in Project A that you want to reuse in Project B.

1. **Create a template folder in Project A**:
   ```bash
   aida add --title "Security Requirements Template" --type folder --prefix SEC
   # Creates SEC-1-001 (node-aware) — promoted to SEC-001 at merge-gate
   ```

2. **Add template requirements as children**:
   ```bash
   aida add --title "Authentication Required" --type functional \
     --description "All API endpoints must require authentication" \
     --parent SEC-1-001
   aida add --title "Input Validation" --type functional \
     --description "All user inputs must be validated and sanitized" \
     --parent SEC-1-001
   ```

3. **Export the template tree**:
   ```bash
   aida export --format tree --id SEC-1-001 -o security-template.json
   ```

4. **Import into Project B**:
   ```bash
   cd /path/to/project-b
   aida import security-template.json --on-conflict rename
   ```

5. **Verify the import**:
   ```bash
   aida list --type folder
   ```

Use `--on-conflict skip` to avoid duplicating templates that already exist, `rename` to keep both copies, or `replace` to overwrite. Pass `--parent FOLDER-001` to graft the imported tree under an existing folder.

---

### Use Case 2: Customizing AI Prompts for Domain-Specific Evaluation

**Scenario**: You're working on a medical device project and need the AI to evaluate requirements against FDA regulations.

1. **Find the evaluation prompt**:
   ```bash
   aida list --type meta            # show all META prompts
   aida show META-002               # "Evaluate Requirement" — default template
   ```

2. **Edit the prompt to add domain-specific criteria**:
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
   Evaluate considering:
   1. Clarity, Completeness, Testability, Consistency, Feasibility
   2. **FDA Compliance**: 21 CFR Part 820 alignment
   3. **Risk Assessment**: safety implications

   ## Response Format
   Respond ONLY with valid JSON:
   {
     "quality_score": <1-10>,
     "issues": [...],
     "strengths": [...],
     "suggested_improvements": {...},
     "fda_compliance_notes": "<observations>"
   }
   EOF
   )"
   ```

3. **Test it**:
   - Web dashboard → select a requirement → Sparkles button → Evaluate
   - The AI now uses your FDA-specific criteria

The default META prompts seeded by `aida init` are: META-002 (Evaluate), META-003 (Find Duplicates), META-004 (Suggest Relationships), META-005 (Improve Description), META-006 (Generate Children). The AI system checks the database first and falls back to embedded defaults.

---

### Use Case 3: GitLab Integration Workflow

**Scenario**: Your team uses GitLab for issue tracking and you want bidirectional sync.

1. **Create GitLab configuration** at `~/.config/aida/gitlab.toml`:
   ```toml
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
   ```

2. **Validate and create missing labels**:
   ```bash
   aida gitlab labels --validate --create-missing
   ```

3. **Link a requirement to an existing GitLab issue** (web dashboard):
   - Open requirement → Links tab → "Link Issue" → enter issue number

4. **Create a new GitLab issue from a requirement** (web dashboard):
   - Open requirement → "Create Issue" — labels are applied from type/priority/status mappings

5. **Monitor sync status**:
   ```bash
   aida gitlab status --diverged   # changed on either side
   aida gitlab refresh STORY-1-042 # manual re-sync
   ```

The dashboard status bar shows `GL:✓` (in-sync) or `GL:⚠` (diverged); background polling raises toasts when changes appear upstream.

---

### Use Case 4: Building a Reusable Requirements Library

**Scenario**: Your organization wants to maintain a library of reusable requirement templates.

1. **Create a dedicated library project**:
   ```bash
   mkdir -p ~/aida-library && cd ~/aida-library
   git init
   aida init
   ```

2. **Organize by domain**:
   ```bash
   aida add --title "Authentication Templates" --type folder --prefix AUTH
   aida add --title "Security Templates"      --type folder --prefix SEC
   aida add --title "Accessibility Templates" --type folder --prefix A11Y
   ```

3. **Add template requirements** with detailed descriptions and a parent folder:
   ```bash
   aida add --title "OAuth 2.0 Integration" --type functional --parent AUTH-1-001 \
     --description "## Overview
   The system shall support OAuth 2.0 authentication.

   ## Acceptance Criteria
   - Authorization Code flow
   - Refresh tokens
   - Token expiration handling
   - Secure token storage"
   ```

4. **Export individual templates or whole categories**:
   ```bash
   aida export --format tree --id AUTH-1-001 -o auth-templates.json
   aida export --format tree --id FR-1-001   -o oauth-template.json
   ```

5. **Import into a project**:
   ```bash
   cd /path/to/project
   aida import ~/aida-library/auth-templates.json --on-conflict skip
   ```

---

## Tips and Best Practices

1. **Use meaningful SPEC-IDs**: Reference requirements by their SPEC-ID in documentation and code comments

2. **Organize by features**: Group related requirements together for better navigation

3. **Track relationships**: Link requirements to tests using "verifies" relationships

4. **Use comments for discussions**: Keep requirement discussions in the comments, not the description

5. **Regular status updates**: Keep status current to track project progress

6. **Backup your data**: SQLite databases can be copied directly; YAML format is also available for git-friendly storage

7. **Use Markdown in descriptions**: Format requirements with headers, lists, and code blocks for clarity

8. **Custom prefixes for cross-cutting concerns**: Use custom ID prefixes like `SEC-`, `PERF-`, `API-` for requirements that span multiple features

9. **Keyboard shortcuts for efficiency**: Learn the shortcuts (f for feature, s for status, j/k for navigation) to speed up your workflow

10. **Set your preferred view**: Configure your default perspective in Settings to match how you like to organize requirements

---

## Troubleshooting

### Common Issues

**"Could not determine requirements file"**
- Run `aida init` to initialize AIDA in the current directory, or
- Register a project with `aida db add`

**"Failed to save"**
- Check file permissions
- Ensure the directory exists

**Web dashboard won't connect**
- Ensure `aida-server` is running on port 8080
- Check that Vite dev proxy is configured (automatic in dev mode)

**Desktop app won't start**
- Ensure you have a display server running
- Check for missing system libraries (OpenGL, etc.)

### Getting Help

- Run `aida --help` for CLI help
- Run `aida <command> --help` for command-specific help
- Open this guide with `aida user-guide`

---

*Generated for AIDA v0.1.0*
