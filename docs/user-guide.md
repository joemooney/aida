# AIDA User Guide

A professional requirements management system with both CLI and GUI interfaces.

**Related Documentation:**
- [Developer's Guide](DEVELOPER_GUIDE.md) - For developers maintaining and extending AIDA
- [Administrator's Guide](admin-guide.md) - For project configuration and administration

## Table of Contents

- [Getting Started](#getting-started)
- [CLI Usage](#cli-usage)
- [GUI Usage](#gui-usage)
- [Working with Requirements](#working-with-requirements)
  - [Meta Requirements](#meta-requirements)
- [Features and Organization](#features-and-organization)
- [Multi-Project Support](#multi-project-support)
- [Storage Backends](#storage-backends)
  - [Tree Export/Import](#tree-exportimport)
- [Keyboard Shortcuts](#keyboard-shortcuts)
- [Settings](#settings)

---

## Getting Started

### Installation

Build the project from source:

```bash
cargo build --workspace --release
```

This creates two binaries in `target/release/`:
- `aida` - Command-line interface
- `aida-gui` - Graphical user interface

### Quick Start

1. **Create your first requirement:**
   ```bash
   aida add --title "User login" --description "Users can log in with email and password"
   ```

2. **List all requirements:**
   ```bash
   aida list
   ```

3. **Or launch the GUI:**
   ```bash
   aida-gui
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

```bash
# List registered projects
aida db list

# Add a new project
aida db add --name "my-project" --path "/path/to/requirements.yaml"

# Set default project
aida db default "my-project"

# Remove a project
aida db remove "my-project"
```

### Opening the User Guide

```bash
# Open in default browser (light mode)
aida user-guide

# Open in dark mode
aida user-guide --dark
```

---

## GUI Usage

Launch the GUI application:
```bash
aida-gui
```

Or open a specific requirements file:
```bash
aida-gui --file /path/to/requirements.yaml
```

### Main Interface

The GUI consists of three main areas:

1. **Top Bar** - Contains action buttons, requirement count, and settings
2. **Left Panel** - Requirements list with search filter (collapsible in edit mode)
3. **Main Area** - Detail view, forms, or welcome screen

### Responsive Layout

When editing or adding requirements, the left panel remains visible if the window is wide enough (900+ pixels). You can:
- Click the **▶ Hide** button in the left panel header to collapse it
- Click **◀ Show List** in the form area to restore the panel

This allows you to reference other requirements while editing, or maximize form space on smaller screens.

### Navigation

- Click a requirement in the left panel to view its details
- Double-click a requirement to open it for editing
- Use the search box to filter requirements by title or description
- Click tabs (Description, Comments, Links, History) to switch views
- Use arrow keys to navigate up/down through the requirements list
- Press Enter to edit the selected requirement
- Press Space to expand/collapse tree nodes (in hierarchical views)

### Actions

| Button | Action |
|--------|--------|
| **+ Add** | Create a new requirement |
| **Reload** | Refresh from disk |
| **Edit** | Edit selected requirement |
| **Delete** | Delete selected requirement |
| **Settings** | Open settings dialog |
| **Help** | Open this user guide |

### View Perspectives

The requirements list can be displayed in different organizational views. Select your preferred view from the dropdown in the top bar or set a default in Settings > Appearance.

| View | Description |
|------|-------------|
| **Flat List** | Simple list of all requirements |
| **Parent/Child** | Hierarchical tree showing parent-child relationships |
| **Verification** | Groups requirements by verification relationships |
| **References** | Groups requirements by reference relationships |

In tree views, use the +/- buttons or press Space to expand/collapse nodes.

### View Presets

You can save your current view configuration (perspective, direction, and filters) as a named preset for quick access later.

**To save a preset:**
1. Configure your view using the View dropdown, direction selector, and filter options
2. When you have unsaved changes, a "💾 Save As..." button appears
3. Click it, enter a name, and click Save

**To use a preset:**
- Select it from the View dropdown under "Saved Presets"
- The preset will restore all saved settings (perspective, direction, root filters, child filters)

**To delete a preset:**
- Click the ✕ button next to the preset name in the dropdown
- Confirm deletion in the dialog

**Modified indicator:**
- If you modify an active preset, its name shows with an asterisk (e.g., "My View*")
- Use "Save As..." to save changes to the same name or create a new preset

**Reset button (↺):**
- Click to return to the default Flat List view with no filters

### Filtering Requirements

The GUI provides powerful filtering capabilities to help you focus on specific subsets of requirements.

**Accessing Filters:**
- Click the "Filters" dropdown button in the top bar
- The filter panel shows two tabs: **Root** and **Children**

**Root vs Children Filters:**
- **Root filters**: Apply to top-level requirements displayed in the list or tree view
- **Children filters**: Apply to nested requirements in hierarchical views (Parent/Child, Verification, References)
- By default, "Same as root" is checked, meaning children use the same filters as root requirements
- Uncheck "Same as root" to set independent filters for child requirements

This two-level filtering allows you to:
- Show only specific root requirements (e.g., all Functional Requirements)
- While displaying all their children regardless of type
- Or filter children independently (e.g., show all root requirements but only their Change Request children)

**Filter Types:**
- **Type Filters**: Show only FR (Functional), NFR (Non-Functional), SR (System), UR (User), or CR (Change Request)
- **Feature Filters**: Show only requirements from selected features
- **ID Prefix Filters**: Show only requirements with specific ID prefixes (e.g., SEC, API, AUTH)
- **Show Archived**: Toggle visibility of archived requirements

**Quick Actions:**
- Click "Clear" next to any filter category to remove all selections in that category
- Empty filters (none selected) means "show all" for that category

### Adding Comments

1. Select a requirement
2. Click the "Comments" tab
3. Click "+ Add Comment"
4. Enter your comment and click Save

Comments support threading - click "Reply" on any comment to add a nested reply. Comments are displayed in a collapsible tree structure.

### Comment Reactions

You can add emoji reactions to comments to quickly indicate your response:

**Adding a reaction:**
- Click the 😊 button on any comment to open the reaction picker
- Select an emoji to add your reaction
- Click the same emoji again to remove your reaction

**Default reactions:**
| Emoji | Name | Use Case |
|-------|------|----------|
| ✅ | Resolved | Mark comment as addressed |
| ❌ | Rejected | Mark comment as declined |
| 👍 | Thumbs Up | Agree or approve |
| 👎 | Thumbs Down | Disagree or disapprove |
| ❓ | Question | Needs clarification |
| ⚠️ | Important | Attention needed |

**Customizing reactions:**
- Go to Settings > Reactions tab
- Add new custom reactions with your own emoji and labels
- Edit existing reactions (emoji, label, description)
- Delete custom reactions (built-in reactions cannot be deleted)
- Reset to defaults to restore the standard reaction set

### Links Tab

The Links tab provides two sections for connecting requirements to other resources:

**External Links:**
- Click "+ New URL" to add an external URL link
- Enter the URL, optional title, and description
- Click "Verify" to validate the URL format
- Links show verification status (✅ valid, ❌ invalid)
- Click a link to open it in your browser
- Edit or remove links using the ✏ and x buttons

**Relationships:**
- View and manage relationships to other requirements
- Double-click a related requirement to navigate to it
- Remove relationships using the x button
- See relationship types with color indicators

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

## Multi-Project Support

Manage multiple requirement sets using the registry system.

### Registry Location

Default: `~/.aida.config`

Override with: `AIDA_REGISTRY_PATH` environment variable

### Project Resolution Order

1. Local `requirements.yaml` in current directory
2. `--project` command line option
3. `AIDA_DB_NAME` environment variable
4. Single project in registry (if only one exists)
5. Default project from registry
6. Interactive prompt

### Example Setup

```bash
# Register projects
aida db add --name "frontend" --path ~/projects/frontend/requirements.yaml
aida db add --name "backend" --path ~/projects/backend/requirements.yaml

# Set default
aida db default frontend

# Work with specific project
aida list --project backend
```

---

## Storage Backends

The system supports two storage formats for your requirements data.

### YAML Storage (Default)

Requirements are stored in human-readable YAML files (`.yaml` or `.yml`).

**Advantages:**
- Human-readable and editable with any text editor
- Version control friendly - meaningful Git diffs
- Easy to backup (just copy the file)

**Best for:**
- Small to medium projects
- Teams using Git for collaboration
- When you need to manually inspect or edit data

### SQLite Storage

For larger projects, SQLite database storage (`.db`, `.sqlite`) offers better performance.

**Advantages:**
- Faster for large datasets (1000+ requirements)
- Better concurrent access handling
- Efficient single-record operations

**Best for:**
- Large projects with many requirements
- Scenarios with frequent concurrent access
- When performance is critical

### Choosing Your Storage Format

The system automatically detects the storage format based on file extension:
- `.yaml`, `.yml` → YAML storage
- `.db`, `.sqlite`, `.sqlite3` → SQLite storage

When registering a project, simply use the appropriate file extension:

```bash
# YAML storage (default)
aida db add --name "my-project" --path ~/project/requirements.yaml

# SQLite storage
aida db add --name "my-project" --path ~/project/requirements.db
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

### GUI Shortcuts

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

Configure how requirement IDs are formatted and numbered. These settings are stored in the project's `requirements.yaml` file.

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

The migration dialog shows:
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

## Tips and Best Practices

1. **Use meaningful SPEC-IDs**: Reference requirements by their SPEC-ID in documentation and code comments

2. **Organize by features**: Group related requirements together for better navigation

3. **Track relationships**: Link requirements to tests using "verifies" relationships

4. **Use comments for discussions**: Keep requirement discussions in the comments, not the description

5. **Regular status updates**: Keep status current to track project progress

6. **Backup your data**: The YAML format is human-readable and version-control friendly

7. **Use Markdown in descriptions**: Format requirements with headers, lists, and code blocks for clarity

8. **Custom prefixes for cross-cutting concerns**: Use custom ID prefixes like `SEC-`, `PERF-`, `API-` for requirements that span multiple features

9. **Keyboard shortcuts for efficiency**: Learn the shortcuts (f for feature, s for status, j/k for navigation) to speed up your workflow

10. **Set your preferred view**: Configure your default perspective in Settings to match how you like to organize requirements

---

## Troubleshooting

### Common Issues

**"No requirements file found"**
- Create a `requirements.yaml` in the current directory, or
- Register a project with `aida db add`

**"Failed to save"**
- Check file permissions
- Ensure the directory exists

**GUI won't start**
- Ensure you have a display server running
- Check for missing system libraries (OpenGL, etc.)

### Getting Help

- Run `aida --help` for CLI help
- Run `aida <command> --help` for command-specific help
- Open this guide with `aida user-guide`

---

*Generated for AIDA v0.1.0*
