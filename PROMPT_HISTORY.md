# Requirements Manager - Prompt History

A chronological record of development sessions and changes made to the Requirements Manager project.

---

## Session 1: Initial Setup and Core Features

### Initial Commit
- Created basic requirements management CLI in Rust
- Implemented core data models (Requirement, RequirementStatus, RequirementPriority, RequirementType)
- Added YAML-based storage layer
- Implemented basic CRUD operations

### Integration Planning
- **Prompt**: Plan integration with ai-provenance system
- **Actions**:
  - Added INTEGRATION.md with detailed integration plan
  - Created SIMPLIFIED_INTEGRATION.md with streamlined approach
  - Added FINAL_RECOMMENDATION.md with implementation recommendations
  - Created INTEGRATION_INDEX.md for documentation navigation

### Export Feature
- **Prompt**: Add export functionality for requirement mappings
- **Actions**:
  - Implemented `export` command in CLI
  - Support for mapping format (UUID/SPEC-ID)
  - Support for JSON export format
  - Output to file or stdout

---

## Session 2: SPEC-ID System and Relationships

### SPEC-ID Implementation
- **Prompt**: Add human-friendly SPEC-ID as alternate key
- **Actions**:
  - Designed SPEC-ID format (SPEC-XXX)
  - Added UUID_SPEC_ID_VERIFICATION.md with mapping verification
  - Added SPEC_ID_AS_ALTERNATE_KEY.md with design document
  - Implemented SPEC-ID in Requirement model
  - Added SPEC_ID_IMPLEMENTATION_COMPLETE.md summary

### Delete Command
- **Prompt**: Add ability to delete requirements
- **Actions**:
  - Implemented `del` command in CLI
  - Support for both UUID and SPEC-ID lookups
  - Added confirmation prompt with --yes/-y skip option

### Relationship System
- **Prompt**: Add relationships between requirements
- **Actions**:
  - Implemented relationship types (Parent, Child, Verifies, VerifiedBy, References, Duplicate, Custom)
  - Added `rel add` command with bidirectional support
  - Added `rel remove` command
  - Added `rel list` command
  - Improved relationship display clarity

---

## Session 3: Workspace Restructure and GUI

### Workspace Restructure
- **Prompt**: Restructure project into workspace with CLI and GUI
- **Actions**:
  - Created Cargo workspace with three crates:
    - requirements-core: Shared library
    - requirements-cli: CLI tool (req binary)
    - requirements-gui: GUI application (req-gui binary)
  - Moved core logic to shared library
  - Cleaned up old requirements-manager directory
  - Updated .gitignore

### GUI Implementation
- **Prompt**: Implement full CRUD operations in GUI
- **Actions**:
  - Implemented egui-based GUI application
  - Added requirements list with search/filter
  - Added detail view for requirements
  - Implemented Add, Edit, Delete operations
  - Added Reload functionality

---

## Session 4: Comments and History

### Comment System
- **Prompt**: Implement threaded comment system
- **Actions**:
  - Added Comment model with threading support
  - Implemented comment CRUD operations
  - Added `comment add` CLI command with parent support
  - Added `comment list`, `comment edit`, `comment delete` commands
  - Integrated comments into GUI

### Collapsible Comments
- **Prompt**: Add collapsible comment trees to GUI
- **Actions**:
  - Implemented tree view for threaded comments
  - Added expand/collapse functionality
  - Added reply button for nested comments

### Change History
- **Prompt**: Add change history tracking to requirements
- **Actions**:
  - Added HistoryEntry and FieldChange models
  - Track all field changes with old/new values
  - Record timestamp and author for changes
  - Added history display in requirement details

### Tabbed Interface
- **Prompt**: Implement tabbed interface with history in GUI
- **Actions**:
  - Added tab system (Description, Comments, Links, History)
  - Implemented DetailTab enum for view state
  - Added History tab showing change log
  - Added Links tab for relationship display

---

## Session 5: Improvements and Documentation

### Many Improvements (Latest Session)
- **Prompt**: Various improvements and polish
- **Actions**:
  - Enhanced GUI with user settings (name, email, handle)
  - Added configurable font size with zoom controls
  - Added multiple view perspectives (Flat, Parent/Child, Verification, References)
  - Implemented ID configuration commands
  - Added requirement type management
  - Created user-guide.md documentation
  - Generated HTML documentation (light and dark modes)
  - Added `user-guide` CLI command to open documentation
  - Created helper scripts for documentation generation

### Documentation Cleanup
- **Prompt**: Review updates and create documentation
- **Actions**:
  - Created OVERVIEW.md with project vision and structure
  - Created REQUIREMENTS.md with system requirements
  - Created PROMPT_HISTORY.md (this file)

### Arrow Key Navigation
- **Prompt**: Add arrow key navigation for requirements list panel
- **Actions**:
  - Added `get_filtered_indices()` helper function to app.rs
  - Implemented Up/Down arrow key handling in update() function
  - Navigation respects current filters (search, type, feature filters)
  - Auto-selects first/last item when nothing selected
  - Updated user-guide.md with new keyboard shortcut
  - Regenerated HTML documentation
  - Updated CLAUDE.md to reflect current workspace structure

### GUI Enhancements (Continued)
- **Prompt**: Various UI improvements
- **Actions**:
  - Enter key to edit selected requirement
  - Double-click to edit requirement
  - Spacebar to expand/collapse tree nodes
  - Full-width title and description fields in forms
  - Full-width comment content field
  - Proper indentation for threaded comments with +/- icons
  - Fixed-width expand/collapse buttons (18x18)
  - Comment text wrapping within panel width

### Theme Selection
- **Prompt**: Add theme selection in preferences
- **Actions**:
  - Added Theme enum (Dark, Light, High Contrast Dark, Solarized Dark, Nord)
  - Implemented theme application via egui::Visuals
  - Added theme selector in Appearance settings tab
  - Themes persist to user settings file

### Preferred View Setting
- **Prompt**: Save preferred view in preferences
- **Actions**:
  - Added Perspective enum (Flat, ParentChild, Verification, References)
  - Added preferred_perspective to UserSettings
  - Load saved perspective on startup
  - Perspective selector in Appearance settings tab

### Tree View Navigation Fix
- **Prompt**: Arrow keys should follow tree view display order
- **Actions**:
  - Implemented collect_tree_indices_top_down() for Parent/Child and Verification views
  - Implemented collect_tree_indices_bottom_up() for References view
  - Navigation now follows actual display order in all perspectives

### Customizable Keybindings
- **Prompt**: Add keyboard mappings panel in settings
- **Actions**:
  - Added KeyAction enum for all bindable actions
  - Created KeyBinding struct with key name, modifiers (ctrl, shift, alt)
  - Added KeyBindings collection with defaults
  - Implemented Keybindings settings tab with capture mode
  - Key capture shows "Press a key..." with Escape to cancel
  - Reset to Defaults button restores default bindings
  - Replaced hardcoded key checks with keybinding lookups
  - Keybindings persist to user settings file

### Project Settings Tab
- **Prompt**: Add project settings for ID naming schemes in settings
- **Actions**:
  - Added Project tab to settings dialog
  - IdFormat selection (Single Level vs Two Level naming)
  - NumberingStrategy selection (Global, Per Prefix, Per Feature+Type)
  - Digit count configuration (1-6 digits)
  - Live example preview showing resulting ID format
  - Project settings stored in requirements.yaml file
  - Settings loaded on dialog open, saved with other settings

### ID Migration Support
- **Prompt**: Add validation and migration for ID settings changes
- **Actions**:
  - Added `IdConfigValidation` struct in requirements-core for validation results
  - Implemented `validate_id_config_change()` method to check proposed settings
  - Added `get_max_digits_in_use()` helper to find maximum digit count in existing IDs
  - Implemented `migrate_ids_to_config()` method to update all requirement IDs
  - Validation prevents digit reduction below existing maximum
  - Format changes require Global Sequential numbering
  - Added validation display in Project settings tab (errors in red, warnings in yellow)
  - Added "Migrate Existing IDs" button when settings differ from current
  - Implemented migration confirmation dialog with affected count and warnings
  - Updated user guide documentation with migration feature details

### Theme Cycling Shortcut
- **Prompt**: Ctrl-T should cycle through the themes
- **Actions**:
  - Added `CycleTheme` action to `KeyAction` enum
  - Added `next()` method to `Theme` enum for cycling through themes
  - Added default keybinding Ctrl+T in `KeyBindings::default()`
  - Added keybinding handler in update function to cycle and save theme
  - Theme order: Dark → Light → High Contrast Dark → Solarized Dark → Nord → Dark
  - Updated user guide documentation with new shortcut

### Markdown Support for Descriptions
- **Prompt**: Add markdown editor/preview for requirement descriptions
- **Actions**:
  - Added `egui_commonmark` crate (v0.18) for markdown rendering
  - Added `CommonMarkCache` to RequirementsApp state for caching rendered markdown
  - Updated detail view to render descriptions as markdown
  - Added preview toggle in edit form (Edit/Preview button)
  - Shows "Supports Markdown" hint in description field header
  - Reset preview mode when clearing form or loading requirement for edit
  - Updated user guide with Markdown support documentation

### Custom ID Prefix Override
- **Prompt**: Allow per-requirement prefix override for flexible ID organization
- **Actions**:
  - Added `prefix_override: Option<String>` field to Requirement model
  - Added `validate_prefix()` and `set_prefix_override()` methods for validation
  - Validation ensures prefix contains only uppercase letters (A-Z)
  - Updated `add_requirement_with_id()` to use prefix_override when set
  - Updated `generate_requirement_id_with_override()` for custom prefix ID generation
  - Updated migration functions to respect prefix_override
  - Added "ID Prefix" field to GUI form with validation indicator
  - Added `--prefix` option to CLI `add` command
  - Per Prefix numbering treats custom prefixes as their own counter
  - Global Sequential numbering uses shared counter regardless of prefix
  - Updated user guide documentation with custom prefix usage

### Prefix Update Bug Fix
- **Prompt**: Updating prefix doesn't update the spec_id, need conflict checking
- **Actions**:
  - Added `regenerate_spec_id_for_prefix_change()` method to RequirementsStore
  - Added `is_spec_id_available()` helper function
  - Rewrote `update_requirement()` in GUI to handle prefix changes properly
  - Checks for ID conflicts before allowing changes
  - Shows error message if new ID would conflict with existing requirement

### Collapsible Left Panel in Edit Mode
- **Prompt**: Keep left panel open in edit mode when window is wide enough with expand/collapse option
- **Actions**:
  - Added `left_panel_collapsed: bool` field to RequirementsApp state
  - Modified update() to conditionally show left panel based on screen width (900px minimum)
  - Added "▶ Hide" button in left panel header when in form view
  - Added "◀ Show List" button in central panel when panel is hidden
  - Updated show_list_panel() function signature to accept `in_form_view: bool`
  - Updated user guide with Responsive Layout section

### Relationship Definition System
- **Prompt**: Add ability to manage and add relationships with constraints on types and cardinality
- **Actions**:
  - Created design document (docs/RELATIONSHIP_DESIGN.md) with full specification
  - Added `Cardinality` enum (OneToOne, OneToMany, ManyToOne, ManyToMany)
  - Added `RelationshipDefinition` struct with full metadata:
    - name, display_name, description
    - inverse relationship name
    - symmetric flag
    - cardinality constraints
    - source_types and target_types constraints
    - built_in flag (cannot delete)
    - color and icon for visualization
  - Added `RelationshipValidation` struct for validation results
  - Added `relationship_definitions` field to `RequirementsStore`
  - Implemented default built-in relationships (parent, child, verifies, verified_by, etc.)
  - Added new relationships: depends_on/dependency_of, implements/implemented_by
  - Added validation methods:
    - `validate_relationship()` - checks type constraints, cardinality, cycles
    - `would_create_cycle()` - detects hierarchical cycles
    - `get_inverse_type()` - looks up inverse from definitions
  - Added management methods for definitions (add, update, remove, ensure_builtin)
  - Added `RelDefCommand` to CLI with list/show/add/edit/remove subcommands
  - Exported new types from requirements-core lib.rs
  - Updated user guide with relationship definitions documentation

### GUI Integration for Relationship Definitions (Phase 4)
- **Prompt**: Proceed to Phase 4: GUI Integration - Add Relationships tab to Settings, update Links tab to respect constraints
- **Actions**:
  - Added `Relationships` tab to Settings dialog with full CRUD for definitions
  - Created relationship definition list view showing:
    - Display name with [built-in] badge
    - Name, inverse/symmetric indicator, cardinality
    - Type constraints (source/target types)
    - Color swatch preview
    - Edit/Delete buttons (delete only for non-built-in)
  - Added relationship definition edit form with:
    - Name field (readonly for built-in/editing)
    - Display name, description (always editable)
    - Inverse/symmetric/cardinality (not editable for built-in)
    - Source/target type constraints
    - Color picker with hex preview
  - Updated Links tab to use relationship definitions:
    - Shows display name instead of enum debug format
    - Displays color indicator swatch from definition
    - Uses definition-based inverse detection for bidirectional removal
  - Added validation feedback when creating relationships:
    - Validates type constraints before creation
    - Checks cardinality constraints
    - Shows errors for invalid relationships
    - Shows warnings for constraint violations
  - Added `parse_hex_color()` helper for color rendering

### View Presets Feature
- **Prompt**: Save view configuration (filters, perspective, direction) as named presets
- **Actions**:
  - Added `ViewPreset` struct to store view configuration:
    - name, perspective, direction
    - filter_types and filter_features as serializable vectors
  - Added `view_presets: Vec<ViewPreset>` to `UserSettings`
  - Added preset state tracking to `RequirementsApp`:
    - `active_preset: Option<String>` for currently active preset
    - `show_save_preset_dialog` and `preset_name_input` for save dialog
    - `show_delete_preset_confirm` for deletion confirmation
  - Added helper methods:
    - `current_view_matches_active_preset()` - checks if view matches saved preset
    - `has_unsaved_view()` - detects when view has unsaved changes
    - `apply_preset()` - applies a preset to current view
    - `save_current_view_as_preset()` - saves current view as new/updated preset
    - `delete_preset()` - removes a preset
    - `reset_to_default_view()` - returns to Flat/TopDown with no filters
  - Updated View dropdown in `show_list_panel()`:
    - Shows "Built-in Views" section with Flat, Parent/Child, etc.
    - Shows "Saved Presets" section with user presets
    - Presets have delete (✕) button inline
    - Selected text shows preset name with * if modified
  - Added "💾 Save As..." button (appears when view has unsaved changes)
  - Added "↺" reset button (appears when not at default view)
  - Implemented `show_save_preset_dialog_window()`:
    - Text input for preset name
    - Warning if overwriting existing preset
    - Shows current view settings summary
  - Implemented `show_delete_preset_confirmation_dialog()`:
    - Confirms preset deletion
  - Added `PerspectiveDirection` serialize/deserialize support
  - Updated user guide with View Presets documentation

### Keybinding Context/Scope System
- **Prompt**: Add when/where context for keybindings (e.g., Edit/Add, Requirements Panel)
- **Actions**:
  - Added `KeyContext` enum with four scopes:
    - `Global` - Works anywhere in the application
    - `RequirementsList` - Only in the requirements list panel
    - `DetailView` - Only when viewing requirement details
    - `Form` - Only when in add/edit form
  - Added `context: KeyContext` field to `KeyBinding` struct with serde default
  - Added `default_context()` method to `KeyAction` for appropriate defaults:
    - NavigateUp/Down, Edit, ToggleExpand → RequirementsList
    - ZoomIn/Out/Reset, CycleTheme → Global
  - Updated `KeyBinding::matches()` to check context compatibility
  - Added `current_key_context: KeyContext` field to `RequirementsApp`
  - Context is determined in update() based on:
    - `ctx.wants_keyboard_input()` → Global (text field focused)
    - `View::List` → RequirementsList
    - `View::Detail` → DetailView
    - `View::Add/Edit` → Form
  - Updated all `is_pressed()` calls to pass current context
  - Updated Keybindings settings UI:
    - Added Context column to keybindings table
    - Added context dropdown for each action
    - Preserved context when capturing new key
  - Updated user guide with context documentation

### Save Keybinding (Ctrl+S)
- **Prompt**: Ctrl-S for save in Edit/Add
- **Actions**:
  - Added `Save` action to `KeyAction` enum
  - Added label "Save" and default context `KeyContext::Form`
  - Added default keybinding: `Ctrl+S` with Form context
  - Added `pending_save: bool` field to `RequirementsApp`
  - Implemented keybinding handler in update() to set `pending_save = true`
  - Updated `show_form()` to check `pending_save` flag and trigger save
  - Keybinding only activates when in Add or Edit form (Form context)

### Comment Reactions
- **Prompt**: Have several (configurable) emoji that we can attach to a comment. A checkmark for resolved would be a minimal need and one for rejected or maybe thumbs up or down.
- **Actions**:
  - Added `ReactionDefinition` struct with name, emoji, label, description, built_in flag
  - Added `CommentReaction` struct with reaction name, author, timestamp
  - Added `default_reaction_definitions()` function with 6 built-in reactions:
    - ✅ Resolved, ❌ Rejected, 👍 Thumbs Up, 👎 Thumbs Down, ❓ Question, ⚠️ Important
  - Added `reactions: Vec<CommentReaction>` field to `Comment` struct
  - Added reaction methods to Comment: `add_reaction()`, `remove_reaction()`, `toggle_reaction()`, `reaction_counts()`, `has_reaction()`
  - Added `reaction_definitions: Vec<ReactionDefinition>` to `RequirementsStore`
  - Exported new types from requirements-core lib.rs
  - Updated GUI `show_comment_tree()` to display:
    - Existing reactions with count and ✓ indicator if user has reacted
    - Reaction picker button (😊) that opens inline picker
    - Clickable reactions to toggle user's reaction
  - Added `pending_reaction_toggle` and `show_reaction_picker` state fields
  - Implemented `toggle_comment_reaction()` method with recursive comment search
  - Added `Reactions` tab to Settings dialog with:
    - List of all reaction definitions with emoji, name, label, description
    - Add/Edit form for custom reactions
    - Delete button for non-built-in reactions
    - Reset to Defaults button
  - Updated user guide with Comment Reactions documentation

### User Meta-Type with $USER-XXX IDs
- **Prompt**: I want a User object type to manage users. A user will have relationships with requirements. A requirement could be created-by, assigned-to, tested-by, closed-by. Since this is a special type, I propose having a prefix '$USER' and its own sequence number starting at one. We will have other special types that start with '$'. For example, Views, Features, and other metatypes can have their own id.
- **Actions**:
  - Added `spec_id: Option<String>` field to User struct for `$USER-XXX` format IDs
  - Added `new_with_spec_id()` constructor and `display_id()` helper method
  - Added meta-type prefix constants: `META_PREFIX_USER`, `META_PREFIX_VIEW`, `META_PREFIX_FEATURE`
  - Added `meta_counters: HashMap<String, u32>` to RequirementsStore for per-prefix counters
  - Added methods to RequirementsStore:
    - `next_meta_id()` - generates next ID for a meta-type prefix
    - `add_user_with_id()` - adds user with auto-generated $USER-XXX ID
    - `find_user_by_spec_id()` / `find_user_by_spec_id_mut()` - lookup by spec_id
    - `migrate_users_to_spec_ids()` - assigns IDs to existing users
  - Added user relationship types to default RelationshipDefinitions:
    - `created_by` - User who created the requirement (N:1, blue)
    - `assigned_to` - User assigned to work on requirement (N:1, green)
    - `tested_by` - User who tested/verified requirement (N:N, orange)
    - `closed_by` - User who closed/completed requirement (N:1, red)
  - Updated GUI users table to show spec_id column with blue highlighting
  - Updated `add_new_user()` to use `add_user_with_id()` for auto-generated IDs
  - Added automatic migration in storage.rs to assign $USER-XXX IDs on load
  - Exported meta prefix constants from requirements-core lib.rs

---

## Git Operations Summary

### Key Commits
| Hash | Description |
|------|-------------|
| 93429bd | Initial commit |
| 8c240c3 | Export command |
| 31353f1 | SPEC-ID implementation |
| b5c4ae5 | Delete command |
| ca97e05 | Relationship system |
| 411edb4 | Workspace restructure |
| 4b91e82 | GUI CRUD operations |
| a16d853 | Threaded comments |
| 41096d3 | Change history |
| 3ec7ace | Tabbed interface |
| 4e96abf | Many improvements |

### Branches
- **main**: Primary development branch

---

## Technical Decisions

### Storage Format
- Chose YAML for human-readability and Git-friendliness
- All data in single requirements.yaml file per project

### ID System
- Dual ID system: UUID for internal use, SPEC-ID for human reference
- Configurable ID formats and numbering strategies

### GUI Framework
- Selected egui for cross-platform Rust GUI
- Immediate mode rendering for simplicity

### Architecture
- Workspace structure to share code between CLI and GUI
- Core library contains all business logic
- CLI and GUI are thin wrappers around core

---

## Session 7: Custom Type Definitions (2025-11-26)

### Custom Type Definitions System
- **Prompt**: Add support for different requirement types with type-specific statuses and custom fields
- **Problem**: Change Requests need different statuses (Submitted, Under Review, In Progress, etc.) than standard requirements. May also need additional fields specific to the type.
- **Solution**: Implemented a hybrid approach with configurable type definitions stored in requirements.yaml
- **Actions**:
  - Added `CustomFieldType` enum (Text, TextArea, Select, Boolean, Date, User, Requirement, Number)
  - Added `CustomFieldDefinition` struct with name, label, type, required, options, default value
  - Added `CustomTypeDefinition` struct with name, display_name, prefix, statuses, custom_fields
  - Added `default_type_definitions()` function with built-in types
  - Added `type_definitions: Vec<CustomTypeDefinition>` to RequirementsStore
  - Added `custom_status: Option<String>` to Requirement for non-enum statuses
  - Added `custom_fields: HashMap<String, String>` to Requirement for type-specific fields
  - Added helper methods: `effective_status()`, `set_status_from_str()`, `get_type_definition()`, `get_statuses_for_type()`, `get_custom_fields_for_type()`
  - ChangeRequest type now has custom statuses: Draft, Submitted, Under Review, Approved, Rejected, In Progress, Implemented, Verified, Closed
  - ChangeRequest type has custom fields: impact (select), requested_by (user ref), target_release (text), justification (textarea)

### GUI Updates for Custom Types
- **Actions**:
  - Updated form to use type-specific status dropdown
  - Added `form_status_string` and `form_custom_fields` to track form state
  - When type changes, status dropdown updates to show type-specific statuses
  - Custom fields section appears when type has custom fields defined
  - Supports all field types with appropriate UI controls
  - User reference fields show dropdown of active users
  - Requirement reference fields show dropdown of requirements
  - Select fields show dropdown of predefined options

### Type Definitions Settings Tab
- **Actions**:
  - Added `TypeDefinitions` variant to SettingsTab enum
  - Added "📝 Types" tab to Settings dialog
  - Added `show_settings_type_definitions_tab()` function
  - Displays all type definitions in collapsible sections
  - Shows type info: name, prefix, description, built-in status
  - Shows available statuses for each type
  - Shows custom field definitions with type, required, options
  - Added "Reset to Defaults" button

### Documentation Updates
- Updated user-guide.md with:
  - Updated Type field to include ChangeRequest
  - Added Custom Fields to Requirement Fields table
  - Updated Status Workflow section to mention type-specific statuses
  - Added Type Definitions section with built-in types table
  - Documented Change Request workflow
  - Documented custom field types
  - Added instructions for managing type definitions in GUI

### ID Prefix Filtering and Management (continued)
- **Prompt**: Add prefix filtering to the GUI and prefix management in admin settings
- **Problem**: Users wanted to filter requirements by their ID prefix (e.g., show only SEC-xxx or API-xxx requirements). Also wanted admin control over which prefixes are allowed.
- **Solution**: Added prefix registry to RequirementsStore with filter support and admin management
- **Actions**:
  - Added `allowed_prefixes: Vec<String>` to RequirementsStore - list of allowed/known prefixes
  - Added `restrict_prefixes: bool` - when true, users must select from allowed list
  - Added `get_used_prefixes()` - gets all unique prefixes currently in use
  - Added `get_all_prefixes()` - combines allowed + used prefixes
  - Added `add_allowed_prefix()`, `remove_allowed_prefix()`, `is_prefix_allowed()` helper methods
  - Added `filter_prefixes: HashSet<String>` to GUI app for prefix filtering
  - Updated ViewPreset to include prefix filters
  - Updated `passes_filters()` to check prefix filter
  - Added "ID Prefix Filters" section to filter panel (shows checkboxes for each unique prefix)
  - Added "ID Prefix Management" section to Admin settings tab:
    - Toggle to restrict prefixes to allowed list
    - Shows prefixes currently in use
    - Add new allowed prefixes
    - Remove prefixes from allowed list
  - Updated add/edit form to show dropdown when prefixes are restricted, text input otherwise
  - Auto-add new prefixes to allowed list when used (unless restricted)
  - Updated documentation with Prefix Management section

### Two-Level Filter System (Root/Children)
- **Prompt**: "I think in filtering it may be worth having two levels. So when you click Filters you get two tabs, one for root and one for children. The root option filters the first level of requirements that are selected, and the children applies to which children are shown recursively. In children there is a checkbox, <same as root> which greys out everything or hides them (that would be the same as what we currently have, one set of filters across all requirements). The purpose of this is to limit the scope of the top level requirements that we want to drill down into."
- **Problem**: Users wanted to filter root-level requirements differently from children in hierarchical views. For example, show only Functional Requirements at root level, but display all their children regardless of type.
- **Solution**: Implemented two-level filtering with separate filter sets for root and children, with "Same as root" option to use unified filters.
- **Actions**:
  - Added `FilterTab` enum (Root, Children) for tab selection
  - Added child filter fields to RequirementsApp:
    - `child_filter_types: HashSet<RequirementType>`
    - `child_filter_features: HashSet<String>`
    - `child_filter_prefixes: HashSet<String>`
    - `children_same_as_root: bool` (defaults to true)
    - `filter_tab: FilterTab` for active tab state
  - Updated ViewPreset struct with child filter fields and `children_same_as_root`
  - Updated `apply_preset()` to restore child filter state
  - Updated `save_current_view_as_preset()` to save child filters
  - Updated `current_view_matches_active_preset()` to compare child filters
  - Updated `has_unsaved_view()` to detect child filter changes
  - Refactored `show_filter_controls()` to display Root/Children tabs
  - Added `show_root_filter_controls()` for root-level filters
  - Added `show_children_filter_controls()` with "Same as root" checkbox that disables child filters when checked
  - Updated `passes_filters()` to accept `is_root: bool` parameter:
    - Root requirements use root filters
    - Child requirements use child filters (or root filters if `children_same_as_root` is true)
  - Updated all callers of `passes_filters()`:
    - `find_tree_roots()` and `find_tree_leaves()` - is_root=true
    - `get_children()` and `get_parents()` - is_root=false
    - Flat list views - is_root=true (all at same level)
  - Updated user-guide.md with "Filtering Requirements" section
  - Updated OVERVIEW.md with two-level filtering in GUI features

### Type Definition Editor
- **Prompt**: "For the Types in the Settings, does it make sense to have an editor so that we can add/remove/update fields?"
- **User Preferences**:
  - Allow modifying built-in types (with reset to defaults option)
  - Validate on save - warn if removing statuses/fields that are in use
- **Solution**: Implemented full type definition editor in Settings > Types tab
- **Actions**:
  - Added type definition editing state to RequirementsApp:
    - `editing_type_def: Option<String>` - name of type being edited
    - `type_def_form_*` fields for name, display_name, description, prefix
    - `type_def_form_statuses: Vec<String>` - editable status list
    - `type_def_form_fields: Vec<CustomFieldDefinition>` - editable fields list
    - `show_type_def_form: bool` - toggle form visibility
    - `new_status_input: String` - input for adding new statuses
  - Added custom field editing state:
    - `editing_field_idx: Option<usize>` - index of field being edited
    - `field_form_*` fields for name, label, type, required, options, default
    - `show_field_form: bool` - toggle field form visibility
  - Refactored `show_settings_type_definitions_tab()`:
    - Added "➕ Add New Type" button
    - Each type shows edit (✏), reset (↺ for built-in), and delete (🗑 for custom) buttons
    - Uses CollapsingState for expandable type details
  - Added `show_type_definition_form()`:
    - Form for editing type properties (name, display_name, description, prefix)
    - Status management with add/remove buttons
    - Validates removing statuses - prevents if status is in use by requirements
    - Custom fields table with edit/remove buttons per field
    - Validates removing fields - prevents if field is in use by requirements
  - Added `show_custom_field_form()`:
    - Form for adding/editing custom fields
    - Field type dropdown (Text, TextArea, Select, Boolean, Date, Number, User, Requirement)
    - Options input for Select type (comma-separated)
    - Required checkbox and default value input
  - Added `save_type_definition()`:
    - Creates CustomTypeDefinition from form data
    - Preserves built_in flag when editing existing types
    - Updates or adds type to store
  - Added `field_type_display()` helper for field type labels
  - Added individual type reset (restore single built-in type to defaults)
  - Added type deletion with validation (cannot delete if in use)
  - Updated user-guide.md with detailed type management documentation

### Navigation Keybindings Bug Fix
- **Prompt**: "When I am add/edit and I start editing the description and I press the up arrow, I think the global key binding is being invoked to move to the previous requirement"
- **Root Cause**: User's saved settings (`~/.requirements_gui_settings.yaml`) had NavigateUp/NavigateDown keybindings with `Global` context instead of `RequirementsList`
- **Solution**: Fixed context determination in keybinding evaluation; user should delete saved settings to reset keybindings
- **Actions**:
  - Changed keybinding context to `KeyContext::Form` when in form view or settings
  - Added debug prints to diagnose the issue
  - Confirmed the bug was due to persisted settings with wrong context values

### External URL Links Feature
- **Prompt**: "When you click on Links tab, beside Relationships show a New URL button, in the modal perhaps a button to verify that it is a valid link."
- **Solution**: Added external URL links to requirements with validation support
- **Actions**:
  - Added `UrlLink` struct to requirements-core/src/models.rs:
    - Fields: id, url, title, description, added_at, added_by, last_verified, last_verified_ok
    - Builder pattern with `new()` constructor
  - Added `urls: Vec<UrlLink>` field to Requirement struct
  - Exported `UrlLink` from requirements-core/src/lib.rs
  - Added dependencies to requirements-gui/Cargo.toml:
    - `url = "2"` for URL parsing/validation
    - `open = "5"` for opening URLs in browser
  - Added URL form state fields to RequirementsApp:
    - `show_url_form`, `editing_url_id`, `url_form_url`, `url_form_title`, `url_form_description`
    - `url_verification_status`, `url_verification_in_progress`
  - Updated `show_links_tab()`:
    - Added "External Links" section with "➕ New URL" button
    - Display list of URL links with verification status indicators (✅/❌)
    - Clickable links that open in browser via `open::that()`
    - Edit (✏) and remove (x) buttons per link
  - Added `show_url_form_modal()`:
    - Form fields for URL, title (optional), description (optional)
    - "🔍 Verify" button to validate URL format
    - Shows verification status with colored messages
  - Added `verify_url()` function:
    - Validates URL starts with http:// or https://
    - Uses `url::Url::parse()` for format validation
    - Checks URL has valid host
  - Added `save_url_link()` function:
    - Creates new or updates existing URL links
    - Sets verification timestamp if URL was verified
  - Updated Links tab count to show total of relationships + URLs
  - Updated user-guide.md with Links Tab documentation

### Detail View Title Bar Styling
- **Prompt**: "Requirement title (bar) should have configurable background/font color/font size to make it stand out more"
- **Solution**: Added configurable title bar styling with themed backgrounds
- **Actions**:
  - Added to CustomTheme struct:
    - `title_bar_bg: ThemeColor` - background color
    - `title_bar_text: Option<ThemeColor>` - optional text color override
    - `title_bar_font_size: f32` - font size multiplier (default 1.0)
  - Added defaults for dark theme: RGB(45, 45, 50) - slightly lighter than panel
  - Added defaults for light theme: RGB(220, 220, 225) - slightly darker than panel
  - Added helper methods to Theme enum:
    - `title_bar_bg()` - returns appropriate background for theme
    - `title_bar_text()` - returns optional text color
    - `title_bar_font_size()` - returns font size multiplier
  - Refactored title bar in `show_detail_view_internal()`:
    - Wrapped in `egui::Frame::none()` with styled background
    - Applied `.fill()`, `.inner_margin()`, and `.rounding()`
    - Changed `ui.heading()` to `ui.label()` with `egui::RichText`
    - Title text uses configurable size (18.0 * multiplier) and bold
    - Optional custom text color support
  - Built-in theme colors:
    - Dark: RGB(45, 45, 50)
    - Light: RGB(220, 220, 225)
    - HighContrastDark: RGB(35, 35, 40)
    - SolarizedDark: RGB(7, 54, 66) (base02)
    - Nord: RGB(59, 66, 82) (nord1)

### Stacked Layout Content Clipping Fix
- **Prompt**: "In the horizontal stacked layout the Details View layout is being clipped after the first line"
- **Root Cause**: The `SidePanel` and `CentralPanel` for the stacked detail view were wrapped in `ui.horizontal()`, which constrains height to a single row
- **Solution**: Removed the `ui.horizontal()` wrapper; panels position themselves side-by-side naturally
- **Actions**:
  - Removed `ui.horizontal(|ui| { ... });` wrapper around the panels
  - Fixed indentation of the SidePanel and CentralPanel code
  - Panels now use full available height in the stacked detail view

### Developer's Guide Documentation
- **Prompt**: "We have a very nice set of layouts... I need a comprehensive write up on our architecture and implementation"
- **Solution**: Created comprehensive Developer's Guide at `docs/DEVELOPER_GUIDE.md`
- **Contents**:
  - Project overview and technology stack
  - Project structure (workspace, crates, modules)
  - Core data model (Requirement, RequirementsStore, relationships)
  - GUI architecture (RequirementsApp, Views, update loop)
  - Layout system (5 layout modes with implementation patterns)
  - Theme system (built-in themes, CustomTheme structure)
  - State management patterns (pending operations, form state)
  - Keyboard system (contexts, actions, bindings)
  - Filtering and perspectives
  - Configuration and persistence
  - Common development tasks with code examples:
    - Adding a new requirement field
    - Adding a new layout mode
    - Adding a new dialog
  - Code patterns and conventions
  - Troubleshooting guide
  - Appendices with file locations and line number references

### Edit/Add Form Redesign
- **Prompt**: "I really like the Details View layout and wonder if we could reuse/mimic it for the Edit/Add view"
- **Solution**: Created new `show_form_stacked()` function that mirrors the Detail View layout
- **Layout Design**:
  - **Title Bar**: Styled header with editable title field, Save/Cancel buttons, and mode indicator (New/Edit)
  - **Left Panel** (25% default width, resizable): Metadata fields in a grid
    - ID (edit mode only)
    - Prefix dropdown/textbox (respects restrict_prefixes setting)
    - Type dropdown (Functional, NonFunctional, System, User, ChangeRequest, Bug, Epic, Story, Task, Spike)
    - Status dropdown (dynamically based on type's allowed statuses)
    - Priority dropdown (High, Medium, Low)
    - Feature text field
    - Owner text field
    - Tags text field (comma-separated)
    - Parent (new requirements only, if set)
    - Custom fields section (type-specific fields)
  - **Right Panel** (75%, remaining space): Description editor
    - Header with Preview/Edit toggle and Markdown help link
    - Full-height text editor or markdown preview
- **Features**:
  - Keyboard shortcuts: Ctrl+S to save, ESC to cancel
  - Cancel confirmation dialog for unsaved changes
  - Context menu support for text fields
  - Type change resets status to first valid status
  - Custom fields support all field types (Text, TextArea, Select, Boolean, Number, Date, User, Requirement)
- **Helper Functions**:
  - `show_prefix_field()` - Reusable prefix dropdown/text input
  - `show_custom_field_editor()` - Reusable custom field renderer for all field types

### Layout-Aware Form Views
- **Prompt**: "The layout for the edit should match the layout we are currently viewing"
- **Solution**: Form layout now adapts based on current view mode
- **Implementation**:
  - `show_form_vertical()` - For List|Details (side-by-side) view:
    - Metadata grid at top (matching Detail View vertical layout)
    - Description editor at bottom with scroll
  - `show_form_stacked()` - For List/Details Stacked view:
    - Metadata on left (25% resizable panel)
    - Description on right (75% remaining space)
  - Form selection logic at call site:
    - `LayoutMode::ListDetailsStacked` → `show_form_stacked()`
    - All other modes → `show_form_vertical()`
- **Both layouts share**:
  - Styled title bar with editable title field
  - Save/Cancel buttons with keyboard shortcuts
  - All metadata fields as dropdowns
  - Description editor with markdown preview toggle
  - Custom fields support
  - Cancel confirmation dialog

### Seamless Detail-to-Edit View Transition
- **Prompt**: "When we switch to Edit we should not adjust the relative width of the panels, the Details View and the Edit View should remain the same width - it is a little jarring to jump in size. Also the font size for the title should remain the same. No need to have the word 'Edit' to the right of the textbox for the title during edit, and make the title textbox is not expanding to use all available width, we should do that."
- **Solution**: Made transitions between Detail View and Edit View seamless
- **Changes to both `show_form_vertical()` and `show_form_stacked()`**:
  1. **Title font size**: Changed from `egui::TextStyle::Heading` to `egui::FontId::proportional(18.0 * title_bar_font_size)` to match Detail View exactly
  2. **Title width**: Changed from `available_width * 0.6` to `(available - button_space).max(200.0)` where `button_space = 180.0`, making title expand to use all available width
  3. **Removed mode indicator**: Removed the "Edit"/"New" label that appeared next to the title, reducing visual clutter
- **Result**: Switching between Detail View and Edit View now feels seamless with consistent sizing and appearance

### Simplified List Panel in Edit View
- **Prompt**: User feedback via screenshots showing list panel width jump when entering Edit view - expanded filter bar with View/Perspective/Direction controls was making the list panel much wider in Edit mode compared to Detail mode
- **Root Cause**: Edit view was using `show_list_panel()` which includes expanded filter bar (Hide, Filters, View, Parent/Child, Top-down, Save As, refresh). Detail view only shows simple search + filter button.
- **Solution**: Created `show_list_panel_simple()` function
- **Implementation**:
  - New function `show_list_panel_simple()` (lines 9268-9327) with simplified content:
    - Header with Hide button
    - Search bar with Search... hint (120.0 width to match Detail View)
    - Filter dropdown button only
    - Scrollable list
  - No perspective/preset/direction controls (these made panel wider)
  - Changed form view to use `show_list_panel_simple()` instead of `show_list_panel()`
- **Result**: List panel now maintains consistent width when switching between Detail and Edit views

### List Panel Max Width Constraint
- **Prompt**: User feedback showing list panel in Edit view auto-expanding to fit long requirement titles (e.g., "REQ-0090 - Arrow keys bad behavior in edit mode123456789...")
- **Root Cause**: `SidePanel::left()` with `.resizable(true)` auto-expands to fit content width. The long title in the list was causing the panel to grow beyond desired bounds.
- **Solution**: Added `max_width` constraint to `show_list_panel_simple()`
- **Implementation**:
  - Calculate `max_panel_width` as 50% of screen width (minimum 350.0)
  - Added `.max_width(max_panel_width)` to the SidePanel configuration
  - Content is now clipped/truncated rather than expanding the panel
- **Result**: List panel stays within bounds even with long requirement titles

### Details View Title Truncation
- **Prompt**: "In the Details view the title needs to be truncated so that the Actions and Edit buttons remain visible"
- **Root Cause**: The title label in `show_detail_view_internal()` was rendered first without width constraints, causing it to push the Actions/Edit/Close buttons off-screen when titles were very long.
- **Solution**: Constrain title width to reserve space for buttons
- **Implementation** (in `show_detail_view_internal()` around line 9635):
  - Calculate reserved `buttons_width` (220px with Close button, 180px without)
  - Calculate `title_max_width = (available_width - buttons_width).max(100.0)`
  - Use `allocate_ui_with_layout()` to create constrained space for title
  - Apply `ui.set_clip_rect()` to prevent overflow
  - Use `egui::Label::new(title_text).truncate()` to truncate with ellipsis
- **Result**: Long titles are now truncated with ellipsis, keeping Actions, Edit, and Close buttons visible

### Edit View Layout Gap Fix (ListDetailsSide Mode)
- **Prompt**: User screenshot showing black gap between list panel and Edit form panel
- **Root Cause**: Architectural mismatch between Detail View and Edit View layouts:
  - Detail View used `CentralPanel` with `ui.columns(2, ...)` for 50/50 split
  - Edit View used `SidePanel::left("list_panel_simple")` + `CentralPanel` - different approach!
  - The different panel IDs and layout mechanisms caused a visual gap
- **Solution**: Make Edit View use identical layout approach as Detail View for `ListDetailsSide` mode
- **Implementation** (in form view code around line 13983):
  - For `ListDetailsSide`: Use `CentralPanel` with `ui.columns(2, ...)` for Edit/Add views
  - Left column renders list content (search bar, filter button, tree list)
  - Right column renders the form via `show_form_vertical()`
  - Uses same scroll area ID (`"list_side_scroll"`) as Detail View for consistency
  - Other layout modes (ListDetailsStacked, SplitListDetails, etc.) continue using SidePanel
- **Result**: Seamless transition between Detail View and Edit View with no visual gaps

### Edit View Layout Fix (ListDetailsStacked Mode)
- **Prompt**: User screenshots showing stacked layout Edit view was using wrong panel arrangement - list on LEFT instead of on TOP
- **Root Cause**: Edit View for `ListDetailsStacked` was using `SidePanel` (list on left) + `CentralPanel` approach, but Detail View uses `TopBottomPanel` (list on top) + `CentralPanel`
- **Solution**: Make Edit View use `TopBottomPanel` for `ListDetailsStacked` mode
- **Implementation** (in form view code around line 14044):
  - For `ListDetailsStacked`: Use `TopBottomPanel::top("list_top_panel")` for list
  - Same panel ID, min_height, default_height, and resizable settings as Detail View
  - List panel has search bar, filter button, scrollable tree list
  - `CentralPanel` below contains the form via `show_form_stacked()`
- **Result**: Edit View in stacked mode now matches Detail View with list on top, form on bottom

### User-Defined Theme Files
- **Prompt**: "Should there be a default aida_gui_settings.yaml that we keep in git?" / "yes that sounds good to implement user-defined theme files and Keep the built-in themes compiled in as fallbacks"
- **Solution**: Added support for loading custom themes from `~/.config/aida/themes/` directory
- **Implementation** (in `app.rs`):
  - New helper function `themes_dir()` returns and creates `~/.config/aida/themes/` directory
  - `load_file_themes()` scans the themes directory for `.yaml`/`.yml` files and deserializes them
  - `save_theme_to_file()` exports a theme as a YAML file to the themes directory
  - Modified `UserSettings::load()` to merge file-based themes with embedded themes
  - Added "Export to File" button in Theme Editor to save current theme to a file
- **Result**: Users can now create, export, and share custom themes as YAML files. Built-in themes remain compiled in as fallbacks.

### Modal Window Size Constraints
- **Prompt**: "Modals should not be taller or wider than the window. We should use scrollbars (as needed) so that we never exceed a certain percentage of window height and width. The markdown Help for example can be very tall."
- **Solution**: Added helper functions and constraints to limit modal windows to 90% width and 85% height of the main window
- **Implementation** (in `app.rs`):
  - Added constants `MODAL_MAX_WIDTH_PERCENT` (0.90) and `MODAL_MAX_HEIGHT_PERCENT` (0.85)
  - New helper function `modal_max_size(ctx)` calculates max dimensions from screen rect
  - New helper function `constrained_modal_size(ctx, width, height)` clamps sizes to max
  - Updated modals to use `.max_width()`, `.max_height()`, and `.scroll()`:
    - Markdown Help modal
    - Settings dialog
    - Theme Editor
    - Switch Project dialog
    - New Project dialog
    - Status & Priority Icons dialog
    - View Settings (List 1 and List 2)
- **Result**: All modal windows now respect window boundaries and show scrollbars when content exceeds available space

### Markdown Help Split Panel with Preview
- **Prompt**: "For Markdown Help can we have a split panel with the right side showing a preview of the help shown on the left?"
- **Solution**: Redesigned Markdown Help modal with side-by-side syntax reference and live preview
- **Implementation** (in `app.rs`, `show_markdown_help_modal()`):
  - Changed from single column to horizontal split layout using `ui.horizontal()`
  - Left panel ("Syntax"): Grouped markdown syntax examples organized by category:
    - Headers (`#`, `##`, `###`)
    - Text formatting (bold, italic, strikethrough, inline code)
    - Lists (bullet lists, numbered lists, nested items)
    - Links (`[text](url)`)
    - Code blocks (fenced with language)
    - Quotes (blockquotes with `>`)
    - Tables (pipe-delimited)
    - Checkboxes (`- [ ]`, `- [x]`)
  - Right panel ("Preview"): Live rendered preview using `CommonMarkViewer`
    - Shows sample markdown demonstrating all syntax from left panel
    - Uses existing `markdown_cache` for efficient rendering
  - Both panels have independent scroll areas with unique `id_salt` values
  - Modal respects max size constraints (90% width, 85% height)
- **Result**: Users can now see syntax examples on the left and rendered output on the right simultaneously


### Database Abstraction Layer
- **Prompt**: "I want to support multiple database implementations. We want to build this so that we can support different databases with an abstraction layer. I would like you to go ahead and implement SQLite support also. We should be able to migrate from YAML into SQLite, we may want a common import/export format (maybe YAML or JSON)."
- **Solution**: Created a pluggable database backend system with YAML and SQLite support
- **Implementation** (in `aida-core/src/db/`):
  - `traits.rs`: Defined `DatabaseBackend` trait with full CRUD operations:
    - `BackendType` enum (Yaml, Sqlite)
    - `DatabaseConfig` struct for backend configuration
    - Core methods: `load()`, `save()`, `update_atomically()`
    - Requirement CRUD: `get_requirement()`, `add_requirement()`, `update_requirement()`, `delete_requirement()`
    - User CRUD: `get_user()`, `add_user()`, `update_user()`, `delete_user()`
    - Metadata operations: `get_name()`, `set_name()`, etc.
    - Utility: `exists()`, `create_if_not_exists()`, `stats()`
  - `yaml_backend.rs`: YAML implementation wrapping existing Storage class
  - `sqlite_backend.rs`: Full SQLite implementation:
    - WAL mode for concurrent access
    - Schema versioning for future migrations
    - Complex types (relationships, comments, history) stored as JSON
    - Efficient single-record CRUD operations overriding default load-all behavior
    - Thread-safe with Mutex-protected Connection
  - `schema.sql`: SQLite schema definition with:
    - `requirements` table with all fields and indexes
    - `users` table
    - `metadata` table for store configuration
    - `schema_version` table for versioning
  - `migration.rs`: Migration and export utilities:
    - `migrate_yaml_to_sqlite()`: Convert YAML to SQLite
    - `migrate_sqlite_to_yaml()`: Convert SQLite to YAML
    - `export_to_json()`: Export store to JSON file
    - `import_from_json()`: Import store from JSON file
  - `mod.rs`: Module root with factory functions:
    - `create_backend()`: Create backend by path (auto-detect from extension)
    - `open_or_create()`: Open existing or create new database
  - Added `rusqlite` dependency to workspace and aida-core Cargo.toml
  - Updated `lib.rs` to export the new `db` module
- **Result**: System now supports multiple storage backends with a unified interface. Migration between formats and JSON import/export for interoperability.


### Documentation: Administrator's and User's Guides
- **Prompt**: "Please update the users guide with pertinent information. I think we also need an administrators guide that goes over the project settings... but we want to cover the schema, and how to migrate, storage issues, multi user control etc."
- **Solution**: Created comprehensive administrator's guide and updated user's guide with storage backend information
- **Implementation**:
  - Created `docs/admin-guide.md` with:
    - Project configuration (ID settings, features, type definitions, relationship definitions, prefix management)
    - Storage backends comparison (YAML vs SQLite, when to use each)
    - Complete database schema documentation (YAML format, SQLite tables with all fields and indexes)
    - Migration procedures (YAML to SQLite, SQLite to YAML, JSON import/export)
    - Multi-user control (file locking for YAML, WAL mode for SQLite, user management)
    - Backup and recovery procedures
    - Performance tuning tips
    - Troubleshooting guide with diagnostic commands
    - Environment variables and registry file format appendices
  - Updated `docs/user-guide.md`:
    - Added "Storage Backends" section covering YAML and SQLite options
    - Updated table of contents
    - Added link to Administrator's Guide for detailed information
- **Result**: Comprehensive documentation for both end users and administrators managing requirements database deployments


### Auto-Title from Description (Add Form)
- **Prompt**: "For Add requirement, if not title is provided use first line of description as title. If we start typing in the description and the title is still empty type in both the title and description until a newline is entered (limit the title to 50 characters and the put elipsis."
- **Solution**: Implemented auto-sync of description first line to title in Add mode
- **Implementation** (in `aida-gui/src/app.rs`):
  - Added two new state fields to `RequirementsApp`:
    - `form_title_auto_synced: bool` - Tracks if auto-sync is active
    - `form_last_description: String` - Tracks previous description to detect changes
  - Updated `clear_form()` to reset auto-sync state when opening Add form
  - Added title change detection after title TextEdit:
    - If title is manually edited while in Add mode, auto-sync is disabled
    - Uses comparison with `form_last_description` to distinguish manual edits from synced changes
  - Added description-to-title sync logic after description TextEdit:
    - Only active in Add mode when auto-sync is enabled
    - Syncs first line of description (before newline) to title
    - Truncates to 50 characters with "..." ellipsis if longer (47 chars + "...")
    - Stops syncing once a newline is entered in description
    - Uses character count (not byte count) for proper Unicode handling
- **Result**: When adding a new requirement, typing in the description automatically populates the title until the user either edits the title manually or presses Enter in the description


### Copy for Claude Code Feature
- **Prompt**: "Add an 'Actions → AI → Implement in Claude Code' button that formats an approved requirement and copies it to clipboard"
- **Solution**: Added "Copy for Claude Code" button to AI submenu
- **Implementation** (in `aida-gui/src/app.rs`):
  - Added helper function `format_requirement_for_claude_code()`:
    - Creates formatted prompt with requirement ID, title, type, priority, feature
    - Includes description and tags
    - Adds implementation task instructions
  - Added button state tracking variable `copy_for_claude_code_idx`
  - Added button in AI submenu with separator
  - Button only enabled when requirement status is "Approved"
  - Shows tooltip "Requirement must be Approved to implement" when disabled
  - Fixed `on_disabled_hover_text()` ownership issue by shadowing `response` variable
  - Copies formatted text to clipboard and primary selection
  - Shows toast notification confirming copy
- **Result**: Users can now copy approved requirements in a format ready for Claude Code to implement


### Background AI Find Duplicates Requirement
- **Prompt**: "The AI 'Find Duplicates' takes time and should run in the background like 'Evaluate Requirement' and store the results in AI Evaluation with a button execute suggestion is there is one."
- **Solution**: Added FR-0148 requirement to track this enhancement
- **Implementation**:
  - Created requirement FR-0148 "Background AI Find Duplicates" via CLI
  - Updated REQUIREMENTS.md with new "AI Integration" section (section 12)
  - Documented current behavior: Find Duplicates runs synchronously blocking UI
  - Required behavior: Run in background thread like Evaluate action
  - Results should appear in AI Evaluation panel
  - Should have "Execute Suggestion" button for actionable duplicate findings
- **Result**: Feature request captured as FR-0148 for future implementation


### Implement Background AI Find Duplicates (FR-0148)
- **Prompt**: Implement FR-0148 - Background AI Find Duplicates
- **Solution**: Converted Find Duplicates AI action to run in a background thread like Evaluate Requirement
- **Implementation** (in `aida-gui/src/app.rs`):
  - Added `BackgroundFindDuplicatesResult` struct at ~line 2313 to hold async results:
    - `req_id: Uuid` - ID of requirement being checked
    - `spec_id: String` - SPEC-ID for display
    - `result: Result<Vec<DuplicateInfo>, String>` - Duplicates or error
  - Added `DuplicateInfo` struct at ~line 2320 to hold simplified duplicate data:
    - `spec_id`, `similarity`, `reason`, `recommendation`
  - Added state fields to `RequirementsApp`:
    - `find_duplicates_receiver: Option<mpsc::Receiver<BackgroundFindDuplicatesResult>>`
    - `find_duplicates_in_progress: Option<(Uuid, String)>`
  - Initialized new fields in `new()` function
  - Converted `AiAction::FindDuplicates` handler at ~line 11088:
    - Checks if already finding duplicates (shows toast if busy)
    - Creates `mpsc::channel()` for async communication
    - Spawns background thread that calls `ai_client.find_duplicates()`
    - Stores receiver and in-progress state
    - Shows "Finding duplicates for..." toast
    - Returns `None` for immediate result (polled later)
  - Added polling logic in `update()` at ~line 15584:
    - Uses `try_recv()` for non-blocking check
    - On success: formats message, shows toast with count, updates AI result panel
    - On error: shows error toast and updates AI result panel with error
    - Clears in-progress state and receiver
- **Pattern**: Follows exact same pattern as background AI Evaluate:
  1. Create channel
  2. Clone necessary data
  3. Store receiver in state
  4. Spawn thread to do async work
  5. Send result via channel
  6. Poll in `update()` loop
- **Result**: Find Duplicates now runs in background, UI remains responsive during AI API calls
- **Status**: FR-0148 marked as Completed



### AI Project Scaffolding (FR-0152)
- **Prompt**: Implement FR-0152 - AI scaffhold project with skills during new project creation
- **Solution**: Added project scaffolding feature to generate Claude Code integration artifacts
- **Implementation**:
  - Created `aida-core/src/scaffolding.rs` module with:
    - `ScaffoldConfig` struct for configuration options
    - `ProjectType` enum (Generic, Rust, Python, TypeScript, Web, Api, Cli)
    - `Scaffolder` struct with `preview()` and `apply()` methods
    - `ScaffoldArtifact` and `ScaffoldPreview` for generated content
  - Added to `aida-core/src/lib.rs` with public re-exports
  - Updated `aida-gui/src/app.rs`:
    - Added scaffolding state fields: `show_scaffold_dialog`, `scaffold_config`, `scaffold_preview`, `scaffold_tech_stack_input`
    - Added "Claude Code Integration" section to Settings > AI tab
    - Added "🔧 Scaffold Project" button that opens dialog
    - Implemented `show_scaffold_dialog()` method with:
      - Artifact selection checkboxes (CLAUDE.md, commands, skills)
      - Project type dropdown
      - Tech stack input with add/remove
      - Preview section showing new files, overwrites, directories
      - Collapsible artifact details
      - Apply and Cancel buttons
- **Generated Artifacts**:
  - `CLAUDE.md` - Project instructions with title, description, tech stack, features, type-specific commands
  - `.claude/commands/status.md` - Project status command
  - `.claude/commands/review.md` - Requirement review command
  - `.claude/skills/aida-req.md` - Requirement creation skill
  - `.claude/skills/aida-implement.md` - Implementation skill with language-specific traceability examples
- **Features**:
  - Preview shows new files vs overwrites with color coding
  - Refresh preview button when config changes
  - Tech stack customization
  - Project type selection affects generated command examples
- **Status**: FR-0152 marked as Completed (partial - new project wizard integration TODO)


### Stale Data Protection / Conflict Detection (FR-0153)
- **Prompt**: Implement FR-0153 - YAML database store last updated and do not overwrite when stale
- **Solution**: Implemented optimistic concurrency control with field-level conflict detection and resolution
- **Implementation**:
  - **aida-core/src/storage.rs**:
    - Added conflict detection types at top of file:
      - `ConflictInfo` - holds requirement ID, spec_id, conflicting fields, disk/local versions
      - `FieldConflict` - holds field name, original/disk/local values
      - `SaveResult` - enum: Success, Merged { merged_count }, Conflict(ConflictInfo)
      - `ConflictResolution` - enum: ForceLocal, KeepDisk, Merge
    - Added `save_with_conflict_detection()` method:
      - Takes original timestamps HashMap and modified IDs HashSet
      - Reloads database from disk before saving
      - Compares timestamps for each modified requirement
      - If timestamp unchanged, saves normally
      - If timestamp newer on disk, checks for field conflicts
      - If no field conflicts, auto-merges and continues
      - If field conflicts exist, returns ConflictInfo for first conflict
    - Added `detect_field_conflicts()` method:
      - Compares 8 key fields: title, description, status, priority, owner, feature, type, tags
      - Returns Vec<FieldConflict> with old/disk/local values for each conflict
    - Added `merge_requirement()` method:
      - Copies non-conflicting fields from disk version
      - Preserves comments, history, relationships, URLs from both versions
      - Updates modified_at timestamp
    - Added `save_with_resolution()` method:
      - Handles user's conflict resolution choice
      - ForceLocal: overwrites disk with local
      - KeepDisk: reloads and discards local changes
      - Merge: auto-merges non-conflicting changes
    - Added `get_requirement_timestamps()` helper for initial snapshot
    - Added 6 unit tests for conflict scenarios
  - **aida-core/src/lib.rs**:
    - Added exports: ConflictInfo, ConflictResolution, FieldConflict, SaveResult
  - **aida-gui/src/app.rs**:
    - Added imports for conflict types and chrono DateTime
    - Added state fields to RequirementsApp:
      - `original_timestamps: HashMap<Uuid, DateTime<Utc>>` - snapshot at load time
      - `modified_requirement_ids: HashSet<Uuid>` - tracks locally modified requirements
      - `show_conflict_dialog: bool` - dialog visibility
      - `current_conflict: Option<ConflictInfo>` - current conflict to resolve
    - Modified `reload()` to update timestamps snapshot
    - Replaced `save()` with conflict-aware version using `save_with_conflict_detection()`
    - Added `mark_requirement_modified()` helper method
    - Added tracking calls in update_requirement(), toggle_archive(), status/priority change handlers
    - Added `show_conflict_resolution_dialog()` method:
      - Displays field-by-field comparison table
      - Shows conflicting field names with disk vs local values
      - Three resolution buttons: "Use My Changes", "Use Disk Version", "Merge (Keep Non-Conflicting)"
      - Cancel button to review further
- **Tests**: 6 tests added covering:
  - test_save_and_load - basic persistence
  - test_conflict_detection_no_conflict - no conflicts when disk unchanged
  - test_conflict_detection_with_external_change - detects when disk modified
  - test_conflict_resolution_force_local - ForceLocal overwrites disk
  - test_conflict_resolution_keep_disk - KeepDisk reloads disk version
  - test_get_requirement_timestamps - verifies timestamp tracking
- **Status**: FR-0153 marked as Completed


---

## Session 8: Timeline View and External Integration Architecture (2025-12-06)

### Search Highlighting Fix
- **Prompt**: "in filter mode I don't think it makes sense to highlight any reqs in yellow/orange"
- **Actions**:
  - Modified `show_draggable_requirement()` and `show_draggable_requirement_inline()` in app.rs
  - Changed `is_current_match` to only apply highlighting in Highlight mode, not Filter mode
  - In Filter mode, all visible items are already search matches, so highlighting is redundant

### Timeline View Implementation
- **Prompt**: "I wonder if we could have a timeline view where I can go back and forward to see requirements changes over time"
- **Actions**:
  - Added `Timeline` variant to the `View` enum
  - Created `TimelineEvent` struct with fields:
    - `timestamp`: When the event occurred
    - `event_type`: Created, Modified, CommentAdded, BaselineCreated
    - `req_id`: UUID of related requirement
    - `spec_id`: Human-readable requirement ID
    - `req_title`: Requirement title
    - `author`: Who made the change
    - `description`: Event description
    - `changes`: Vec<FieldChange> for modification details
  - Created `TimelineEventType` enum with display methods `icon()` and `label()`
  - Added state fields to RequirementsApp:
    - `timeline_selected_date`, `timeline_events`
    - `timeline_filter_author`, `timeline_filter_field`
    - `timeline_selected_event_idx`
  - Implemented `rebuild_timeline_events()`:
    - Collects events from requirement creation, history entries, comments, and baselines
    - Sorts events chronologically (newest first)
  - Implemented `collect_comment_events()` helper for recursive comment tree traversal
  - Implemented `get_filtered_timeline_events()` for author/field filtering
  - Implemented `show_timeline_view()`:
    - Two-column layout: event list on left, detail on right
    - Date grouping with headers
    - Event selection with single-click
    - Navigation to requirement with double-click or spec_id link
    - Shows field changes with old→new value display for modifications
  - Added `truncate_string()` helper function
  - Added Timeline to View menu
- **Technical Fixes**:
  - Fixed `req.created_by` type mismatch (Option<String> vs String) using `unwrap_or_else`
  - Fixed Rust borrow checker issues by pre-computing data before closures
  - Extracted state mutations to after closure execution using mutable variables

### External Integration Architecture Document
- **Prompt**: "consider gitlab integration and write a architecture document on how we could accomplish that, also consider github and jira"
- **Actions**:
  - Created `/docs/EXTERNAL_INTEGRATION_ARCHITECTURE.md`
  - Designed Integration Manager component for coordinating all connectors
  - Defined `IssueConnector` trait with methods:
    - `test_connection()`, `fetch_issues()`, `fetch_issue()`
    - `create_issue()`, `update_issue()`
    - `fetch_comments()`, `add_comment()`
    - `webhook_config()`, `handle_webhook()`
  - Designed platform-specific connectors:
    - GitLabConnector: Project path, milestones, labels, merge requests
    - GitHubConnector: Owner/repo, milestones, labels, pull requests
    - JiraConnector: Project key, issue types, statuses, sprints, transitions
  - Created ExternalLinkStore for tracking requirement-to-issue relationships
  - Defined field mapping layer for translating between platforms
  - Documented synchronization processes:
    - Initial import workflow
    - Polling-based continuous sync
    - Webhook-based real-time sync
  - Outlined conflict resolution strategies:
    - AidaWins, ExternalWins, LastWriteWins, ManualResolve
  - Defined configuration storage in requirements.yaml
  - Addressed security considerations:
    - Credential storage via system keyring
    - Webhook signature validation
    - API rate limiting
  - Proposed 4-phase implementation roadmap:
    - Phase 1: Read-only import
    - Phase 2: Bidirectional sync
    - Phase 3: Real-time sync via webhooks
    - Phase 4: Extended features (GitHub, Jira, custom fields)
  - Described GUI and CLI integration points

---

## Session 9: Kanban Navigation, Sprint Selection, Git Hooks & Documentation (2025-12-07)

### Kanban Column Navigation
- **Prompt**: "Have Ctrl+L and Ctrl+H move the req to the next or previous column in the board (do not wrap)"
- **Actions**:
  - Modified `handle_kanban_keyboard()` in app.rs
  - Added Ctrl+L to move selected requirement to next status column
  - Added Ctrl+H to move selected requirement to previous status column
  - No wrapping - stays at first/last column when at boundary

### Sprint Selection in Planning View
- **Prompt**: "In the Sprint Planning view, how about being able to select the sprints themselves so that we can edit, rename etc."
- **Actions**:
  - Added `planning_selected_sprint: Option<Uuid>` state field
  - Made sprint headers clickable in Sprint Planning view
  - Updated detail panel to show sprint details when sprint is selected
  - Sprint selection is mutually exclusive with item selection

### Git Hooks for Code Traceability
- **Prompt**: "Review ~/ai/ai-provenance and import features to AIDA"
- **Analysis**:
  - Compared ai-provenance project features with AIDA capabilities
  - Identified gaps: git hooks, trace query commands, CI/CD templates
  - ai-provenance has: hierarchical metadata, git notes support, CI/CD templates
- **Requirements Created**:
  - FR-0226: Git Hooks for Code Traceability Validation
  - FR-0227: Code Traceability Query and Reporting
  - FR-0228: CI/CD Template Generation for Traceability
  - All linked as children of FR-0149 (Claude AI Configuration Scaffolding)
- **Implementation** (FR-0226):
  - Added config fields to `ScaffoldConfig`:
    - `generate_git_hooks: bool`
    - `include_commit_msg_hook: bool`
    - `include_pre_commit_hook: bool`
  - Added hook preview/generation in `preview()` method
  - Implemented `generate_commit_msg_hook()`:
    - Validates SPEC-ID references in commit messages
    - Non-blocking warnings for invalid references by default
  - Implemented `generate_pre_commit_hook()`:
    - Validates trace comments in staged files
    - Non-blocking warnings for malformed traces by default
  - Added Unix executable permissions for generated hooks
  - Only generates hooks if .git directory exists

### Documentation Updates
- **Actions**:
  - Added Section 14 "Code Traceability & Git Hooks" to DEVELOPER_GUIDE.md
  - Documented trace comment format: `// trace:SPEC-ID | ai:tool:confidence`
  - Documented commit-msg and pre-commit hooks functionality
  - Added configuration options documentation

### HTML Guide Generation
- **Prompt**: "please generate the html version of the guides"
- **Actions**:
  - Used pandoc to regenerate all three HTML guides:
    - user-guide.html
    - admin-guide.html
    - DEVELOPER_GUIDE.html
  - Added consistent navigation header with links between guides
  - Added dark/light mode theme toggle
  - Styled with embedded CSS for professional appearance

### AIDA Slideshow Creation
- **Prompt**: "I would like to supplement these guides with a slideshow showcasing the capabilities of AIDA"
- **Actions**:
  - Created `/docs/slideshow.html` - comprehensive 16-slide presentation
  - Slides cover:
    - Title and Overview
    - List View, Kanban View, Timeline View, Sprint Planning
    - Keyboard Navigation, Requirement Types, Relationships
    - AI Integration, Project Scaffolding, Code Traceability
    - Themes, CLI Commands, Multi-Project Support, Getting Started
  - Features:
    - Dark/light mode theme toggle
    - Keyboard navigation (arrows, Home, End)
    - Progress indicator
    - Responsive layout
  - Created `/docs/images/` directory for screenshots
  - Identified 12 screenshots needed:
    - ss-overview.png, ss-list-view.png, ss-kanban.png
    - ss-timeline.png, ss-sprint.png, ss-relationships.png
    - ss-ai-evaluation.png, ss-scaffold.png, ss-themes.png
    - ss-projects.png, ss-keyboard.png, ss-cli.png

### GUI gRPC Client Support (FR-0227 continued)
- **Prompt**: "how do I start the client gui to connect to the server backend?" → "yes please implement GUI gRPC client support"
- **Actions**:
  - Added gRPC dependencies to aida-gui/Cargo.toml (tonic, prost, tokio as optional)
  - Created "remote" feature flag for conditional compilation
  - Created aida-gui/build.rs for proto compilation (client-only)
  - Created aida-gui/src/remote.rs with:
    - `StorageBackend` enum (Local/Remote) for transparent backend abstraction
    - `RemoteStorage` struct with tokio runtime and Arc<Mutex<Client>>
    - Proto-to-Rust type conversions (RequirementsStore, Requirements, etc.)
    - `normalize_addr()` for flexible address format handling
  - Updated aida-gui/src/main.rs:
    - Added CliArgs struct for argument parsing
    - Added --server/-s flag for remote server address
    - Added AIDA_SERVER environment variable support
    - Added --help/-h and --version/-V flags
    - Dynamic window title showing connection type
  - Updated aida-gui/src/app.rs:
    - Added `remote_client` and `server_addr` fields to RequirementsApp
    - Created `new_with_config()` method to dispatch to local/remote
    - Created `new_with_server()` method for remote initialization
- **Usage**:
  ```bash
  cargo build -p aida-gui --features remote  # Build with remote support
  aida-gui --server localhost:50051          # Connect to remote server
  AIDA_SERVER=localhost:50051 aida-gui       # Via environment variable
  ```
- **Note**: Save operations currently stubbed (read-only client)

### Tag Picker Popup (FR-0233)
- **Prompt**: "Implement tag assignment popup with 't' hotkey"
- **Actions**:
  - Added `OpenTagPicker` to `KeyAction` enum
  - Added state fields: `show_tag_picker`, `tag_picker_search`, `tag_picker_selected_tags`, `tag_picker_dropdown_idx`
  - Created `show_tag_picker_popup()` function with:
    - Fuzzy search filtering
    - Multi-select support (Space to toggle)
    - Enter to apply selected tags
    - Shows existing tags + available tags from all requirements
  - Added Tags variant to QuickChangeField enum match statements

### SQLite/YAML Auto-Detection Bug Fix (REQ-0234)
- **Prompt**: "I got this [Multiple Database Files Found dialog] and then there were no requirements shown when I clicked ok"
- **Root Cause**: Storage.load() was trying to parse SQLite file as YAML when user selected SQLite
- **Solution**: Added SQLite auto-detection in Storage class
- **Implementation**:
  - Added `is_sqlite` detection in `Storage::load()` and `Storage::save()` based on file extension (.db, .sqlite, .sqlite3)
  - Added `load_sqlite()` and `save_sqlite()` helper methods using `SqliteBackend`
  - Fixed GUI to skip migration check when `--file` is explicitly provided

### Migration Warning Dialog UX Improvements
- **Prompt**: "Center the OK button, and add a 'do not show warning again' checkbox"
- **Actions**:
  - Centered OK button using `ui.with_layout()` and `egui::Align::Center`
  - Added "don't show again" checkbox
  - Reduced dialog height by removing unnecessary spacing
  - Added `migration_yaml_path` and `migration_dont_show_again` fields to track state

### CLI --file Argument Fix
- **Prompt**: "For the last /aida-capture we had FR-0233 and REQ-0234 but I do not see 233 or 234 in the database, maybe they are in the yaml?"
- **Root Cause**: CLI defined `--file` option but never used it; path determination only used `cli.project`
- **Solution**:
  - Changed `cli.file` from `String` with default to `Option<String>`
  - Updated main.rs to check `cli.file` first before auto-detection
  - When `--file` is specified, bypasses migration status check entirely
  - CLI now defaults to SQLite when both files exist (matching GUI behavior)
- **Database Sync**:
  - Migrated YAML to SQLite to sync FR-0233 and REQ-0234
  - Used `aida db migrate --from yaml --to sqlite --force` with explicit `--file requirements.yaml`

### Add Menu Popup ('a' hotkey)
- **Prompt**: "Since 'n' and Shift+N are used for search, I think I should eliminate the overlap and maybe use 'a' for add"
- **Solution**: Implemented add menu popup similar to delete menu
- **Implementation**:
  - Added `OpenAddMenu` to `KeyAction` enum
  - Added `show_add_menu` and `add_menu_selected` state fields
  - Created `show_add_menu_popup()` function with:
    - 's' for New Sibling requirement
    - 'c' for New Child requirement
    - Arrow/j/k navigation with Enter to confirm
    - Escape to close
  - Added `start_add_sibling()` and `start_add_child()` helper methods
  - Removed old 'n'/'N' bindings for NewSiblingRequirement/NewChildRequirement
  - 'n'/'N' are now exclusively for search navigation (next/prev match)
  - Ctrl+N still works for smart new requirement with heuristic parent
- **Key Bindings Summary**:
  - 'a' -> opens add menu -> 's' for sibling, 'c' for child
  - 'n' -> next search match (when search active)
  - 'N' (Shift+N) -> previous search match
  - Ctrl+N -> smart new requirement (global)

### Database Change Detection and Auto-Reload
- **Prompt**: "Maybe we should have a periodic check for updates to the database and alert the user"
- **Requirements**:
  - Auto-reload when not editing (silent, preserves selection)
  - Toast notification when editing with deferred reload
  - Configurable poll interval (default 30 seconds)
- **Implementation**:
  - Added `UserSettings` fields:
    - `db_poll_interval_secs` (default 30): check interval, 0 to disable
    - `db_auto_reload` (default true): auto-reload when not editing
  - Added `RequirementsApp` state fields:
    - `last_db_check`: tracks last file mtime check time
    - `known_db_mtime`: last known modification time
    - `pending_external_reload`: flag for deferred reload
    - `external_change_detected_at`: timestamp of detected change
  - Core methods:
    - `get_db_file_mtime()`: get file modification time
    - `check_for_external_db_changes()`: periodic mtime comparison
    - `reload_database()`: reload with selection preservation
    - `is_editing()`: check if in Edit/Add view
    - `handle_db_change_detection()`: main detection logic
  - Integrated into update loop for continuous monitoring
- **Behavior**:
  - When not editing: auto-reload if enabled, else show toast
  - When editing: show toast "Database changed externally, reload pending"
  - Selection is preserved across reloads by matching requirement IDs


### Personal Work Queue Feature
- **Prompt**: "I would like a user queue. This is like an inbox that the user can self manage in terms of relative ranking. So we have a ranking say an integer 1-100 with lower number higher in rank."
- **Requirements**:
  - User-managed work queue (not requirement metadata)
  - Rankings 1-100 (lower = higher priority)
  - Same-rank items use requirement priority as tiebreaker
  - Hotkeys: 'q t' (top), 'q b' (bottom), 'q m' (middle), 'q d' (remove), 'q v' (view queue)
  - Queue view also accessible via 'v q'
  - Reorder in queue view with Ctrl+Up/Down or Ctrl+k/j
- **Implementation**:
  - Added `QueueEntry` struct in `aida-gui/src/app.rs`:
    - `requirement_id: Uuid` - reference to the requirement
    - `rank: u8` - priority ranking (1-100)
    - `added_at: DateTime<Utc>` - timestamp
  - Added `queue: Vec<QueueEntry>` field to `UserSettings`
  - Added queue management methods to `UserSettings`:
    - `is_in_queue()`, `queue_position()` - queries
    - `queue_add_top()`, `queue_add_middle()`, `queue_add_bottom()` - add operations
    - `queue_remove()`, `queue_move_up()`, `queue_move_down()` - remove/reorder
    - `renumber_queue_ranks()`, `get_sorted_queue()` - utility methods
  - Added `View::Queue` to View enum
  - Added state fields: `show_queue_menu`, `queue_menu_selected`, `queue_selected_idx`
  - Added 'q' key handler for queue popup menu
  - Added queue to view picker ('v q')
  - Added `show_queue_menu_popup()` function for queue action menu
  - Added `show_queue_view()` function for queue list display
  - Queue stored per-user in settings (~/.config/aida/aida_gui_settings.yaml)
- **Key Bindings**:
  - 'q' -> opens queue menu
    - 't' -> add to top of queue
    - 'm' -> add to middle of queue
    - 'b' -> add to bottom of queue
    - 'd' -> remove from queue
    - 'v' -> view queue
  - 'v q' -> view queue (via view picker)
  - In queue view:
    - Arrow keys / j/k -> navigate
    - Ctrl+Up/Down or Ctrl+k/j -> reorder item
    - d/Delete/Backspace -> remove from queue
    - Enter -> select item for detail view

---

## Session 10: WASM Browser Client (2025-12-12)

### WASM Browser Client Implementation (FR-0273)
- **Prompt**: "please start on the WASM browser client"
- **Problem**: Need browser-based access to AIDA for users without native client installation
- **Solution**: Created new `aida-web` crate compiled to WebAssembly, connecting via gRPC-Web
- **Technology Decisions**:
  - New crate instead of modifying `aida-gui` (heavy native dependencies)
  - `eframe`/`egui` for UI (same as native GUI)
  - `trunk` for WASM compilation and bundling
  - `tonic-web-wasm-client` for gRPC-Web protocol
- **Implementation**:
  - Created `aida-web/Cargo.toml` with WASM-compatible dependencies:
    - eframe 0.29 (default_fonts, glow features)
    - tonic 0.12 (client-only, no transport)
    - tonic-web-wasm-client 0.6
    - web-sys with HtmlCanvasElement, Element, HtmlElement features
    - chrono with wasmbind feature
  - Created `aida-web/build.rs` for proto compilation (client-only)
  - Created `aida-web/Trunk.toml` for build configuration (port 8088)
  - Created `aida-web/index.html` with loading indicator
  - Created `aida-web/src/lib.rs` - module definitions
  - Created `aida-web/src/main.rs` - WASM entry point with canvas creation
  - Created `aida-web/src/client.rs` - gRPC-Web client wrapper using `tonic-web-wasm-client::Client`
  - Created `aida-web/src/app.rs` - main egui application (~700 lines):
    - Connection state management
    - Requirements list with search
    - Detail view for selected requirement
    - Create/edit forms
    - Comment support
    - Async state with `Rc<RefCell<SharedState>>`
  - Updated workspace `Cargo.toml` to include aida-web
  - Added Makefile targets: web-build, web-build-release, web-serve, web-clean, web-deps, web-proto
- **Technical Fixes During Implementation**:
  - Missing favicon.ico - removed from index.html
  - Borrow checker error in draw_detail_view - cloned requirement, used edit_clicked flag
  - eframe API mismatch - WebRunner.start expects HtmlCanvasElement, not string
  - Missing web-sys features - added HtmlCanvasElement, Element, HtmlElement
  - Multiple target artifacts - added data-bin="aida-web" to index.html
- **Features**:
  - Connect to server via gRPC-Web
  - View requirements list with search
  - View requirement details
  - Create new requirements
  - Edit requirements (title, description, status)
  - Add comments
- **Usage**:
  ```bash
  make web-deps                # Install trunk and wasm32 target
  make run-server              # Start the gRPC server
  make web-serve               # Serve WASM client on http://localhost:8088
  ```
- **Status**: FR-0273 marked as Completed

---

### Unified Storage Architecture Integration (FR-0278)
- **Prompt**: Continue with unified storage abstraction integration into aida-gui
- **Problem**: GUI has separate code paths for local (Storage) and remote (remote_client) operations
- **Solution**: Created unified StorageClient trait and integrated into RequirementsApp
- **Implementation**:
  - Created `aida-gui/src/storage/` module:
    - `traits.rs` - StorageClient trait, ServerStatus, StorageError types
    - `grpc_client.rs` - GrpcStorageClient implementing StorageClient via tonic
    - `embedded.rs` - EmbeddedServer wrapper for spawning local aida-server subprocess
    - `mod.rs` - Factory function `create_storage_client()`
  - Updated `aida-gui/Cargo.toml`:
    - Made tonic, prost, tokio always available (not optional)
    - Added libc for native Unix signal handling
  - Updated `aida-gui/src/app.rs`:
    - Added `storage_client: Option<Box<dyn StorageClient>>` field
    - Updated `new_with_server()` to create storage_client via `create_storage_client()`
    - Updated `save()` to prefer storage_client over legacy remote_client
- **Architecture Benefits**:
  - Consistent interface for both local and remote storage
  - Reduced conditional compilation in business logic
  - Path toward full gRPC-based storage (even for local)
  - EmbeddedServer will spawn aida-server subprocess for local storage
- **Commits**:
  - 527848a: feat: add unified storage abstraction with StorageClient trait
  - 402be3c: feat: integrate storage_client into RequirementsApp
- **Status**: Phase 1 complete - storage_client integrated, legacy code preserved for compatibility
