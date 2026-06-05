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

---

### Shared UI Components for Native/WASM Code Reuse
- **Prompt**: Bring WASM UI in line with desktop UI with maximum code reuse
- **Problem**: aida-web had duplicate rendering code, inconsistent with native GUI
- **Solution**: Create shared UI components in aida-gui that work on both platforms
- **Implementation**:
  - Created `aida-gui/src/ui/` module with pure egui rendering functions:
    - `formatters.rs` - Text formatters for status/priority/type/timestamps
    - `badges.rs` - Colored badge/dot rendering for status, priority, type
    - `list_item.rs` - Requirement list item with configurable display options
    - `requirement_form.rs` - Form components with combo boxes for status/priority/type
    - `comment_list.rs` - Comment rendering and input components
    - `detail_view.rs` - Full requirement detail view rendering
    - `mod.rs` - Module root exporting all components
  - Updated `aida-gui/src/lib.rs`:
    - Added `pub mod ui;` for both native and web builds
  - Updated `aida-web/src/lib.rs`:
    - Re-export proto from `aida_gui::storage::proto` for type compatibility
  - Updated `aida-web/src/app.rs`:
    - Import shared UI components from `aida_gui::ui`
    - Use `requirement_list_item` for list rendering
    - Use `status_combo`, `priority_combo`, `type_combo` for forms
    - Use `comment_list`, `comment_input` for comments
    - Removed duplicate local helper functions
- **Key Design Decision**: Extract pure egui rendering functions that work with proto types, rather than refactoring the full 29k-line app.rs for dual-target
- **Commits**:
  - 7758d76: [AI:claude:high] feat: add shared UI components for native/WASM code reuse
- **Benefits**:
  - Consistent UI rendering between native desktop and browser clients
  - Reduced code duplication (~200 lines removed from aida-web)
  - Shared proto types ensure type compatibility
  - Easy to add more shared components incrementally
- **Status**: Complete - both native (cargo build -p aida-gui) and WASM (trunk build) compile successfully

### Server --force Option for Port Conflicts (2025-12-12)
- **Prompt**: Add --force option to aida-server to kill existing processes on ports
- **Problem**: "Address already in use (os error 98)" when starting server with existing process on port
- **Solution**: Add `--force/-f` CLI option that kills processes using the specified ports before binding
- **Implementation**:
  - Added `--force` flag to Args struct in `aida-server/src/main.rs`
  - Created `kill_process_on_port()` function using `lsof -t -i:PORT` to find PIDs
  - Created `kill_process_on_port_ss()` fallback using `ss` command for Linux systems without lsof
  - Process termination uses SIGTERM first, then SIGKILL if needed
  - Added 100ms delay after killing to allow OS to release ports
  - Updated Makefile with `FORCE=1` variable for easy usage: `make run-server FORCE=1`
- **Commits**:
  - ac74f49: feat: add --force option to aida-server to kill existing processes
- **Status**: Complete

### Dual-Target GUI Compilation (Native + WASM) (2025-12-12)
- **Prompt**: Make aida-gui compile for both native desktop and WASM browser targets
- **Goal**: Provide nearly identical user experience for core requirements management on both platforms
- **Problem**: aida-gui had ~30k lines with native-only dependencies (threads, filesystem, SQLite)
- **Solution**: Add conditional compilation to gate native-only features while preserving shared UI
- **Implementation**:
  - **Phase 1: Conditional Thread Spawning**
    - Gate heartbeat thread, AI evaluation thread, duplicate detection thread
    - Use `#[cfg(not(target_arch = "wasm32"))]` for native-only code
  - **Phase 2: Struct Field Gating**
    - Gate native-only struct fields (storage, ai_client, edit_lock, etc.)
    - Add separate initialization for WASM builds
  - **Phase 3: Type and Import Gating**
    - Split imports into cross-platform and native-only sections
    - Gate types like `Storage`, `EditLock`, `ConflictInfo`, `MigrationCheck`
  - **Phase 4: Entry Point Configuration**
    - Update `lib.rs` with wasm_bindgen(start) entry point
    - Update `main.rs` with cfg-gated native/WASM main functions
    - Configure `index.html` with trunk build options
  - **Phase 5: Makefile Updates**
    - Update web-* targets to use aida-gui (full-featured)
    - Add web-*-lite targets for aida-web (lightweight alternative)
- **Key Files Modified**:
  - `aida-gui/src/app.rs` - Extensive conditional compilation (~1000+ lines changed)
  - `aida-gui/src/lib.rs` - WASM entry point with canvas setup
  - `aida-gui/src/main.rs` - Cfg-gated native/WASM main functions
  - `aida-gui/Cargo.toml` - Added web-sys features (HtmlCanvasElement, Element)
  - `aida-gui/index.html` - Trunk build configuration
  - `aida-core/Cargo.toml` - WASM uuid/getrandom with js feature
  - `Makefile` - Updated web build targets
- **Build Commands**:
  - Native: `cargo build -p aida-gui`
  - WASM: `cd aida-gui && trunk build` (or `make web-build`)
- **Commits**:
  - 2453be9: feat: enable aida-gui dual-target compilation for native and WASM
- **Status**: Complete - both native and WASM builds compile successfully
- **Benefits**:
  - Same codebase for desktop and browser
  - Nearly identical UI on both platforms
  - Reduced maintenance burden
  - aida-web remains available as lightweight alternative

### WASM Browser Client Testing (2025-12-12)
- **Prompt**: Continue testing WASM client in browser with server
- **Goal**: Verify dual-target WASM build works in browser environment
- **Testing Steps**:
  1. Built WASM client: `cd aida-gui && trunk build`
  2. Started aida-server with gRPC-Web support: port 50051 (gRPC), port 8080 (REST)
  3. Started trunk serve: `trunk serve --port 8088`
  4. Verified web client accessible at http://localhost:8088 (HTTP 200)
  5. Confirmed WASM module loading: `aida-gui-7f1acc2a63f43ac1_bg.wasm` (24.4 MB)
- **Build Artifacts**:
  - `aida-gui/dist/aida-gui-7f1acc2a63f43ac1_bg.wasm` - WASM binary (24.4 MB)
  - `aida-gui/dist/aida-gui-7f1acc2a63f43ac1.js` - JS bindings (81 KB)
- **Additional Changes**:
  - Added `.gitignore` entries for WASM dist folders
- **Testing Commands**:
  ```bash
  # Start server
  make run-server FORCE=1

  # Build and serve WASM client
  make web-build
  make web-serve

  # Access browser client
  http://localhost:8088
  ```
- **Status**: Complete - WASM build compiles and serves successfully

### PostgreSQL Database Backend (2025-12-13)
- **Prompt**: Add PostgreSQL as database backend option alongside SQLite and YAML
- **Requirement**: FR-0316
- **Goal**: Enable enterprise-grade PostgreSQL deployments for multi-user scenarios
- **Implementation**:
  1. **Dependencies** (Cargo.toml):
     - Added `postgres` crate with UUID, chrono, serde_json features
     - Added `r2d2` and `r2d2_postgres` for connection pooling
     - New feature flag: `postgres` in aida-core
  2. **BackendType enum** (traits.rs):
     - Added `BackendType::Postgres` variant
     - Updated Display implementation
  3. **Schema** (postgres_schema.sql):
     - PostgreSQL-native schema with JSONB columns
     - TIMESTAMPTZ for timestamps, UUID native type
     - Identical table structure to SQLite
  4. **PostgresBackend** (postgres_backend.rs):
     - ~1000 line implementation
     - r2d2 connection pooling (max 10 connections)
     - GenericClient trait for transaction support
     - Full CRUD operations with optimistic locking
     - All trait methods implemented
  5. **Factory function** (mod.rs):
     - Auto-detect `postgres://` connection strings
     - Feature-gated instantiation
  6. **Migration functions** (migration.rs):
     - `migrate_to_postgres()` - any backend to PostgreSQL
     - `migrate_from_postgres()` - PostgreSQL to any backend
  7. **CLI support** (main.rs, cli.rs):
     - `aida db migrate --from <format> --to postgres --output <conn_string>`
     - `aida db info` shows PostgreSQL-specific info
     - Direct usage: `aida --file "postgres://..." list`
- **Key Files Modified**:
  - `Cargo.toml` - workspace dependencies
  - `aida-core/Cargo.toml` - postgres feature
  - `aida-core/src/db/traits.rs` - BackendType::Postgres
  - `aida-core/src/db/mod.rs` - factory function
  - `aida-core/src/db/postgres_backend.rs` - NEW (1030 lines)
  - `aida-core/src/db/postgres_schema.sql` - NEW (92 lines)
  - `aida-core/src/db/migration.rs` - postgres migrations
  - `aida-core/src/lib.rs` - re-exports
  - `aida-cli/Cargo.toml` - enable postgres feature
  - `aida-cli/src/cli.rs` - migrate command update
  - `aida-cli/src/main.rs` - handle postgres migrations
- **Usage**:
  ```bash
  # Migrate to PostgreSQL
  aida db migrate --from sqlite --to postgres --output "postgres://user:pass@localhost:5432/aida"

  # Use PostgreSQL directly
  aida --file "postgres://user:pass@localhost:5432/aida" list

  # Show database info
  aida --file "postgres://..." db info
  ```
- **Status**: Complete - compiles successfully

### Edit View Tabs Matching Detail View (2025-12-14)
- **Prompt**: Implement FR-0295 - Edit req should show tabs same as Detail view
- **Requirement**: FR-0295
- **Goal**: Add tabbed interface to Edit view matching Detail view for consistent navigation
- **Implementation**:
  1. **Tab Bar in Edit Mode** (app.rs):
     - Added tab bar to `show_form_stacked()` after title bar
     - Tabs: AI, Fields, Comments, Links, Attachments, History
     - "Fields" tab shows editable form (renamed from "Description" for clarity)
     - Other tabs show read-only content using existing tab display functions
  2. **Content Routing**:
     - `show_fields` conditional routes to appropriate content
     - Fields tab shows left panel (metadata) + right panel (description editor)
     - Non-fields tabs call existing `show_ai_tab()`, `show_comments_tab()`, etc.
  3. **Tab Selection Persistence**:
     - Reuses existing `active_tab: DetailTab` field
     - Tab selection persists between Detail and Edit views
  4. **Keyboard Navigation** (FR-0295):
     - Added Ctrl+1-6 shortcuts for tab switching in Edit mode
     - Ctrl+1=AI, Ctrl+2=Fields, Ctrl+3=Comments, Ctrl+4=Links, Ctrl+5=Attachments, Ctrl+6=History
     - Handles both Ctrl (Linux/Windows) and Cmd (macOS)
- **Key Code Changes**:
  - Lines 26729-26785: Tab bar UI in `show_form_stacked()`
  - Lines 26792-27172: Conditional content based on `show_fields` and `active_tab`
  - Lines 30179-30200: Ctrl+1-6 keyboard shortcut handling
- **Commit**: a2d7cb1
- **Status**: Complete - FR-0295 marked as completed

### GitLab Integration Planning (2025-12-14)
- **Prompt**: Think deeply about GitLab integration - bugs/issues stored in GitLab with CRUD operations, or AIDA as master with periodic sync
- **Requirement**: EPIC-0320 (GitLab Integration)
- **Goal**: Design comprehensive GitLab integration with phased implementation

**Phase 1: Read-only Integration**
- STORY-0321: GitLab Connection Configuration
- STORY-0322: View GitLab Issues in AIDA
- STORY-0323: Link AIDA Requirements to GitLab Issues

**Phase 2: One-way Create**
- STORY-0324: Create GitLab Issue from AIDA Requirement
- STORY-0325: GitLab Sync State Tracking
- STORY-0326: GitLab Label Mapping Configuration

**Phase 3: Divergence Detection**
- STORY-0327: Poll GitLab for Changes
- STORY-0328: Divergence Detection and Display
- STORY-0329: GitLab Change Notifications

**Phase 4: Bidirectional Sync**
- STORY-0330: Push AIDA Changes to GitLab
- STORY-0331: Pull GitLab Changes to AIDA
- STORY-0332: Conflict Resolution for GitLab Sync
- STORY-0333: Automated Sync Rules Configuration

**Technical Spike**
- SPIKE-0334: GitLab Integration Architecture Design

**Key Design Decisions:**
1. Hybrid approach - AIDA as requirements master, GitLab for developer work
2. Phased implementation to deliver value incrementally
3. Polling-based change detection (webhooks as future enhancement)
4. Flexible sync rules - per-field direction control
5. Three-way diff for conflict resolution
6. Sync state tracked in separate table/collection
7. Direct reqwest for API calls (vs heavy gitlab crate)

- **Status**: Requirements captured, ready for implementation prioritization

### GitLab Integration Implementation - Phase 1 (2025-12-14)
- **Prompt**: "1" - Start implementing Phase 1 of GitLab integration
- **Requirement**: STORY-0321, STORY-0322

#### STORY-0321: GitLab Connection Configuration
**Goal**: Create GitLab API client and configuration system

**Implementation:**
1. **aida-core/src/integrations/gitlab/config.rs**:
   - `GitLabConfig` struct with URL, project_id, token, labels, polling, sync settings
   - `LabelConfig` for type/status/priority label mapping
   - `PollingConfig` for refresh interval and batch size
   - `SyncConfig` with mode (push-only/pull-only/bidirectional/manual)
   - Config file load/save to `~/.config/aida/gitlab.toml`
   - Token via `AIDA_GITLAB_TOKEN` environment variable

2. **aida-core/src/integrations/gitlab/client.rs**:
   - `GitLabClient` struct wrapping reqwest::Client
   - Async methods: `test_connection`, `list_issues`, `get_issue`, `create_issue`, `update_issue`, `add_comment`
   - Bearer token authentication
   - Error handling with `ClientError` enum

3. **aida-core/src/integrations/gitlab/models.rs**:
   - `GitLabProject`, `GitLabIssue`, `GitLabUser`, `GitLabLabel`, `GitLabMilestone`
   - `IssueState` (Opened/Closed), `MilestoneState` (Active/Closed)
   - `CreateIssueRequest`, `UpdateIssueRequest`, `CreateNoteRequest`
   - `IssueFilter` with state, labels, search, pagination

4. **CLI Commands** (aida-cli):
   - `aida gitlab config` - Configure GitLab connection
   - `aida gitlab test` - Test connection
   - `aida gitlab list` - List issues
   - `aida gitlab show <IID>` - Show issue details
   - `aida gitlab status` - Show linked issues with sync status

- **Commit**: 3999d48

#### STORY-0322: View GitLab Issues in AIDA (GUI)
**Goal**: Display GitLab issues in the GUI with detail panel

**Implementation:**
1. **New View Type**:
   - Added `View::GitLabIssues` enum variant
   - Added to view picker (shortcut: `v g`)

2. **App State Fields** (conditionally compiled for native):
   - `gitlab_config: Option<GitLabConfig>` - Loaded from config file
   - `gitlab_issues: Vec<GitLabIssue>` - Cached issues
   - `gitlab_last_fetch: Option<Instant>` - Cache timestamp
   - `gitlab_selected_issue: Option<u64>` - Selected IID
   - `gitlab_loading: bool` - Loading indicator
   - `gitlab_error: Option<String>` - Error display
   - `gitlab_filter_state: Option<IssueState>` - State filter
   - `gitlab_filter_search: String` - Search filter

3. **GitLab Issues View** (`show_gitlab_issues_view`):
   - Two-column layout (issues list / detail panel)
   - Header with 🦊 icon and "Refresh" button
   - State filter combo box (Open/Closed/All)
   - Search text field
   - Issues list with state icons (🟢/🔴) and selection
   - Last updated timestamp display
   - Loading spinner and error handling

4. **Issue Detail Panel** (`show_gitlab_issue_detail`):
   - Issue title with state badge
   - Author and assignee info
   - Labels display
   - Created/updated timestamps
   - Markdown-rendered description
   - "Open in Browser" button

- **Commit**: 7c64d0f
- **Status**: STORY-0321 and STORY-0322 completed

#### STORY-0323: Link AIDA Requirements to GitLab Issues
**Goal**: Create traceability links between AIDA requirements and GitLab issues

**Implementation:**

1. **Data Model (aida-core/src/models.rs)**:
   - `GitLabIssueLink` struct with:
     - `id: Uuid` - Unique link identifier
     - `issue_iid: u64` - GitLab issue IID (project-scoped)
     - `project_id: Option<u64>` - Optional project override
     - `issue_title: String` - Cached title from GitLab
     - `link_type: GitLabLinkType` - Relationship type
     - `notes: Option<String>` - Optional notes
     - `created_at`, `created_by` - Audit fields
     - `last_synced`, `issue_state` - Sync metadata
   - `GitLabLinkType` enum: ImplementedBy, TracesTo, RelatedBug, FollowUp
   - Added `gitlab_issues: Vec<GitLabIssueLink>` to Requirement struct

2. **GUI - Links Tab Section**:
   - New "🦊 GitLab Issues" section between URLs and Relationships
   - Shows linked issues with:
     - State icons (🟢 open, 🔴 closed, ⚪ unknown)
     - Link type badge ([impl], [trace], [bug], [followup])
     - Issue ID and title as clickable link
     - Sync timestamp
   - Remove button (x) to unlink issues
   - "➕ Link Issue" button (only shown when GitLab configured)

3. **GUI - Link Picker Modal**:
   - Search field to filter cached GitLab issues
   - Lists up to 20 matching issues with state icons
   - Click to select and create link
   - Duplicate detection (prevents linking same issue twice)
   - Cancel button to close without linking

4. **State Management**:
   - `show_gitlab_link_picker: bool` - Controls modal visibility
   - `gitlab_link_picker_search: String` - Search text in picker
   - `gitlab_link_picker_req_id: Option<Uuid>` - Target requirement

5. **Backend Updates**:
   - SQLite: Added `gitlab_issues: Vec::new()` initialization
   - PostgreSQL: Added `gitlab_issues: Vec::new()` initialization
   - gRPC client: Added `gitlab_issues: Vec::new()` initialization

- **Commit**: cac0ee8
- **Status**: STORY-0323 completed

**Phase 1 Complete!** All three stories in Phase 1 (GitLab Read-only Integration) are now complete:
- STORY-0321: GitLab Connection Configuration ✓
- STORY-0322: View GitLab Issues in AIDA ✓
- STORY-0323: Link AIDA Requirements to GitLab Issues ✓

---

### Phase 2: GitLab Write Integration

#### STORY-0324: Create GitLab Issue from AIDA Requirement
**Goal**: Allow users to create new GitLab issues directly from AIDA requirements

**Implementation:**

1. **App State Fields** (aida-gui/src/app.rs):
   - `show_create_gitlab_issue_dialog: bool` - Controls dialog visibility
   - `create_gitlab_issue_req_id: Option<Uuid>` - Source requirement
   - `create_gitlab_issue_title: String` - Editable issue title
   - `create_gitlab_issue_description: String` - Editable issue description
   - `create_gitlab_issue_labels: String` - Comma-separated labels
   - `create_gitlab_issue_creating: bool` - Loading indicator

2. **Create Issue Button**:
   - Added "🆕 Create Issue" button next to "Link Issue" in GitLab links section
   - Pre-populates from requirement:
     - Title: `[SPEC-ID] Requirement Title`
     - Description: Markdown with requirement details and trace link
     - Labels: `aida:<type>` and `priority:<priority>` (if not medium)

3. **Create Issue Dialog Modal** (`show_create_gitlab_issue_modal`):
   - Centered window with 500px default width
   - Shows source requirement spec_id
   - Editable fields:
     - Title (single line)
     - Description (multiline, 200px height with scroll)
     - Labels (comma-separated)
   - Action buttons:
     - "✓ Create Issue" (disabled if title empty)
     - "Cancel"
   - Loading spinner during creation

4. **Async Issue Creation**:
   - Uses `tokio::runtime::Runtime::block_on()` for sync GUI context
   - Calls `GitLabClient::new(config)` and `client.create_issue(request)`
   - On success:
     - Adds issue to cached `gitlab_issues` list
     - Auto-creates `GitLabIssueLink` with type `ImplementedBy`
     - Saves requirement with new link
     - Shows success toast: "Created GL-{iid}: {title}"
   - On error:
     - Shows error toast with message
     - Keeps dialog open for retry

5. **Field Mapping (AIDA → GitLab)**:
   - `req.title` → Issue title (with spec_id prefix)
   - `req.description` → Issue description (in markdown)
   - `req.req_type` → Label `aida:<type>`
   - `req.priority` → Label `priority:<priority>` (if not medium)

- **Commit**: 185c928
- **Status**: STORY-0324 completed

---

## Session 11: GitLab Sync State Tracking (2025-12-14)

### GitLab Sync State Infrastructure (STORY-0325)
- **Prompt**: Continue STORY-0325 - GitLab sync state tracking
- **Problem**: No way to track sync state between AIDA requirements and linked GitLab issues to detect changes
- **Solution**: Add comprehensive sync state tracking with database persistence and CLI visibility

**Implementation:**

1. **Data Model** (aida-core/src/models.rs):
   - `LinkOrigin` enum - How a link was created:
     - `CreatedFromAida` - Issue created from AIDA via GUI
     - `ImportedFromGitLab` - Issue imported from GitLab
     - `ManualLink` - User manually linked existing issue
   - `SyncStatus` enum - Current synchronization state:
     - `InSync` - Content matches between AIDA and GitLab
     - `AidaModified` - AIDA content changed since last sync
     - `GitLabModified` - GitLab content changed since last sync
     - `Conflict` - Both sides modified (needs resolution)
     - `Error` - Sync check failed
     - `Untracked` - Not yet tracked (manual links)
   - `GitLabSyncState` struct - Complete sync tracking record:
     - requirement_id, spec_id, gitlab_project_id, gitlab_issue_iid, gitlab_issue_id
     - linked_at, last_sync timestamps
     - aida_content_hash, gitlab_content_hash (SHA256)
     - link_origin, sync_status, last_error
   - Content hash functions:
     - `GitLabSyncState::hash_requirement()` - Hash req title/description/status/priority/owner/type/tags
     - `GitLabSyncState::hash_gitlab_issue()` - Hash issue title/description/state/labels/assignees

2. **Database Schema** (schema version 5):
   - SQLite (aida-core/src/db/schema.sql)
   - PostgreSQL (aida-core/src/db/postgres_schema.sql)
   - New `gitlab_sync_state` table:
     - Primary key: (requirement_id, gitlab_issue_iid)
     - Indexes for requirement lookup, issue lookup, status filtering

3. **Backend CRUD Operations**:
   - SQLite (aida-core/src/db/sqlite_backend.rs):
     - `save_sync_state()` - Upsert sync state record
     - `load_sync_state()` - Load by requirement_id + issue_iid
     - `load_sync_states_for_requirement()` - All states for a requirement
     - `load_all_sync_states()` - All sync states
     - `load_sync_states_by_status()` - Filter by status
     - `delete_sync_state()` - Remove sync state
   - PostgreSQL (aida-core/src/db/postgres_backend.rs):
     - Same operations with PostgreSQL-specific SQL

4. **Storage Layer** (aida-core/src/storage.rs):
   - Added sync state methods to `Storage` type
   - SQLite-only support (validates `is_sqlite()`)

5. **GUI Integration** (aida-gui/src/app.rs):
   - Manual link creation (`show_gitlab_link_picker_modal`):
     - Creates sync state with `LinkOrigin::ManualLink`
     - Status set to `Untracked` (hash unknown for existing issues)
   - Issue creation (`show_create_gitlab_issue_modal`):
     - Creates sync state with `LinkOrigin::CreatedFromAida`
     - Status set to `InSync` (just created, hashes match)
     - Computes both AIDA and GitLab content hashes

6. **CLI Command** (aida-cli/src/main.rs):
   - `aida gitlab status` - View sync state for all linked items
   - `aida gitlab status <SPEC-ID>` - View sync state for specific requirement
   - `aida gitlab status --diverged` - Show only non-InSync items
   - Output includes:
     - Status icon (✓/△/▽/⚠/✗/?)
     - Requirement spec_id
     - Link direction (→GL/←GL/↔GL)
     - Issue IID
     - Status text (colored)
     - Last sync timestamp
     - Error message (if any)
     - Summary counts

- **Commit**: 6b36f7b
- **Status**: STORY-0325 completed

---

## Session 12: GitLab Label Mapping & Polling (2025-12-14)

### GitLab Label Mapping Configuration (STORY-0326)
- **Prompt**: Implement STORY-0326 - GitLab Label Mapping Configuration
- **Problem**: No way to map AIDA requirement types, priorities, and statuses to GitLab labels
- **Solution**: Add label mapping configuration with CLI commands for validation and creation

**Implementation:**

1. **LabelConfig Helper Methods** (aida-core/src/integrations/gitlab/config.rs):
   - `get_type_label(req_type)` - Get GitLab label for requirement type
   - `get_priority_label(priority)` - Get GitLab label for priority
   - `get_status_label(status)` - Get GitLab label for status
   - `get_labels_for_requirement(type, priority, status)` - Get comma-separated labels
   - `with_defaults()` - Initialize with default mappings if empty
   - `all_labels()` - Get all unique label names from mappings

2. **GitLab Client Extension** (aida-core/src/integrations/gitlab/client.rs):
   - `create_label(name, color, description)` - Create label in GitLab project

3. **GUI Issue Creation** (aida-gui/src/app.rs):
   - Updated to use label mappings when creating issues
   - Auto-applies type, priority, and status labels

4. **CLI Commands** (aida-cli/src/cli.rs, aida-cli/src/main.rs):
   - `aida gitlab labels` - Show configured label mappings
   - `aida gitlab labels --validate` - Check which labels exist in GitLab
   - `aida gitlab labels --create-missing` - Create missing labels in GitLab
   - `aida gitlab labels --init` - Initialize with default label mappings

- **Commits**: b10c097, ead0cc6
- **Status**: STORY-0326 completed

### GitLab Polling for Changes (STORY-0327)
- **Prompt**: Continue with STORY-0327 - Poll GitLab for Changes
- **Problem**: No automatic detection of changes in linked GitLab issues
- **Solution**: Add background polling with UI status indicator

**Implementation:**

1. **CLI Commands** (aida-cli/src/main.rs):
   - `aida gitlab refresh [ID] [--force]` - Manually refresh sync state
   - `aida gitlab poll status` - Show current polling status
   - `aida gitlab poll start [--interval <secs>]` - Start background polling daemon
   - `aida gitlab poll stop` - Stop polling daemon

2. **GUI Background Polling** (aida-gui/src/app.rs):
   - `GitLabPollResult` struct - Poll result with counts and error
   - `poll_gitlab_for_changes()` async function:
     - Loads all sync states from storage
     - Fetches linked issues in single API call using IID filter
     - Computes current content hashes
     - Compares with stored hashes to detect changes
     - Updates sync status (InSync/AidaModified/GitLabModified/Conflict)
     - Saves updated sync states to database
   - Polling state fields in App struct:
     - `gitlab_polling_enabled` - Whether polling is active
     - `gitlab_last_poll` - Timestamp of last poll
     - `gitlab_poll_receiver` - Channel for poll results
     - `gitlab_poll_status` - Last status message
     - `gitlab_diverged_count` - Count of diverged items
   - In `update()`:
     - Checks for poll results via `try_recv()`
     - Triggers new poll when interval elapsed
     - Shows toast notification for diverged items

3. **Status Bar Indicator** (aida-gui/src/app.rs - show_top_panel):
   - `GL:○` gray - Not yet polled
   - `GL:🔄` yellow - Currently polling
   - `GL:✓` green - All items in sync
   - `GL:⚠` orange - Items have diverged
   - Tooltip shows detailed status and last poll time

- **Commits**: 3b55d47, 62828f5, 2c46afa
- **Status**: STORY-0327 completed

---

## Session 13: UI Enhancements (2025-12-19)

### Right-Click Context Menu for Requirements
- **Prompt**: Add right-click context menu on requirements showing same options as Actions menu
- **Problem**: Users had to use keyboard shortcut (Shift+A) to access AI actions on requirements
- **Solution**: Added right-click context menu that displays the same AI Actions popup

**Implementation:**

1. **State Field** (aida-gui/src/app.rs):
   - Added `context_menu_position: Option<egui::Pos2>` to track right-click location
   - When `Some(pos)`, menu positions at click location
   - When `None`, menu is centered (keyboard trigger)

2. **Right-Click Detection** (show_draggable_requirement):
   - Added `response.secondary_clicked()` handler
   - Selects the clicked requirement
   - Stores click position in `context_menu_position`
   - Opens the existing AI Actions menu

3. **Dynamic Positioning** (show_action_menu_popup):
   - Uses `context_menu_position` when set, otherwise centers
   - Clamps position to screen bounds to prevent off-screen menus
   - Clears position on menu close (click outside, action selected, ESC)

4. **Keyboard Trigger** (global input handling):
   - Sets `context_menu_position = None` when opening via Shift+A
   - Ensures centered positioning for keyboard access

- **Commit**: 06e0c64
- **Features**: Right-click any requirement to access AI actions at cursor position

### Resizable List/Detail Panel Divider
- **Prompt**: Add a slider/divider to resize the requirements list and details view
- **Problem**: Fixed 50/50 split between list and detail panels couldn't be adjusted
- **Solution**: Changed to resizable SidePanel layout

**Implementation:**

1. **State Field** (aida-gui/src/app.rs):
   - Added `list_panel_width: f32` to persist panel width during session
   - Default width: 400.0 pixels

2. **Layout Change** (ListDetailsSide mode):
   - Replaced `ui.columns(2, ...)` with `SidePanel::left()` + `CentralPanel`
   - SidePanel properties:
     - `min_width(200.0)` - minimum usable width
     - `default_width(self.list_panel_width)` - uses stored width
     - `max_width(screen_width * 0.7)` - max 70% of screen
     - `resizable(true)` - enables drag-to-resize
   - Panel width stored after each frame for persistence

3. **Right-Click Context Menu Fix**:
   - Switched to egui's built-in `context_menu()` pattern
   - Added to both `show_draggable_requirement` (flat view) and
     `show_draggable_requirement_inline` (tree view)

- **Commit**: 0ecd944
- **Features**: Drag the divider between list and detail panels to resize

---

## Session 14: META Requirements and Tree Export/Import (2025-12-22)

### Meta Requirement Type
- **Prompt**: Add new Meta requirement type for storing AI prompts, skills, and configuration
- **Problem**: AI prompts and templates were embedded in binary, not editable or browsable
- **Solution**: Added Meta type as new requirement category with MetaSubtype for categorization

**Implementation:**

1. **Core Types** (aida-core/src/models.rs):
   - Added `RequirementType::Meta` variant
   - Added `MetaSubtype` enum with variants: Prompt, Skill, Command, Template, Config
   - Added `meta_subtype: Option<MetaSubtype>` field to Requirement struct
   - Meta is stateless (like Folder) with prefix "META"

2. **Database Schemas**:
   - SQLite: Added `meta_subtype TEXT` column (schema v5→v6 migration)
   - PostgreSQL: Added `meta_subtype TEXT` column (schema v5→v6 migration)

3. **Proto/gRPC** (proto/aida.proto):
   - Added `REQUIREMENT_TYPE_META = 13` to RequirementType enum
   - Regenerated proto code for all components

4. **CLI/GUI/Server**:
   - Updated type parsing and display across all components
   - Added Meta emoji "⚡" for GUI display

### Tree Export/Import Feature
- **Prompt**: Add ability to export requirement trees to JSON and import into other databases
- **Problem**: No way to share requirement hierarchies between databases or create reusable templates
- **Solution**: Implemented recursive tree export/import with UUID/spec_id remapping

**Implementation:**

1. **Export Structures** (aida-core/src/export.rs):
   - `ExportedTree` - Root container with version, timestamp, source database
   - `ExportedRequirement` - Recursive structure preserving all fields and children
   - `ExternalRelRef` - Captures relationships to requirements outside the tree

2. **Import Structures**:
   - `TreeImportOptions` - Parent ID, conflict strategy, created_by
   - `ConflictStrategy` - Skip, Rename, or Replace on title collision
   - `TreeImportResult` - Import counts, UUID/spec_id mappings, unresolved refs

3. **Export Functions**:
   - `export_tree(store, root_id)` - Recursively exports requirement and descendants
   - `export_tree_to_file(store, root_id, path)` - Exports to JSON file

4. **Import Functions**:
   - `import_tree(store, tree, options)` - Recursively imports with new UUIDs
   - `import_tree_from_file(store, path, options)` - Imports from JSON file
   - UUID and spec_id remapping for all imported requirements
   - Parent-child relationships recreated with new IDs

5. **CLI Commands** (aida-cli):
   - `aida export --format tree --id <SPEC-ID> --output tree.json`
   - `aida import tree.json [--parent <SPEC-ID>] [--on-conflict skip|rename|replace]`

6. **GUI Dialogs** (aida-gui/src/app.rs):
   - Menu items: Menu > "🌳 Export Tree..." and "🌳 Import Tree..."
   - Export dialog: Searchable requirement picker, file save dialog
   - Import dialog: File picker, parent selection, conflict strategy options

**Use Cases:**
- Export META folder with all prompts and import into new database
- Share requirement templates between projects
- Backup/restore requirement hierarchies
- Create reusable requirement libraries

- **Commits**: (pending)
- **Status**: Core implementation complete, meta seeding and prompt fallback pending

### Meta Seeding and Prompt Fallback
- **Prompt**: Implement meta seeding for new databases and prompt fallback to check database first
- **Problem**: AI prompts were embedded and not editable; needed to store them as browsable requirements
- **Solution**: Created `aida-core/src/meta.rs` module with seeding and fallback functions

**Implementation:**

1. **Meta Module** (aida-core/src/meta.rs):
   - Default prompt templates as const strings:
     - `DEFAULT_EVALUATION_PROMPT`
     - `DEFAULT_DUPLICATES_PROMPT`
     - `DEFAULT_RELATIONSHIPS_PROMPT`
     - `DEFAULT_IMPROVE_PROMPT`
     - `DEFAULT_GENERATE_CHILDREN_PROMPT`
   - `get_prompt_template(store, name)` - checks database for META prompt, falls back to embedded
   - `seed_meta_requirements(store)` - creates META-PROMPTS folder with default templates
   - `needs_meta_seeding(store)` - checks if seeding is needed

2. **Prompt Fallback** (aida-core/src/ai/prompts.rs):
   - Modified all prompt building functions to use `get_prompt_template()`
   - Priority order:
     1. Custom template in `store.ai_prompts` configuration
     2. META requirement in database matching prompt name
     3. Embedded default template
   - Prompt names matched: "Evaluate Requirement", "Find Duplicates", "Suggest Relationships", "Improve Description", "Generate Children"

3. **Exports** (aida-core/src/lib.rs):
   - Added `meta` module
   - Exported: `get_prompt_template`, `needs_meta_seeding`, `seed_meta_requirements`
   - Exported default prompt constants

**Usage:**
- Call `seed_meta_requirements(&mut store)` after creating a new database
- Edit META requirements in GUI/CLI to customize prompts
- Changes take effect immediately without code changes

- **Commit**: (pending)
- **Features**: Database-stored customizable AI prompts, seeding for new databases


### Documentation Update - Use Cases and Tutorials
- **Prompt**: Make sure all documentation is up-to-date with step-by-step use cases explained in detail
- **Problem**: Documentation lacked detailed use case examples for new features (Meta requirements, Tree export/import, GitLab integration)
- **Solution**: Added comprehensive use case section to OVERVIEW.md and updated user guide

**Changes:**

1. **OVERVIEW.md** - Added "Use Cases & Tutorials" section with 6 detailed scenarios:
   - Use Case 1: Sharing Requirement Templates Between Projects (tree export/import)
   - Use Case 2: Customizing AI Prompts for Domain-Specific Evaluation (meta requirements)
   - Use Case 3: Setting Up a New Project with Meta Seeding
   - Use Case 4: Migrating Requirements Between Storage Backends
   - Use Case 5: GitLab Integration Workflow
   - Use Case 6: Building a Reusable Requirements Library

2. **docs/user-guide.md** - Added new sections:
   - Meta Requirements section with subtypes, default prompts, customization guide, placeholders
   - Tree Export/Import section with CLI and GUI instructions
   - Updated Table of Contents with new sections

3. **CLAUDE.md** - Added new features:
   - Added `meta` type to Organizational types
   - Added "Meta Requirements and AI Prompt Customization" section
   - Added "Tree Export/Import" section with CLI commands

- **Commit**: (pending)
- **Status**: Documentation complete


### Templates View Implementation (FR-0357)
- **Prompt**: "I want a view in aida what will show all the meta data, hooks, skills, prompts etc. I think we should store this information encoded in aida via build.rs so that we can bootstrap new projects from just the binary."
- **Problem**: No way to browse embedded templates from the GUI; users couldn't see what skills/commands/hooks were available
- **Solution**: Created Templates view in GUI with category navigation and content preview

**Implementation:**

1. **build.rs Enhancements**:
   - Added `embed_file()` function for single files (e.g., settings.json)
   - Added `TEMPLATE_CATEGORIES` static for introspection
   - Embedded settings.json for Claude Code configuration

2. **templates.rs New Functions**:
   - `TemplateInfo` struct with key, category, name, content, source
   - `TemplateSource` enum: ProjectLocal, UserConfig, Embedded, NotFound
   - `get_embedded_templates()` - returns all templates with metadata
   - `get_templates_by_category()` - filter by category
   - `get_template_categories()` - list categories with descriptions

3. **lib.rs Exports**:
   - Exported TemplateInfo, TemplateLoader, TemplateSource
   - Exported helper functions for templates

4. **GUI Templates View** (aida-gui/src/app.rs):
   - Added `View::Templates` to View enum
   - Added menu item "📄 Templates" in View menu
   - Added 'm' keyboard shortcut for Templates view
   - Two-column layout: categories/list on left, preview on right
   - Category tabs: skills, commands, hooks, settings.json
   - Template list with source icons (📦 Embedded, 📁 Project, 👤 User)
   - Content preview in scrollable monospace area

5. **Keyboard Navigation**:
   - j/k and arrow keys for template list navigation
   - Home/End to jump to first/last template
   - Navigation hint in header

- **Commits**: c076e0c, 340dc25
- **Status**: Complete
- **Requirement**: FR-0357 (Templates View - Browse embedded skills, commands, hooks)


### Resizable Panels and Template Naming Fixes (2025-12-31)
- **Prompt**: "In the requirements view, we have a slider between the list and the details view. We should have the same slider in the Timeline, My Queue, Other Queue, Templates views"
- **Problem**: Queue and Templates views lacked the resizable divider between list and detail panels
- **Solution**: Added resizable SidePanels to Queue, UserQueue, and Templates views

**Changes:**

1. **State Variables** (aida-gui/src/app.rs):
   - Added `queue_panel_width: f32` for Queue/UserQueue views
   - Added `templates_panel_width: f32` for Templates view

2. **Queue Views**:
   - Refactored Queue and UserQueue to use `egui::SidePanel` with `.resizable(true)`
   - List panel shows on left, details on right with draggable divider

3. **Templates View Refactoring**:
   - Split rendering into `show_templates_list_panel()` and `show_templates_preview_panel()`
   - Added resizable SidePanel pattern matching other views

4. **Timeline View** (reverted):
   - Initially tried adding SidePanel but broke internal columns layout
   - Timeline already has `ui.columns(2, ...)` for event list + detail
   - Reverted to CentralPanel to preserve existing behavior

- **Commit**: 679bf06
- **Status**: Complete


### Template File Renaming
- **Prompt**: "In Templates, I see review.md and status.md. Should these be prefixed like aida-review.md so that they conform to our naming convention?"
- **Problem**: Template files didn't follow aida-* naming convention
- **Solution**: Renamed template files and updated symlinks

**Files Renamed:**
- `templates/commands/review.md` → `aida-review.md`
- `templates/commands/status.md` → `aida-status.md`
- `templates/hooks/commit-msg` → `aida-commit-msg`

**Symlinks Updated:**
- `.claude/commands/aida-review.md`
- `.claude/commands/aida-status.md`
- `.git/hooks/commit-msg` → `aida-commit-msg`

- **Commit**: (included in 679bf06)
- **Status**: Complete


### Templates View Keyboard Navigation Fix
- **Prompt**: "In Templates view the navigation keys automatically switch us back to requirements view"
- **Problem**: j/k and arrow keys in Templates view were intercepted by general list navigation code, causing requirements list navigation instead of template list navigation
- **Root Cause**: General navigation code at line ~32172 excluded Timeline, Planning, KanBan, Queue but not Templates
- **Solution**: Added `in_templates` check to keyboard navigation exclusion conditions

**Changes** (aida-gui/src/app.rs):
```rust
#[cfg(not(target_arch = "wasm32"))]
let in_templates = self.current_view == View::Templates;
#[cfg(target_arch = "wasm32")]
let in_templates = false;
if !in_timeline && !in_planning && !in_kanban && !in_queue && !in_templates && can_navigate
```

- **Commit**: a5c1431
- **Status**: Complete


### Multi-Project Support (2025-01-02)
- **Prompt**: "I would like to be able to support multiple projects on aida.joemooney.com, how could we go about doing that?"
- **Problem**: AIDA only supported a single project/database, limiting multi-tenant use
- **Solution**: Implemented multi-project support with per-project SQLite databases

**Architecture:**
- Each project gets its own isolated SQLite database file
- Server uses `x-project` header to route requests to correct database
- Client passes `?project=name` URL parameter
- Project selector UI shown when no project is selected (WASM only)

**Server Changes (aida-server):**

1. **ProjectManager** (src/projects.rs - NEW):
   - Manages multiple isolated SQLite databases
   - Lazy loading of database backends
   - Project registry stored in `projects.json`
   - Methods: `list_projects()`, `create_project()`, `delete_project()`, `get_backend()`
   - Automatic migration of legacy requirements.db to "default" project

2. **REST Endpoints** (src/rest.rs):
   - `GET /api/projects` - List all projects
   - `POST /api/projects` - Create new project
   - `GET /api/projects/:name` - Get project info
   - `DELETE /api/projects/:name` - Delete project
   - All requirement endpoints require `X-Project` header

3. **gRPC Multi-Project Service** (src/service.rs):
   - Added `AidaServiceMultiProject` struct
   - Extracts `x-project` from gRPC metadata for every request
   - Routes to correct backend via ProjectManager

4. **Main Entry Point** (src/main.rs):
   - Added `--data-dir` argument for multi-project mode
   - Detects mode: multi-project (default) or single-project (--database)
   - Default data_dir: `/data` (Docker) or `~/.aida` (local)

**Client Changes (aida-gui):**

1. **URL Parameter Parsing** (src/lib.rs):
   - Parse `?project=name` from URL query parameters

2. **gRPC Header Support** (src/storage/grpc_client.rs):
   - Added `project` field to GrpcStorageClient
   - `connect_with_project()` method for project-aware connections
   - `make_request()` helper adds `x-project` metadata to all requests

3. **Project Selector UI** (src/app.rs):
   - `ProjectInfo` struct for parsed server responses
   - State fields: `wasm_projects`, `wasm_projects_error`, `wasm_projects_loading`
   - `show_project_selector()` - Full-screen project selector UI
   - `fetch_projects()` - REST API call to list projects
   - `create_project()` - REST API call to create new project
   - `navigate_to_project()` - Updates URL with `?project=name`

**Dependencies Added:**
- `aida-gui/Cargo.toml`: serde-wasm-bindgen, web-sys features (Headers, Request, etc.)
- `aida-server/Cargo.toml`: dirs crate for home directory resolution

- **Commit**: 605967f
- **Status**: Complete
- **Requirements**: FR-0227 (Multi-Project Support)


### Multi-Project Deployment Fixes (2026-01-03)
- **Prompt**: Fix deployment issues for multi-project support on aida.joemooney.com
- **Problems**: Multiple issues discovered during production deployment

**Issue 1: Traefik Network Routing (504 Gateway Timeout)**
- **Root Cause**: aida-server is on two networks (proxy + internal). Traefik was picking the `internal` network IP (alphabetically first) which it couldn't reach
- **Solution**: Added `traefik.docker.network=proxy` label to aida-server in docker-compose.yml
- **Commit**: 6b68874

**Issue 2: JSON Deserialization (expected a sequence)**
- **Root Cause**: REST API returns `{"projects": [...]}` wrapper, but WASM was deserializing as `Vec<ProjectInfo>` directly
- **Solution**: Added `ProjectsResponse` wrapper struct to handle the response format
- **Commit**: 66fcc91

**Issue 3: Field Name Mismatch (missing field 'created_at')**
- **Root Cause**: API returns camelCase (`createdAt`) but struct used snake_case (`created_at`)
- **Solution**: Added `#[serde(rename_all = "camelCase")]` to ProjectInfo struct
- **Commit**: 0b1982c

**Docker Compose Changes:**
```yaml
aida-server:
  labels:
    - traefik.docker.network=proxy  # NEW - ensures correct network routing
```

**Client Changes (aida-gui/src/app.rs):**
```rust
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]  // Handle API camelCase
pub struct ProjectInfo { ... }

#[derive(Debug, Deserialize)]
struct ProjectsResponse {  // Wrapper for API response
    projects: Vec<ProjectInfo>,
}
```

- **Status**: Complete - Multi-project UI working at aida.joemooney.com

---

## Session: Scaffolding Modernization (5-Phase Plan)

**Date**: 2026-02-15
**Branch**: `feature/modernize-web-frontend`

### Overview
Complete modernization of the AIDA scaffolding system across 5 phases, transforming skill templates from plain markdown to Claude Code-compatible skills with frontmatter, dynamic context, and MCP integration.

### Phase 1: Foundation — Consolidate Templates + Add Frontmatter
- **Prompt**: Review scaffolding system and create improvement plan
- **Actions**:
  - Migrated 6 inline skill string literals from `scaffolding.rs` to load from `EMBEDDED_TEMPLATES`
  - Added YAML frontmatter to all 9 skills (`name`, `description`, `allowed-tools`, `disable-model-invocation`)
  - Split `scaffolding.rs` (~3,000 lines) into modules: `mod.rs`, `claude_md.rs`, `hooks.rs`, `settings.rs`
  - Sorted `build.rs` template embedding for deterministic builds
  - Bumped SCAFFOLD_VERSION to 1.1.0
- **Commit**: 761d716

### Phase 2: Skill Quality + Dynamic Context Injection
- **Actions**:
  - Added `!`command`` dynamic context injection to key skills (aida-req, aida-implement, aida-capture, aida-commit, aida-status)
  - Trimmed verbose skills: aida-release (329→165 lines), aida-plan (210→138 lines), aida-sync (239→155 lines)
  - All commands use `2>/dev/null || echo "fallback"` for graceful degradation
- **Commit**: c382f26, 9b52475

### Phase 3: New Skills + Enhanced Hooks
- **Actions**:
  - Created 6 new skill templates with frontmatter:
    - `aida-test.md` — Generate tests linked to requirements
    - `aida-review.md` — Review code changes against specs
    - `aida-onboard.md` — Project onboarding for new team members
    - `aida-sprint.md` — Sprint planning from approved requirements
    - `aida-search.md` — Unified search across requirements and code
    - `aida-standup.md` — Generate daily standup reports
  - Created 5 matching command wrappers in `templates/commands/`
  - Created 2 new hooks: `aida-stop-check.sh` (untraced edit warnings), `aida-session-context.sh` (session context injection)
  - Registered all new skills in `scaffolding/mod.rs` with `ScaffoldConfig` boolean fields
  - Expanded CLAUDE.md generation to document all 15 skills by category
  - Added `.mcp.json` generation to scaffolding
  - Bumped SCAFFOLD_VERSION to 1.2.0
- **Commits**: 2c20052, 3d332a6

### Phase 4: MCP Server (`aida mcp-serve`)
- **Actions**:
  - Created `aida-cli/src/mcp.rs` (~500 lines) — full MCP server over stdio
  - JSON-RPC 2.0 protocol: initialize, tools/list, tools/call, resources/list, resources/read
  - 7 tools: list_requirements, show_requirement, add_requirement, update_requirement, search_requirements, add_comment, list_features
  - 2 resources: aida://project/summary, aida://requirements/tree
  - Added `McpServe` CLI subcommand
  - Added serde/serde_json dependencies to aida-cli
  - Tested: MCP handshake, tool listing, search tool (returned 16 results from live DB)
- **Commit**: eb533a1

### Phase 5: Organization Template Layer + Advanced Features
- **Actions**:
  - Extended `TemplateLoader` with 4-tier priority: project → organization → user → embedded
  - Added `Organization(PathBuf)` variant to `TemplateSource` enum
  - Updated all template lookup methods for org tier (`~/.config/aida/org-templates/`)
  - Added organization source icon in GUI
  - Fixed aida-web `UrlLink` missing `open_mode` field
  - Bumped SCAFFOLD_VERSION to 2.0.0
- **Commit**: b49b5c8

### Ancillary Changes
- Added `ts-rs` derive macros to ~40 structs/enums for TypeScript type generation
- Committed all new `.claude/` symlinks for Phase 3 skills/commands
- **Commits**: d6f4ff7, 0761e29

### Key Technical Details
- **Template architecture**: `aida-core/templates/` → embedded via `build.rs` → loaded by `EMBEDDED_TEMPLATES` HashMap → scaffolded to `.claude/` as symlinks
- **MCP protocol**: Minimal implementation using only serde_json (no heavy framework), reads JSON lines from stdin, writes to stdout
- **Frontmatter format**: `---` delimited YAML with `name`, `description`, `allowed-tools[]`, optional `disable-model-invocation: true`
- **Dynamic context**: Executed by Claude Code at skill load time, provides live project data without bloating templates

---

## Session 22: AIDA React Dashboard (2026-02-20)

### Prompt
User requested implementation of a full React dashboard for AIDA, replacing/complementing the egui-based WASM browser client with a modern React SPA.

### Implementation Plan
9-phase implementation:
1. **Foundation** — Vite + React 19 + Tailwind CSS 4 project setup, Vite dev proxy to REST API
2. **UI Primitives** — Reusable component library (Badge, Button, Card, Input, Select, StatusBadge)
3. **Layout** — App shell with sidebar navigation, header, dark/light theme toggle
4. **Data Layer** — @tanstack/react-query hooks for requirements CRUD, API client module
5. **Kanban Board** — Drag-and-drop board with @dnd-kit, columns per status, optimistic updates
6. **Dashboard** — Metrics cards, charts by status/priority/type, recent activity
7. **List View** — Sortable/filterable table with inline status badges
8. **Detail Panel** — Requirement detail view with editing, comments, relationships
9. **Polish** — URL-based state management, search, responsive layout, production build verification

### Actions Taken
- Created `aida-web-react/` with React 19, Vite 8, Tailwind CSS 4, @tanstack/react-query, react-router-dom, @dnd-kit, lucide-react, clsx
- Created `shared/types.ts` with TypeScript types generated from Rust structs via ts-rs
- Built 35 source files organized into:
  - `api/` — REST API client with fetch wrapper
  - `lib/` — Utility functions, cn() helper
  - `hooks/` — React Query hooks for requirements data
  - `components/ui/` — Badge, Button, Card, Input, Select, StatusBadge primitives
  - `components/layout/` — AppLayout, Sidebar, Header, ThemeProvider
  - `components/kanban/` — KanbanBoard, KanbanColumn, KanbanCard
  - `components/list/` — RequirementList, RequirementRow
  - `components/detail/` — RequirementDetail panel
  - `components/dashboard/` — MetricsCards, StatusChart, PriorityChart
- 52 files committed total (7,240 lines of code)
- TypeScript passes clean, production build succeeds
- Port registry updated: 5173 (React dev server), 8080 (REST API)

### Key Design Choices
- **Theming**: CSS custom properties for dark/light mode, toggled via ThemeProvider context
- **State management**: URL-based filter and detail state (query params and route params) for shareability
- **Drag-and-drop**: Optimistic updates on Kanban card moves, with rollback on API failure
- **Type safety**: Shared TypeScript types generated from Rust structs ensure API contract consistency

### Commit
- **8f3be09** — feat(web-react): implement AIDA React Dashboard

---

## Session: Sprint View with Planning & Backlog

### Prompt
Implement Sprint Planning view with drag-and-drop between backlog and sprints.

### Actions Taken

**Phase 1: REST API Endpoints (Rust)**
- Added `PUT /api/v2/requirements/:id/sprint` — assign requirement to a sprint
- Added `DELETE /api/v2/requirements/:id/sprint` — remove from sprint (back to backlog)
- Created `SprintAssignRequest` struct with `sprint_id` and optional `username`
- Handlers validate target is a Sprint type, call `store.assign_to_sprint()` / `store.remove_from_sprint()`
- File modified: `aida-server/src/rest.rs`

**Phase 2: TypeScript API, Hooks, and Utils**
- Created `src/api/sprints.ts` — `assignToSprint()` and `removeFromSprint()` API functions
- Created `src/hooks/useSprints.ts` — `useAssignToSprint()` and `useRemoveFromSprint()` mutation hooks
- Created `src/lib/sprint-utils.ts` — Sprint utility functions:
  - `isSprintAssignment()` — type-checks `{ Custom: "sprint_assignment" }` relationship
  - `getSprintNumber()`, `getSprintGoal()`, `getSprintDates()` — custom field accessors
  - `getSprintState()` — returns `'active' | 'past' | 'future' | 'unknown'` based on dates
  - `computeSprintProgress()` — calculates completion percentage and story points
  - `getSprintAssignmentTarget()` — extracts sprint UUID from requirement relationships

**Phase 3: Sprint UI Components (7 files)**
- `SprintView.tsx` — Main page component at `/sprints` route, derives sprints/backlog from `useRequirements()`, DnD context
- `SprintSelector.tsx` — Horizontal scrollable strip of sprint cards at top
- `SprintCard.tsx` — Sprint card showing title, dates, state badge, progress bar
- `SprintBoard.tsx` — Two-column layout (backlog + sprint items)
- `SprintColumn.tsx` — Droppable column with header showing item count and story points
- `SprintItemCard.tsx` — Draggable requirement card with spec_id, priority, type badge, story points
- `SprintProgressBar.tsx` — Reusable progress bar with color-coded fill

**Phase 4: Integration**
- Modified `App.tsx` — added `/sprints` route
- Modified `Sidebar.tsx` — added Sprints nav item with Zap icon

**DnD Logic**
- Drag from backlog → sprint column: calls `assignToSprint(reqId, sprintId)`
- Drag from sprint → backlog: calls `removeFromSprint(reqId)`
- Same column drop: no-op
- Both mutations invalidate `['requirements']` query key

### Build Verification
- `cargo build -p aida-server` — compiled successfully (no new warnings)
- `npm run build` — production build succeeded (357KB JS, 31KB CSS)

### Files Changed
- 3 modified: `rest.rs`, `App.tsx`, `Sidebar.tsx`
- 10 created: `sprints.ts`, `useSprints.ts`, `sprint-utils.ts`, 7 sprint components

### Commit
- **b0e5025** — [AI:claude] feat(web): add sprint view with planning and backlog management

---

## Sprint Enhancements: Create, Archive, and Charts (2026-02-20)

### Prompt
Implement sprint create modal, archive sprint toggle, and burndown/burn-up/velocity charts using pure SVG.

### Plan
Saved to `docs/plans/2026-02-20-sprint-enhancements.md`

### Phase 1: Extend V2 API (Rust)
- Added `custom_fields` and `archived` to `UpdateRequirementV2Request` in `rest.rs`
- Added `CreateRequirementV2Request` struct and `create_requirement_v2_legacy` handler
- Registered `POST /api/v2/requirements` route in legacy router

### Phase 2: Sprint API & Hooks (TypeScript)
- Added `createSprint()` API function and `CreateSprintData` interface
- Added `useCreateSprint()` mutation hook

### Phase 3: Create Sprint Modal
- Created `CreateSprintModal.tsx` — modal with sprint number, title, start/end dates, goal, planned velocity
- Auto-suggests next sprint number, auto-generates "Sprint N" title

### Phase 4: Archive Sprint
- Added archive button (hover-visible `Archive` icon) to `SprintCard.tsx`
- Passed `onArchive` through `SprintSelector.tsx`
- Added "Show archived" toggle button in `SprintView.tsx` header

### Phase 5: Sprint Charts (Pure SVG)
- `BurndownChart.tsx` — ideal vs actual remaining items line chart
- `BurnupChart.tsx` — scope vs cumulative completed with area fill
- `VelocityChart.tsx` — bar chart of completed points per sprint with average line
- `SprintCharts.tsx` — container rendering all three in responsive 3-column grid
- Added `computeBurndownData`, `computeBurnupData`, `computeVelocityData` utilities to `sprint-utils.ts`

### Phase 6: Integration
- Updated `SprintView.tsx` with "New Sprint" button, archive toggle, charts below board
- Both `cargo build -p aida-server` and `npm run build` pass

### Files Changed
- 7 modified, 5 created (12 files total, 836 insertions)

### Commit
- **8f28d2a** — [AI:claude] feat(web): add sprint create, archive, and charts

---

## Plan Archival Feature (2026-02-20)

### Prompt
Add instruction to CLAUDE.md for storing implementation plans in `docs/plans/` with related requirement IDs.

### Actions
- Added "Plan Archival" section to CLAUDE.md under Session Workflow
- Created `docs/plans/` directory
- Saved the sprint enhancements plan as `docs/plans/2026-02-20-sprint-enhancements.md`

---

## Skills Browser View (2026-02-20)

### Prompt
Implement a Skills browser view in the web dashboard: REST API endpoints to list/view/edit skills and commands, plus a React UI with card grid, detail panel, and edit mode.

### Actions

#### Phase 1: Backend — Skills API endpoints
- Added 3 new endpoints to `aida-server/src/rest.rs`:
  - `GET /api/v2/skills` — lists all skills + commands with name, description, kind
  - `GET /api/v2/skills/:name` — returns full content, allowed_tools, frontmatter
  - `PUT /api/v2/skills/:name` — updates content (writes to symlink target via canonicalize)
- Implemented YAML frontmatter parser for skill files (name, description, allowed-tools)
- Scans `.claude/skills/*.md` and `.claude/commands/*.md` relative to CWD

#### Phase 2: Frontend API + Hooks
- Created `aida-web-react/src/api/skills.ts` with `fetchSkills`, `fetchSkill`, `updateSkill`
- Created `aida-web-react/src/hooks/useSkills.ts` with `useSkills`, `useSkill`, `useUpdateSkill`

#### Phase 3: Skills View Components
- `SkillsView.tsx` — top-level view with header, filter toggle (All/Skills/Commands), responsive grid
- `SkillCard.tsx` — card with name, description, kind badge; click opens detail panel
- `SkillDetailPanel.tsx` — slide-in panel with view/edit modes, tool badges, save button

#### Phase 4: Routing + Sidebar
- Added Sparkles icon + "Skills" nav item to `Sidebar.tsx`
- Added `/skills` route to `App.tsx`

### Files Changed
- 3 modified (`rest.rs`, `Sidebar.tsx`, `App.tsx`)
- 5 created (`api/skills.ts`, `hooks/useSkills.ts`, `SkillsView.tsx`, `SkillCard.tsx`, `SkillDetailPanel.tsx`)

---

## Docs Browser View (2026-02-20)

### Prompt
Add a "Docs" page to the React dashboard to browse and read markdown files from the `docs/` directory (including `docs/plans/`), with read-only markdown rendering via react-markdown.

### Actions

#### Phase 1: Backend — Docs API endpoints
- Added 2 new endpoints to `aida-server/src/rest.rs`:
  - `GET /api/v2/docs` — recursively lists all `.md` files from `docs/` with title, section, path
  - `GET /api/v2/docs/*path` — returns full markdown content by relative path (supports nested paths)
- Title extraction from first `# heading` line
- Section classification: files in `docs/plans/` → "plans", others → "docs"
- Path traversal protection via canonicalization

#### Phase 2: Frontend API + Hooks
- Created `aida-web-react/src/api/docs.ts` with `fetchDocs()` and `fetchDoc(path)`
- Created `aida-web-react/src/hooks/useDocs.ts` with `useDocs()` and `useDoc(path)`

#### Phase 3: Docs View Components
- `DocsView.tsx` — top-level view with header, filter toggle (All/Docs/Plans), search, grouped sections
- `DocCard.tsx` — card with title, section badge, file path; click opens detail panel
- `DocDetailPanel.tsx` — slide-in panel (max-w-3xl) with read-only rendered markdown

#### Phase 4: Routing + Sidebar
- Added FileText icon + "Docs" nav item to `Sidebar.tsx`
- Added `/docs` route to `App.tsx`

### Files Changed
- 3 modified (`rest.rs`, `Sidebar.tsx`, `App.tsx`)
- 5 created (`api/docs.ts`, `hooks/useDocs.ts`, `DocsView.tsx`, `DocCard.tsx`, `DocDetailPanel.tsx`)
- 1 created (`docs/plans/2026-02-20-docs-browser.md`)

---

### Sprint Edit, Close, and Carry-Over
- **Prompt**: Implement sprint edit, close, and carry-over functionality
- **Date**: 2026-02-20

#### Phase 1: EditSprintModal
- Created `aida-web-react/src/components/sprint/EditSprintModal.tsx`
- Cloned CreateSprintModal pattern, pre-populates fields from `sprint.custom_fields`
- Uses `useUpdateRequirement()` to save changes (title + custom_fields)

#### Phase 2: CloseSprintModal
- Created `aida-web-react/src/components/sprint/CloseSprintModal.tsx`
- Summary section with sprint name, dates, progress bar
- Checkbox list of incomplete items (all checked by default)
- "Close Sprint" — sets sprint status to Completed
- "Close & Create Next" — closes sprint, creates next sprint (number+1, dates = day after end + 2 weeks), moves checked items via `assignToSprint`
- Sequential `mutateAsync` calls with error handling at each step

#### Phase 3: SprintCard Action Buttons
- Modified `SprintCard.tsx` — added pencil (edit) and check-circle (close) icons alongside archive
- All three buttons appear on hover in a row, hidden for archived sprints
- Close button hidden for past sprints

#### Phase 4: Wiring in SprintSelector + SprintView
- `SprintSelector.tsx` — passes `onEdit` and `onClose` through to SprintCard
- `SprintView.tsx` — added `editingSprint` and `closingSprint` state, handlers to find sprint by id, renders modals conditionally

### Files Changed
- 3 modified (`SprintCard.tsx`, `SprintSelector.tsx`, `SprintView.tsx`)
- 2 created (`EditSprintModal.tsx`, `CloseSprintModal.tsx`)

---

## Parent/Child Tree Toggle on List View (2026-02-20)

### Prompt
Add a List/Tree toggle to the web List View so requirements can be viewed as a flat table (default) or as an indented, collapsible parent/child tree based on Parent relationships.

### Actions
- **Date**: 2026-02-20

#### Phase 1: Tree Utility Functions
- Created `aida-web-react/src/lib/tree-utils.ts`
- `TreeNode` type with requirement, children array, and depth
- `buildTree()` — scans relationships for `rel_type === "Parent"` to find each item's parent `target_id`, builds parent→children map, returns sorted root nodes
- `flattenTree()` — returns flat list of visible rows, respects collapsed set, computes ancestor-only dimming when filters active
- `collectParentIds()` — helper for expand/collapse all buttons

#### Phase 2: TreeRow Component
- Created `aida-web-react/src/components/list/TreeRow.tsx`
- Mirrors `RequirementsRow` cell layout (ID, title, status, priority, type, owner, modified)
- Title cell indented by `depth * 20px`
- ChevronRight/ChevronDown toggle for nodes with children
- `opacity-50` for dimmed ancestor-only context nodes
- Click opens detail panel via `useDetailPanel().open()`

#### Phase 3: RequirementsList Modifications
- Modified `aida-web-react/src/components/list/RequirementsList.tsx`
- Added `viewMode: 'flat' | 'tree'` state (default flat)
- Added `collapsed: Set<string>` state for tree collapse tracking
- Toggle button group (List icon / GitBranch icon) in header
- Expand all / Collapse all buttons (ChevronsUpDown / ChevronsDownUp) in tree mode
- Column sorting disabled in tree mode (headers not clickable)
- Filters work in both modes; tree mode shows ancestor nodes dimmed for context

### Files Changed
- 1 modified (`RequirementsList.tsx`)
- 2 created (`tree-utils.ts`, `TreeRow.tsx`)
- 1 plan saved (`docs/plans/2026-02-20-list-tree-toggle.md`)

### Timeline View for Web Dashboard
- **Prompt**: Add a Timeline view showing chronological event feed from requirement history, comments, and creation events
- **Actions**:
  - Created `aida-web-react/src/lib/timeline-utils.ts` — utility functions to build, filter, and group timeline events from requirements data
  - Created `aida-web-react/src/components/timeline/TimelineEventCard.tsx` — single event row with icon, time, spec ID, title, and author avatar
  - Created `aida-web-react/src/components/timeline/TimelineDateGroup.tsx` — sticky date header with grouped event cards
  - Created `aida-web-react/src/components/timeline/TimelineDetailPanel.tsx` — right-column detail showing event info, field change diffs, and comment content
  - Created `aida-web-react/src/components/timeline/TimelineFilterBar.tsx` — author and field text filters with event count and clear button
  - Created `aida-web-react/src/components/timeline/TimelineView.tsx` — top-level two-column layout with scrollable event list and detail panel
  - Modified `App.tsx` to add `/timeline` route
  - Modified `Sidebar.tsx` to add Timeline nav item with Clock icon between Sprints and Skills
  - All data built client-side from existing `useRequirements()` hook — no backend changes
- **Git**: `a33cac0` — `[AI:claude] feat(web): add Timeline view with chronological event feed`

### Files Changed
- 2 modified (`App.tsx`, `Sidebar.tsx`)
- 6 created (`timeline-utils.ts`, `TimelineEventCard.tsx`, `TimelineDateGroup.tsx`, `TimelineDetailPanel.tsx`, `TimelineFilterBar.tsx`, `TimelineView.tsx`)

### Settings View for Web Dashboard
- **Prompt**: Implement Settings view with backend CRUD endpoints and frontend tab-based UI for managing store metadata, relationship definitions, type definitions, reaction definitions, ID configuration, and prefix management
- **Actions**:
  - **Backend**: Added 15 REST endpoint handlers in `aida-server/src/rest.rs` under `/api/v2/settings/...`:
    - Metadata: GET/PUT at `/api/v2/settings/metadata`
    - Relationship definitions: GET/POST at `/api/v2/settings/relationship-definitions`, PUT/DELETE at `/:name`
    - Type definitions: GET/POST at `/api/v2/settings/type-definitions`, PUT/DELETE at `/:name`
    - Reaction definitions: GET/POST at `/api/v2/settings/reaction-definitions`, PUT/DELETE at `/:name`
    - ID config: GET/PUT at `/api/v2/settings/id-config`
    - Prefixes: GET/PUT at `/api/v2/settings/prefixes`
    - Uses existing `ServerState` pattern with built-in protection validation
  - **Frontend API**: Created `aida-web-react/src/api/settings.ts` with all CRUD functions
  - **Frontend Hooks**: Created `aida-web-react/src/hooks/useSettings.ts` with `useQuery`/`useMutation` hooks
  - **Frontend Components**: Created 8 components in `aida-web-react/src/components/settings/`:
    - `SettingsView.tsx` — Tab-based layout (General, Relationships, Types, Reactions, IDs & Prefixes)
    - `GeneralTab.tsx` — Store name/title/description form
    - `RelationshipsTab.tsx` — Table with add/edit/delete, built-in badge
    - `RelationshipForm.tsx` — Modal form for relationship CRUD
    - `TypesTab.tsx` — Table with add/edit/delete, field count, color indicator
    - `TypeForm.tsx` — Modal form with statuses/priorities tag lists, custom fields section
    - `ReactionsTab.tsx` — Card grid with emoji display, hover actions
    - `ReactionForm.tsx` — Simple modal for reaction CRUD
    - `IdsTab.tsx` — ID format/numbering/digits config + prefix management
  - Modified `App.tsx` to add `/settings` route
  - Modified `Sidebar.tsx` to add Settings nav item with gear icon
  - Saved plan to `docs/plans/2026-02-20-settings-view.md`

### Files Changed
- 3 modified (`aida-server/src/rest.rs`, `App.tsx`, `Sidebar.tsx`)
- 11 created (`api/settings.ts`, `hooks/useSettings.ts`, `SettingsView.tsx`, `GeneralTab.tsx`, `RelationshipsTab.tsx`, `RelationshipForm.tsx`, `TypesTab.tsx`, `TypeForm.tsx`, `ReactionsTab.tsx`, `ReactionForm.tsx`, `IdsTab.tsx`)

---

## Tag Filtering, Structured Search, and Markdown Descriptions (2026-02-20)

### Prompt
Add tag filtering to list/kanban/timeline views, structured search parsing (field:value syntax) in the search bar, and markdown rendering for requirement descriptions.

### Plan
Saved to `docs/plans/2026-02-20-tag-filtering-structured-search-markdown.md`

### Actions

#### Phase 1: Tag Filter in useFilters Hook
- Added `tag: string` to `Filters` interface
- Read `tag` from URL search params
- Added tag matching in `applyFilters`: checks `req.tags` array includes filter value
- Added `tag` to `clearFilters` deletions
- Added `removeFilter(key)` function to delete a single filter from URL params
- File modified: `aida-web-react/src/hooks/useFilters.ts`

#### Phase 2: Filter Bar UI Enhancements
- Extracted unique tags from requirements: `flatMap(r => r.tags ?? [])` with dedup and sort
- Added tag `<select>` dropdown after owner dropdown (same styling)
- Created `FilterChip` component — accent-colored pill with `label:value` and X button
- Replaced "Clear (N)" button with active filter chip row
- "Clear all" text button appears when 2+ filters active
- File modified: `aida-web-react/src/components/kanban/KanbanFilterBar.tsx`

#### Phase 3: Structured Search in Header
- Added `parseStructuredQuery(input)` — regex parses `field:value` and `field:"quoted value"` patterns
- Supported fields: status, priority, type, feature, owner, tag
- Added `normalizeFilterValue(key, value)` — title-cases status/priority/type values
- On Enter keydown: parse query, apply detected filters via `setFilter()`, keep remainder as search text
- Updated placeholder: `'Search... (try owner:joe, tag:frontend)'`
- File modified: `aida-web-react/src/components/layout/Header.tsx`

#### Phase 4: Tag Pills in List Rows
- Wrapped title cell content in flex div
- Added up to 3 small tag badges after title span
- Shows `+N` overflow indicator if more than 3 tags
- File modified: `aida-web-react/src/components/list/RequirementsRow.tsx`

#### Phase 5: Markdown Description Rendering
- Replaced `EditableText` for description with inline view/edit toggle
- View mode: renders through `<Markdown remarkPlugins={[remarkGfm]}>` with prose styling from DocFullPage
- Edit mode: textarea with Ctrl+Enter to save, Escape to cancel
- Click-to-edit pencil icon on hover in view mode
- File modified: `aida-web-react/src/components/detail/DetailBody.tsx`

### Files Changed
- 5 modified (`useFilters.ts`, `KanbanFilterBar.tsx`, `Header.tsx`, `RequirementsRow.tsx`, `DetailBody.tsx`)
- 1 plan saved (`docs/plans/2026-02-20-tag-filtering-structured-search-markdown.md`)

---

### Dashboard: Sprint Summary + Clickable Status Navigation
- **Prompt**: Add active sprint summary section to Dashboard and make status count cards clickable to navigate to filtered List View
- **Actions**:

#### Phase 1: Clickable MetricsCards
- Added `useNavigate` from react-router-dom
- Changed card `<div>` to `<button>` elements
- Each card navigates to `/list?status=X` on click; "Total" navigates to `/list` (no filter)
- Added `cursor-pointer` and `hover:border-accent/50 hover:bg-surface-hover` styling
- File modified: `aida-web-react/src/components/dashboard/MetricsCards.tsx`

#### Phase 2: SprintSummary Component
- Created new component with sprint header (name, date range, days-left badge)
- Sprint-scoped status count cards (same clickable style as project-wide cards)
- Progress bar with percentage and story points display
- Uses existing sprint-utils: `getSprintNumber`, `getSprintDates`, `computeSprintProgress`
- File created: `aida-web-react/src/components/dashboard/SprintSummary.tsx`

#### Phase 3: DashboardPage Integration
- Computed active sprint + items using same pattern as Sidebar (filter Sprint type, find active, match assignments)
- Rendered `<SprintSummary>` between project-wide `<MetricsCards>` and charts grid
- Only shown when an active sprint exists
- File modified: `aida-web-react/src/components/dashboard/DashboardPage.tsx`

### Files Changed
- 2 modified (`DashboardPage.tsx`, `MetricsCards.tsx`)
- 1 created (`SprintSummary.tsx`)

---

### My Queue: Personal Focus Inbox — Requirements & Plan
- **Prompt**: Formalize the My Queue concept from aida-gui into requirements and draft an implementation plan
- **Actions**:

#### Research
- Explored existing My Queue implementation in aida-gui (local-only, rank-based, stored in YAML settings)
- Identified existing related requirements: FR-0189 (Work Queue View), FR-0340 (Team Queue View), FR-0313 (filter completed)
- Analyzed limitations: no backend storage, no API, no CLI, no collaboration

#### Requirements Created
- **EPIC-0365**: My Queue: Personal Focus Inbox (parent epic)
- **STORY-0366**: Queue: Database storage model (gapped-integer positions, SQLite/PostgreSQL)
- **STORY-0367**: Queue: REST API endpoints (CRUD + bulk reorder + summary)
- **STORY-0368**: Queue: CLI commands (aida queue list/add/remove/move/clear)
- **STORY-0369**: Queue: React web UI — My Queue view (drag-to-reorder, /queue route)
- **STORY-0370**: Queue: Dashboard focus widget (top items + count on dashboard)
- **STORY-0371**: Queue: Assign-to-queue inbox capability (cross-user assignment with notes)
- Linked existing FR-0189, FR-0340, FR-0313 as references to EPIC-0365

#### Plan Saved
- `docs/plans/2026-02-20-my-queue-personal-focus-inbox.md`
- 6-phase plan: Database → API → CLI + Web UI (parallel) → Dashboard Widget → Inbox

### My Queue: Full-Stack Implementation (EPIC-0365)
**Date**: 2026-02-21

#### Prompt
Implement the full My Queue feature across all 6 phases.

#### Actions Taken

**Phase 1: Database Storage (STORY-0366)**
- Added `QueueEntry` model to `aida-core/src/models.rs`
- Added 5 queue trait methods to `DatabaseBackend` trait in `aida-core/src/db/traits.rs`
- Updated `schema.sql` and `postgres_schema.sql` with `queue_entries` table (schema v7)
- Implemented SQLite migration v6→v7 and all 5 queue methods in `sqlite_backend.rs`
- Implemented PostgreSQL migration v6→v7 and all 5 queue methods in `postgres_backend.rs`
- Added queue wrapper methods to `Storage` class for CLI access
- Exported `QueueEntry` from `aida-core/src/lib.rs`

**Phase 2: REST API Endpoints (STORY-0367)**
- Added 5 queue routes to `create_rest_router_legacy()` in `aida-server/src/rest.rs`
- Implemented handlers: `queue_list`, `queue_add`, `queue_remove`, `queue_update`, `queue_reorder`
- Request/response types with camelCase JSON serialization
- Enrichment: each queue entry joined with requirement title/status/priority/type

**Phase 3: CLI Commands (STORY-0368)**
- Added `QueueCommand` enum (List, Add, Remove, Move, Clear) to `aida-cli/src/cli.rs`
- Implemented `handle_queue_command` in `aida-cli/src/main.rs`
- User resolution: AIDA_USER → USER → USERNAME → "default"
- Colored terminal output with status indicators

**Phase 4: React Web UI (STORY-0369)**
- Added `QueueEntry` type to `shared/types.ts`
- Created `aida-web-react/src/api/queue.ts` — API client
- Created `aida-web-react/src/hooks/useQueue.ts` — React Query hooks with optimistic updates
- Created `QueuePage.tsx` with @dnd-kit/sortable drag-to-reorder
- Created `QueueItem.tsx` with drag handle, badges, remove button
- Added `/queue` route to `App.tsx`
- Added "My Queue" nav item with Inbox icon to `Sidebar.tsx`
- Added "Add to Queue" button (ListPlus icon) to `DetailHeader.tsx`
- Added hover "Add to Queue" button to `RequirementsRow.tsx`

**Phase 5: Dashboard Widget (STORY-0370)**
- Created `QueueWidget.tsx` — compact card with top 5 items
- Auto-hides when queue is empty
- Added to `DashboardPage.tsx` after SprintSummary

**Phase 6: Assign-to-Queue (STORY-0371)**
- Handled by design: `added_by` field, `note` field, visual badge in QueueItem, CLI `--user` flag

#### Files Modified (Backend — 8)
- `aida-core/src/models.rs` — QueueEntry struct
- `aida-core/src/db/traits.rs` — 5 queue trait methods
- `aida-core/src/db/schema.sql` — queue_entries table, v7
- `aida-core/src/db/sqlite_backend.rs` — queue methods + migration
- `aida-core/src/db/postgres_schema.sql` — queue_entries table, v7
- `aida-core/src/db/postgres_backend.rs` — queue methods + migration
- `aida-core/src/storage.rs` — queue wrapper methods
- `aida-core/src/lib.rs` — export QueueEntry

#### Files Modified (Server — 1)
- `aida-server/src/rest.rs` — 5 queue route handlers

#### Files Modified (CLI — 2)
- `aida-cli/src/cli.rs` — QueueCommand enum
- `aida-cli/src/main.rs` — handle_queue_command

#### Files Created (Frontend — 5)
- `aida-web-react/src/api/queue.ts`
- `aida-web-react/src/hooks/useQueue.ts`
- `aida-web-react/src/components/queue/QueuePage.tsx`
- `aida-web-react/src/components/queue/QueueItem.tsx`
- `aida-web-react/src/components/dashboard/QueueWidget.tsx`

#### Files Modified (Frontend — 5)
- `shared/types.ts` — QueueEntry type
- `aida-web-react/src/App.tsx` — /queue route
- `aida-web-react/src/components/layout/Sidebar.tsx` — My Queue nav item
- `aida-web-react/src/components/detail/DetailHeader.tsx` — Add to Queue button
- `aida-web-react/src/components/list/RequirementsRow.tsx` — Add to Queue hover button

#### Verification
- `cargo build` — all workspace members compile
- `cargo test -p aida-core` — 68/68 tests pass
- `npx tsc --noEmit` — no TypeScript errors

---

### Auto-Link Spec IDs in Markdown (2026-02-21)
- **Prompt**: Auto-link spec IDs (e.g., EPIC-0365, FR-0042) in markdown content so they become clickable hyperlinks
- **Requirement**: STORY-0372 (under EPIC-0365)
- **Actions**:
  - Created `remarkSpecLinks` remark plugin that detects XXX-NNNN patterns in markdown text nodes and converts them to link nodes
  - Created `LinkedMarkdown` wrapper component around `react-markdown` that injects the plugin and handles click navigation to open the detail panel
  - Applied `LinkedMarkdown` to all 4 markdown rendering locations: DetailBody, DocFullPage, DocDetailPanel, SkillDetailPanel
  - Spec ID patterns supported: 1-8 uppercase letters followed by hyphen and 1-6 digits (e.g., FR-0042, EPIC-0365, STORY-0372)
- **Commit**: 7386495

### Admin Rebuild & Restart (2026-02-21)
- **Prompt**: Implement dev-mode Admin tab in Settings for triggering cargo build from browser with real-time SSE output streaming and auto-restart
- **Requirement**: TASK-0373
- **Actions**:
  - Created `aida-server/src/admin.rs` — AdminState, status endpoint (`GET /api/v2/admin/status`), SSE rebuild endpoint (`GET /api/v2/admin/rebuild?restart=true`) with concurrent build protection via AtomicBool, workspace root auto-detection, stdout/stderr streaming, and process replacement restart
  - Created `aida-web-react/src/api/admin.ts` — AdminStatus type, fetchAdminStatus, SSE event types
  - Created `aida-web-react/src/hooks/useAdmin.ts` — useAdminStatus (React Query), useRebuild (EventSource SSE with reconnect polling)
  - Created `aida-web-react/src/components/settings/AdminTab.tsx` — Server status card, build action buttons, dev-mode hint, error banner, terminal log with auto-scroll
  - Modified `aida-server/src/main.rs` — Added `mod admin`, AdminState creation gated on AIDA_DEV_MODE env var, merged admin router into both multi-project and legacy REST routers
  - Modified `aida-web-react/src/components/settings/SettingsView.tsx` — Added Admin tab to settings tab bar

#### Files Changed
- `aida-server/src/admin.rs` (new) — Backend admin module
- `aida-server/src/main.rs` — Wire admin routes
- `aida-web-react/src/api/admin.ts` (new) — API types
- `aida-web-react/src/hooks/useAdmin.ts` (new) — React hooks
- `aida-web-react/src/components/settings/AdminTab.tsx` (new) — UI component
- `aida-web-react/src/components/settings/SettingsView.tsx` — Add admin tab

#### Verification
- `cargo build -p aida-server` — compiles with no new warnings
- `npx tsc --noEmit` — no TypeScript errors

---

### Session — 2026-02-21: AIDA Chat (STORY-0374)

#### Prompt
Implement requirements-aware AI chat for PMs/stakeholders — web-based chat UI that streams AI responses from the Claude API with full requirements database as context. Spec IDs auto-linked via existing LinkedMarkdown component.

#### Actions Taken

**Phase 1: Make aida-core prompt helpers public**
- Made `build_project_context` and `build_requirements_summary` `pub` in `aida-core/src/ai/prompts.rs`
- Added new `pub fn build_all_requirements_summary(store)` — full requirements summary with status/priority for chat context

**Phase 2: Backend chat.rs module**
- Added `reqwest` (with stream feature) and `futures-util` dependencies to `aida-server/Cargo.toml`
- Created `aida-server/src/chat.rs` with:
  - `GET /api/v2/chat/status` — returns availability (checks ANTHROPIC_API_KEY)
  - `POST /api/v2/chat` — SSE streaming endpoint: validates key, builds system prompt with project context + all requirements, POSTs to Claude API with streaming, parses `content_block_delta` events, forwards as `event: delta` SSE events
  - Model defaults to `claude-sonnet-4-20250514`, overridable via `AIDA_CHAT_MODEL`
  - Stub router for multi-project mode (returns `available: false`)
- Wired `chat::create_chat_router(state)` into legacy REST router and stub into multi-project router

**Phase 3: Frontend API + hooks**
- Created `aida-web-react/src/api/chat.ts` — types + `fetchChatStatus()` + `sendChatMessage()` (raw fetch for streaming)
- Created `aida-web-react/src/hooks/useChat.ts`:
  - `useChatStatus()` — React Query hook for status endpoint
  - `useChat()` — full conversation state: messages, isStreaming, send(), clear(); SSE parsing via ReadableStream reader

**Phase 4: ChatPage component**
- Created `aida-web-react/src/components/chat/ChatPage.tsx`:
  - Header with clear button
  - Empty state with 5 starter question chips
  - Message bubbles: user (right, accent bg), assistant (left, LinkedMarkdown rendering with auto-linked spec IDs)
  - Blinking cursor during streaming
  - Fixed input bar with textarea (Enter to send, Shift+Enter for newline)
  - Loading/unavailable states

**Phase 5: Route + navigation**
- Added `/chat` route in `App.tsx`
- Added Chat nav item with `MessageCircle` icon in `Sidebar.tsx` (before Settings)

#### Files Changed
- `aida-core/src/ai/prompts.rs` — Made helpers pub, added `build_all_requirements_summary`
- `aida-server/Cargo.toml` — Added reqwest + futures-util deps
- `aida-server/src/chat.rs` (new) — Chat SSE streaming endpoints
- `aida-server/src/main.rs` — Added mod chat, merged routers
- `aida-web-react/src/api/chat.ts` (new) — API types and fetch functions
- `aida-web-react/src/hooks/useChat.ts` (new) — Chat hooks
- `aida-web-react/src/components/chat/ChatPage.tsx` (new) — Chat UI page
- `aida-web-react/src/App.tsx` — Added /chat route
- `aida-web-react/src/components/layout/Sidebar.tsx` — Added Chat nav item

#### Verification
- `cargo build -p aida-server` — compiles with no new warnings
- `npx tsc --noEmit` — no TypeScript errors

---

### Session — 2026-02-21: Runtime API Key Management (TASK-0374)

#### Prompt
Implement API key management in Settings Admin tab so PMs/stakeholders can set ANTHROPIC_API_KEY without env vars.

#### Actions Taken
- Added `RwLock<HashMap>` API key store to AdminState with env var pre-population
- Added GET/PUT/DELETE `/api/v2/admin/api-keys` REST endpoints with masked key display
- Created `ChatState` wrapper combining `ServerState` + `AdminState`, updated chat module to read API key from runtime store
- Added frontend API functions (`fetchApiKeys`, `setApiKey`, `deleteApiKey`) and React Query hooks
- Built `ApiKeysCard` component in AdminTab with set/update/clear UX
- Cargo build and TypeScript check pass clean

#### Files Changed
- `aida-server/src/admin.rs` — Added API key store and REST endpoints
- `aida-server/src/chat.rs` — Created ChatState wrapper, read from runtime store
- `aida-server/src/main.rs` — Passed admin_state to chat router
- `aida-web-react/src/api/admin.ts` — API key fetch/set/delete functions
- `aida-web-react/src/hooks/useAdmin.ts` — useApiKeys, useSetApiKey, useDeleteApiKey hooks
- `aida-web-react/src/components/settings/AdminTab.tsx` — ApiKeysCard component

#### Design Decisions
- Keys stored in-memory only (not persisted to disk) for security
- Env var fallback: if runtime key is cleared, falls back to env var
- Masked display: first 7 + last 4 chars shown (e.g., "sk-ant-...xYz9")
- Auto-invalidates chat-status query on key changes

#### Git
- Commit: feat(admin): add runtime API key management via Settings UI
- Requirement: TASK-0374

### Add .env File Support to aida-server (2026-02-21)

**Prompt**: Add dotenvy support so the server auto-loads `.env` on startup for convenient local API key configuration.

#### Actions Taken
- Added `dotenvy = "0.15"` to workspace `Cargo.toml` and `aida-server/Cargo.toml`
- Added `dotenvy::dotenv().ok()` as first line of `main()` in `aida-server/src/main.rs`, before `Args::parse()` so env vars are available for all downstream initialization including `AdminState::new()`
- Created `.env` file with `ANTHROPIC_API_KEY` for local development
- Added `.env` to root `.gitignore` to prevent secret leakage
- Created `.env.example` template documenting available environment variables

#### Files Changed
- `Cargo.toml` — Added `dotenvy = "0.15"` to workspace dependencies
- `aida-server/Cargo.toml` — Added `dotenvy = { workspace = true }`
- `aida-server/src/main.rs` — Added `dotenvy::dotenv().ok()` at start of `main()`
- `.env` — Created with ANTHROPIC_API_KEY (gitignored)
- `.env.example` — Created with documented placeholder variables
- `.gitignore` — Added `.env` pattern

#### Design Decisions
- Using `dotenvy` (maintained fork of `dotenv`) rather than the unmaintained `dotenv` crate
- `.ok()` silently ignores missing `.env` — not an error if absent
- Loaded before `Args::parse()` so env vars are available for clap defaults and all downstream code
- `.env` gitignored; `.env.example` committed as documentation for developers

#### Git
- Commit: feat(server): add .env file support via dotenvy

### Add Timestamps and Owner to Chat Requirements Context (2026-02-21)

**Prompt**: Chat couldn't answer "what requirements were added today" because the requirements summary sent to Claude lacked timestamps.

#### Actions Taken
- Updated `build_all_requirements_summary()` in `aida-core/src/ai/prompts.rs` to include `created_at` date, `modified_at` date, and `owner` for each requirement
- Added today's date to the summary header so Claude knows what "today" means
- Format: `- SPEC-ID [type|status|priority] created:YYYY-MM-DD modified:YYYY-MM-DD owner:name: title — description`

#### Files Changed
- `aida-core/src/ai/prompts.rs` — Added date/owner fields to `build_all_requirements_summary()`

#### Verification
- Chat correctly answered "what requirements were created today 2026-02-21?" listing all 20 requirements with spec IDs

#### Git
- Commit: fix(chat): include timestamps and owner in requirements context

### Add Git History to Chat Context (2026-02-21)

**Prompt**: Chat couldn't answer "what git commits happened today" — it only had requirements context. Include recent git log in the system prompt.

#### Actions Taken
- Added `build_git_context()` async function in `aida-server/src/chat.rs` that runs `git log` to get last 50 commits (hash, date, author, message)
- Included git context in the system prompt alongside requirements context
- Runs in a blocking task since it spawns a subprocess; returns empty string gracefully if not in a git repo

#### Files Changed
- `aida-server/src/chat.rs` — Added `build_git_context()` and included in system prompt

#### Verification
- Chat correctly listed all 15 commits from today, grouped by feature area, with linked requirement IDs

#### Git
- Commit: feat(chat): include recent git history in chat context

### Add REST Evaluate Endpoint + React UI (2026-02-21)

**Prompt**: The React web UI has no way to trigger AI evaluation of requirements. Add a dedicated evaluate endpoint and UI components.

#### Actions Taken
- Created `aida-server/src/evaluate.rs` — new module with `POST /api/v2/requirements/:id/evaluate` endpoint
  - Reuses `build_evaluation_prompt()` and `parse_evaluation_response()` from `aida-core`
  - Calls Claude API directly via `reqwest` (non-streaming, needs structured JSON)
  - Stores result as `StoredAiEvaluation` on the requirement and persists to database
- Wired `mod evaluate` and `create_evaluate_router()` into `aida-server/src/main.rs`
- Created `aida-web-react/src/api/evaluate.ts` — API function using `apiFetch`
- Created `aida-web-react/src/hooks/useEvaluation.ts` — `useMutation` hook that invalidates requirement queries on success
- Added Sparkles evaluate button to `DetailHeader.tsx` (shows spinner during eval, checkmark on success)
- Added AI Evaluation results section to `DetailBody.tsx` (score badge, strengths, issues with severity, suggested improvements, timestamp)

#### Files Created
- `aida-server/src/evaluate.rs`
- `aida-web-react/src/api/evaluate.ts`
- `aida-web-react/src/hooks/useEvaluation.ts`

#### Files Modified
- `aida-server/src/main.rs` — Added `mod evaluate` and router merge
- `aida-web-react/src/components/detail/DetailHeader.tsx` — Added evaluate button
- `aida-web-react/src/components/detail/DetailBody.tsx` — Added evaluation results section

#### Git
- Commit: feat(evaluate): add REST evaluate endpoint and React UI (STORY-0375)

---

### List View Drag-and-Drop: Queue + Tree Reparenting (2026-02-21)

**Prompt**: Add drag-and-drop to the List View for two capabilities: drag any row to add it to My Queue, and drag to reparent items in tree mode.

#### Actions Taken

**Phase 1: Backend — `PUT /api/v2/requirements/:id/parent` endpoint**
- Added `SetParentRequest` struct with `parent_id: Option<String>`
- Handler resolves child and parent by UUID or spec_id
- When `parent_id` is set: calls `store.set_relationship()` with `RelationshipType::Parent` (auto-removes existing parent)
- When `parent_id` is null: finds and removes existing Parent relationship via `store.remove_relationship()`
- Saves store and returns updated requirement
- File modified: `aida-server/src/rest.rs`

**Phase 2: Frontend API + Hook**
- Added `setParent(id, parentId)` API function in `api/requirements.ts`
- Added `useSetParent()` mutation hook in `hooks/useRequirements.ts` — invalidates requirements on settle

**Phase 3: Tree Utility — Circular Reference Prevention**
- Added `isDescendant(roots, ancestorId, candidateId)` to `lib/tree-utils.ts`
- Walks tree from ancestorId, returns true if candidateId found among descendants
- Used in drag-end handler to prevent dropping a parent onto its own child/grandchild

**Phase 4: Draggable List Rows**
- `RequirementsRow.tsx` — Added `useDraggable({ id })`, drag handle (GripVertical icon visible on hover), transform style, isDragging opacity
- `TreeRow.tsx` — Added both `useDraggable` and `useDroppable` on same element, drag handle, `isOver` highlight (ring-2 accent glow), combined refs

**Phase 5: DndContext in RequirementsList**
- Wrapped table in `DndContext` with `PointerSensor` (5px distance) and `pointerWithin` collision detection
- Queue drop zone: appears above table when dragging, "Drop here to add to My Queue" with ListPlus icon
- Root drop zone (tree mode only): appears below table, "Drop here to make root-level (remove parent)" with XCircle icon
- DragOverlay: compact card with spec_id + title
- Drag handle column header (narrow empty `<th>`) added to align with handle cells

**handleDragEnd logic**:
- `queue-drop-zone` → `addToQueue.mutate()`
- `root-drop-zone` → `setParent.mutate({ parentId: null })`
- tree row → `setParent.mutate({ parentId: overId })` with circular reference check

#### Files Modified
- `aida-server/src/rest.rs` — Added route + handler for parent assignment
- `aida-web-react/src/api/requirements.ts` — Added `setParent()` function
- `aida-web-react/src/hooks/useRequirements.ts` — Added `useSetParent()` hook
- `aida-web-react/src/lib/tree-utils.ts` — Added `isDescendant()` helper
- `aida-web-react/src/components/list/RequirementsList.tsx` — Added DndContext, drop zones, drag overlay
- `aida-web-react/src/components/list/RequirementsRow.tsx` — Added useDraggable, drag handle
- `aida-web-react/src/components/list/TreeRow.tsx` — Added useDraggable + useDroppable, drag handle, drop highlight

#### Verification
- `cargo build -p aida-server` — compiles with no new warnings
- `npx tsc --noEmit` — no TypeScript errors

---

### Keyboard Shortcuts System (2026-02-21)

**Prompt**: Implement a centralized keyboard shortcuts system for the React web dashboard, matching the aida-gui's capabilities with chord navigation, list selection, and quick pickers.

#### Actions Taken

**Phase 1: Core Infrastructure**
- Created `src/hooks/useHotkeys.ts` — `HotkeyBinding` interface, `HotkeyContext`, `useHotkeys()` consumer hook, `isInputFocused()` helper
- Created `src/components/hotkeys/HotkeyProvider.tsx` — single `document.addEventListener('keydown')` handler with:
  - Key normalization (modifier keys, shifted chars like `?`)
  - Chord sequence support (two-key sequences like `g` then `l`)
  - 1000ms chord timeout with automatic clear
  - Input field exclusion (skip shortcuts when typing in INPUT/TEXTAREA/SELECT/contentEditable)
  - Dynamic binding registration/unregistration
- Created `src/components/hotkeys/ChordIndicator.tsx` — fixed bottom-right badge showing pending chord key (e.g., `g...`)
- Created `src/components/hotkeys/KeyboardHelp.tsx` — `?` triggered modal showing all shortcuts grouped by category with key badges

**Phase 2: Global & Navigation Shortcuts**
- Created `src/hooks/useGlobalHotkeys.ts` — registers global shortcuts:
  - `?` — Show keyboard help modal
  - `/` — Focus search input
  - `Escape` — Close detail panel
  - `g+d/q/b/l/s/t/c/x` — Navigate to Dashboard/Queue/Board/List/Sprints/Timeline/Chat/Settings
- Created `src/components/layout/GlobalHotkeys.tsx` — wrapper component calling `useGlobalHotkeys()` inside HotkeyProvider
- Modified `AppLayout.tsx` — wrapped in `<HotkeyProvider>`, rendered `<GlobalHotkeys />`
- Modified `Header.tsx` — removed standalone `/` keydown listener (now centralized)
- Modified `DetailPanel.tsx` — removed standalone `Escape` keydown listener (now centralized)

**Phase 3: List View Selection**
- Created `src/hooks/useListSelection.ts` — manages selected row with hotkeys:
  - `j` / `ArrowDown` — Select next row
  - `k` / `ArrowUp` — Select previous row
  - `Enter` — Open detail panel for selected row
  - `q` — Add selected row to queue
  - `Escape` — Clear selection (when no detail panel open)
- Modified `RequirementsList.tsx` — integrated `useListSelection`, passes `isSelected` to rows, scrolls selected into view
- Modified `RequirementsRow.tsx` — converted to `forwardRef`, added `isSelected` prop with `ring-2 ring-accent/40 bg-accent/5` styling
- Modified `TreeRow.tsx` — converted to `forwardRef`, added `isSelected` prop with selection styling

**Phase 4: Quick Pickers**
- Created `src/components/ui/QuickPicker.tsx` — keyboard-navigable popover for changing properties:
  - Arrow keys / j/k to navigate options
  - Enter to select, Escape to close
  - Positioned near selected row via anchor ref
  - Capture-phase keydown listener to intercept before hotkey system
- Added `s/p/o` shortcuts in RequirementsList for status/priority/owner pickers
  - `s` — Open status picker (Draft, Approved, In-Progress, Completed, Rejected)
  - `p` — Open priority picker (High, Medium, Low)
  - `o` — Open owner picker (derived from existing requirement owners)

#### Files Created (8)
- `src/hooks/useHotkeys.ts`
- `src/hooks/useGlobalHotkeys.ts`
- `src/hooks/useListSelection.ts`
- `src/components/hotkeys/HotkeyProvider.tsx`
- `src/components/hotkeys/ChordIndicator.tsx`
- `src/components/hotkeys/KeyboardHelp.tsx`
- `src/components/layout/GlobalHotkeys.tsx`
- `src/components/ui/QuickPicker.tsx`

#### Files Modified (6)
- `src/components/layout/AppLayout.tsx` — HotkeyProvider wrapper
- `src/components/layout/Header.tsx` — Removed standalone `/` listener
- `src/components/detail/DetailPanel.tsx` — Removed standalone `Escape` listener
- `src/components/list/RequirementsList.tsx` — Selection + pickers integration
- `src/components/list/RequirementsRow.tsx` — forwardRef + isSelected
- `src/components/list/TreeRow.tsx` — forwardRef + isSelected

#### Verification
- `npx tsc --noEmit` — no TypeScript errors

---

### Advanced Query Builder for List View (2026-02-22)

- **Prompt**: Implement advanced query builder for AIDA List View with react-querybuilder, json-logic evaluation, AND/OR grouping, saved queries, and URL persistence
- **Actions**:
  - Installed `react-querybuilder` and `json-logic-js` dependencies
  - Created type declarations for json-logic-js (`src/types/json-logic-js.d.ts`)
  - Created field definitions builder (`src/lib/query-fields.ts`) — dynamically discovers owners, features, sprints, and custom fields from data; supports 15+ field types with appropriate operators
  - Created json-logic evaluation engine (`src/lib/query-eval.ts`) — enriches requirements with virtual `_sprint` and `_cf_*` fields, registers custom json-logic operations for case-insensitive text/array matching
  - Created `useAdvancedQuery` hook (`src/hooks/useAdvancedQuery.ts`) — manages query state, URL persistence via base64-encoded `?aq=` param, localStorage saved queries with save/load/delete
  - Created `AdvancedQueryBuilder` component (`src/components/filters/AdvancedQueryBuilder.tsx`) — wraps react-querybuilder with Tailwind dark-theme styling matching existing filter bar
  - Created `SavedQueryPicker` component (`src/components/filters/SavedQueryPicker.tsx`) — dropdown for saving current query, loading saved queries, and deleting queries
  - Modified `RequirementsList.tsx` — added "Advanced" toggle button (highlights when active with ON badge), chained filter pipeline (dropdowns → advanced query), rendered query builder between filter bar and table, added `f` hotkey for toggle

#### Files Created (6)
- `src/types/json-logic-js.d.ts`
- `src/lib/query-fields.ts`
- `src/lib/query-eval.ts`
- `src/hooks/useAdvancedQuery.ts`
- `src/components/filters/AdvancedQueryBuilder.tsx`
- `src/components/filters/SavedQueryPicker.tsx`

#### Files Modified (2)
- `aida-web-react/package.json` — added react-querybuilder + json-logic-js
- `src/components/list/RequirementsList.tsx` — integrated advanced query builder

#### Verification
- `npx tsc --noEmit` — no TypeScript errors

---

### Owner-Scoped Queues: View Any User's Queue (2026-02-22)

- **Prompt**: Add `?user=` URL param to Queue page so users can view any owner's queue, with owner-picker dropdown and read-only mode
- **Actions**:
  - Modified `QueuePage.tsx` — replaced hardcoded `USER_ID = 'default'` with `useSearchParams` to read `?user=` param, added owner-picker `<select>` dropdown populated from `useRequirements`, dynamic title/subtitle (`My Queue` vs `{userId}'s Queue`), read-only badge, conditional empty state text
  - Modified `QueueItem.tsx` — added `readOnly` prop that hides drag handle and remove button, disables `useSortable` drag, while keeping click-to-open detail panel working
  - Cleaned up unused `Trash2` import from QueuePage

#### Files Modified (2)
- `src/components/queue/QueuePage.tsx` — URL param, owner picker, read-only mode
- `src/components/queue/QueueItem.tsx` — readOnly prop for drag/remove controls

#### Verification
- `npx tsc --noEmit` — no TypeScript errors

---

### My Activity: Planned vs. Actual Work Reconciliation (2026-02-22)

- **Prompt**: Implement "My Activity" page that shows actual work cross-referenced against queue to surface delta between planned and actual work
- **Actions**:
  - Created `src/lib/activity-utils.ts` — data transformation layer: `buildUserActivity()` reuses `buildTimelineEvents()` to extract per-user activity, cross-references with queue entries to tag items as "in queue" or not; `computeActivityStats()` computes worked-on/in-queue/unqueued-work/queue-untouched stats; `groupActivityByDate()` wraps `groupEventsByDate()` for date grouping; time range filtering (today/week/month/all)
  - Created `src/components/activity/ActivityStatsBar.tsx` — 4 compact stat cards (Worked On blue, In Queue green, Unqueued Work amber, Queue Untouched slate) showing planned vs. actual delta
  - Created `src/components/activity/ActivityItemCard.tsx` — single activity event card with event type icon (Sparkles/Pencil/MessageSquare), spec ID, title, relative timestamp, "In Queue" green badge, change/comment description preview
  - Created `src/components/activity/ActivityDateGroup.tsx` — date-grouped event list with sticky date header and event count
  - Created `src/components/activity/ActivityPage.tsx` — main page composing all components: owner picker dropdown (same `?user=` URL param pattern as QueuePage), time range selector, stats bar, two-column layout with scrollable activity feed and detail panel (reuses `TimelineDetailPanel`)
  - Modified `src/App.tsx` — added `/activity` route after `/queue`
  - Modified `src/components/layout/Sidebar.tsx` — added "My Activity" nav item with `Activity` icon after "My Queue"
  - Modified `src/hooks/useGlobalHotkeys.ts` — added `g+a` chord shortcut for navigation

#### Files Created (5)
- `src/lib/activity-utils.ts` — Activity data transformation + stats computation
- `src/components/activity/ActivityPage.tsx` — Main page with user scoping, time filter, two-column layout
- `src/components/activity/ActivityStatsBar.tsx` — 4 stat cards showing planned vs. actual delta
- `src/components/activity/ActivityItemCard.tsx` — Single activity event with queue badge
- `src/components/activity/ActivityDateGroup.tsx` — Date-grouped event list

#### Files Modified (3)
- `src/App.tsx` — Add `/activity` route
- `src/components/layout/Sidebar.tsx` — Add "My Activity" nav item
- `src/hooks/useGlobalHotkeys.ts` — Add `g+a` chord shortcut

#### Verification
- `npx tsc --noEmit` — no TypeScript errors

---

### Enhanced Description Editor: Expand, Preview, and Markdown Help (2026-02-22)

- **Prompt**: Add expand/collapse, live markdown preview, and help cheat sheet to the description editor in the detail panel
- **Actions**:
  - Modified `src/components/detail/DetailBody.tsx` — added 3 state variables (`expanded`, `showPreview`, `showHelp`), toolbar row with icon buttons (Maximize2/Minimize2, Eye/EyeOff, HelpCircle from lucide), expanded mode (min-h-[50vh] vs rows=8), live preview pane using LinkedMarkdown with same prose classes, and markdown help card with syntax reference
  - Added `Maximize2`, `Minimize2`, `Eye`, `EyeOff`, `HelpCircle` icon imports from lucide-react

#### Files Modified (1)
- `src/components/detail/DetailBody.tsx` — Toolbar, expand mode, preview pane, help card

#### Verification
- `npx tsc --noEmit` — no TypeScript errors

---

### Colored Text Markdown Syntax (2026-02-22)

- **Prompt**: Add color support to LinkedMarkdown
- **Actions**:
  - Created `src/lib/remarkColorText.ts` — custom remark plugin that converts `::color[text]` syntax to link nodes with `#color:<name>` URLs; supports 20 colors (red, green, blue, yellow, orange, purple, pink, cyan, gray, grey, amber, lime, teal, indigo, violet, rose, emerald, sky, slate, white); exports `COLOR_CLASSES` map to Tailwind classes
  - Modified `src/components/ui/LinkedMarkdown.tsx` — imported remarkColorText plugin, added `#color:` href detection in AnchorComponent to render `<span>` with Tailwind color class
  - Modified `src/components/detail/DetailBody.tsx` — updated help card with color syntax reference

#### Files Created (1)
- `src/lib/remarkColorText.ts` — Remark plugin for `::color[text]` syntax

#### Files Modified (2)
- `src/components/ui/LinkedMarkdown.tsx` — Color text rendering via link-component pattern
- `src/components/detail/DetailBody.tsx` — Help card updated with color syntax

#### Verification
- `npx tsc --noEmit` — no TypeScript errors

---

### Syntax Highlighting for Code Blocks (2026-02-22)

- **Prompt**: Add program language syntax highlighting for code blocks
- **Actions**:
  - Installed `react-syntax-highlighter` and `@types/react-syntax-highlighter`
  - Modified `src/components/ui/LinkedMarkdown.tsx` — imported Prism SyntaxHighlighter with oneDark theme, added `CodeComponent` callback that detects `language-*` class on fenced code blocks and renders via SyntaxHighlighter, multi-line blocks without language render as plain text, inline code unchanged
  - Modified `src/components/detail/DetailBody.tsx` — updated help card to mention syntax highlighting

#### Dependencies Added
- `react-syntax-highlighter` — Syntax highlighting component (Prism-based)
- `@types/react-syntax-highlighter` — TypeScript definitions

#### Files Modified (2)
- `src/components/ui/LinkedMarkdown.tsx` — SyntaxHighlighter integration with oneDark theme
- `src/components/detail/DetailBody.tsx` — Help card mentions syntax highlighting

#### Verification
- `npx tsc --noEmit` — no TypeScript errors

---

### Prompt: Implement `aida init` command (2026-02-22)

#### Request
Implement a proper `aida init` command that creates the database, scaffolds all files, creates `docs/plans/`, seeds META requirements, and prints getting-started guidance. Add `--no-hooks` and `--no-skills` escape hatches. Also fix `determine_requirements_path()` to detect `requirements.db`.

#### Actions Taken
1. **`aida-core/src/project.rs`** — Added `requirements.db` detection before `requirements.yaml` check in `determine_requirements_path()`
2. **`aida-cli/src/cli.rs`** — Added `Init { no_skills, no_hooks, force }` variant to `Command` enum
3. **`aida-cli/src/main.rs`** — Added early dispatch before path resolution, `handle_init_command()` function with:
   - Idempotency check (refuses without `--force` if already initialized)
   - SQLite database creation + META requirement seeding
   - `docs/plans/` directory creation
   - Scaffold execution with configurable `ScaffoldConfig`
   - Colored post-init guidance message
4. **Migration check fix** — Skip migration check when path is already `.db` to avoid false "Both YAML and SQLite exist" warning

#### Files Modified (3)
- `aida-core/src/project.rs` — `requirements.db` detection, updated error message
- `aida-cli/src/cli.rs` — `Init` command variant
- `aida-cli/src/main.rs` — `handle_init_command()`, early dispatch, migration check fix

#### Verification
- Fresh `aida init` creates all expected files
- `aida list` works immediately after init
- `aida init` again shows "already initialized" message
- `--force` allows reinitializing
- `--no-skills` omits `.claude/skills/` and `.claude/commands/`
- `--no-hooks` omits `.claude/hooks/`

---

### Prompt: Modernize documentation — OVERVIEW.md, User Guide, Getting Started (2026-02-22)

#### Request
Update OVERVIEW.md with the new init command. Modernize the User Guide (aida-gui is the desktop app, web dashboard is the preferred UI). Create a standalone Getting Started guide.

#### Actions Taken
1. **OVERVIEW.md** — Updated Vision (AI-native, SQLite default), Project Structure (added aida-web-react as primary UI), renamed "Dual Interface" to "Three Interfaces", rewrote Getting Started to use `aida init` + web dashboard, fixed Data Storage (SQLite now default), fixed Use Case 3 (requirements.db not .yaml), updated Documentation section
2. **docs/getting-started.md** — Created standalone guide covering: install, `aida init`, first CLI steps, launching web dashboard, launching desktop app, Claude Code skills, storage backends, next steps
3. **docs/user-guide.md** — Major modernization:
   - Getting Started now uses `aida init` and references standalone guide
   - Added new "Web Dashboard" section documenting all 10 views, search/filtering, keyboard shortcuts, description rendering, AI features
   - Renamed "GUI Usage" to "Desktop App (aida-gui)" and condensed it
   - Updated Multi-Project resolution order (requirements.db before .yaml)
   - Updated Storage Backends (SQLite as default, added PostgreSQL, updated examples)
   - Updated keyboard shortcuts section title
   - Updated troubleshooting for web dashboard + `aida init`
   - Updated tips for SQLite-first world

#### Files Modified (3) + Created (1)
- `OVERVIEW.md` — Modernized vision, structure, getting started, storage defaults
- `docs/user-guide.md` — Web dashboard section, desktop app reframe, init command, storage updates
- `docs/getting-started.md` — New standalone Getting Started guide

---

### Rename aida-gui to aida-desktop (2026-02-22)

#### Prompt
Rename `aida-gui` to `aida-desktop`. The web dashboard (`aida-web-react`) is now the primary UI, making `aida-gui` ambiguous. The new name aligns with how docs already describe it ("Desktop App") and parallels the naming convention: `aida` (CLI), `aida-server`, `aida-desktop`, `aida-web-react`.

#### Actions Taken
1. **Phase 1 — Directory + Cargo rename**:
   - Renamed `aida-gui/` directory to `aida-desktop/`
   - Updated `aida-desktop/Cargo.toml` package name and binary name
   - Updated root `Cargo.toml` workspace member
   - Updated `aida-web/Cargo.toml` dependency path
   - Updated `pnpm-workspace.yaml`

2. **Phase 2 — Rust source updates**:
   - Updated `aida-desktop/src/main.rs`: `use aida_desktop::`, help text, version string
   - Updated `aida-web/src/lib.rs`, `client.rs`, `app.rs`: all `aida_gui` → `aida_desktop`
   - Updated `aida-desktop/src/lib.rs`, `build.rs`, `Trunk.toml`, `storage/embedded.rs`, `ui/mod.rs`: comments
   - Updated `aida-desktop/src/app.rs`: cargo build command, binary fallback strings, migration messages
   - Kept `aida_gui_settings` key for backward compatibility (user data path)

3. **Phase 3 — Build infrastructure**:
   - Updated `Makefile`: all target names, paths, cargo commands (~18 occurrences)
   - Updated `.github/workflows/ci.yml`: gui_binary references
   - Updated `docker/Dockerfile.web` and `docker/Dockerfile.server`: COPY paths, WORKDIR

4. **Phase 4 — Templates**:
   - Updated `aida-core/templates/skills/aida-req.md` and `aida-implement.md`

5. **Phase 5 — Documentation**:
   - Updated OVERVIEW.md, README.md, docs/user-guide.md, docs/getting-started.md, docs/DEVELOPER_GUIDE.md, PLAN.md
   - Left historical docs unchanged (PROMPT_HISTORY.md, unified-gui-plan.md, plans/*.md)
   - Regenerated user-guide.html and user-guide-dark.html

6. **Verification**:
   - `cargo build --workspace` — full workspace compiles successfully
   - `./target/debug/aida-desktop --version` — shows `aida-desktop 0.1.0`
   - `./target/debug/aida-desktop --help` — shows correct binary name
   - No stale `aida-gui` references in .rs, .toml, or Makefile
   - Only `aida_gui_settings` remains in .rs (intentional backward compat)

#### Files Modified (24+)
- `Cargo.toml` — workspace member
- `aida-desktop/Cargo.toml` — package + binary name
- `aida-web/Cargo.toml` — dependency path
- `pnpm-workspace.yaml` — package list
- `aida-desktop/src/main.rs` — imports, help text
- `aida-desktop/src/lib.rs`, `build.rs`, `Trunk.toml` — comments
- `aida-desktop/src/app.rs` — strings, cargo commands
- `aida-desktop/src/storage/embedded.rs`, `src/ui/mod.rs` — comments
- `aida-web/src/lib.rs`, `src/client.rs`, `src/app.rs` — imports, comments
- `Makefile` — all targets and paths
- `.github/workflows/ci.yml` — release matrix
- `docker/Dockerfile.web`, `docker/Dockerfile.server` — COPY paths
- `aida-core/templates/skills/aida-req.md`, `aida-implement.md`
- `OVERVIEW.md`, `README.md`, `PLAN.md`
- `docs/user-guide.md`, `docs/getting-started.md`, `docs/DEVELOPER_GUIDE.md`
- `docs/user-guide.html`, `docs/user-guide-dark.html` — regenerated

#### Git
- Commit: `refactor: rename aida-gui to aida-desktop`
- Pushed to main

---

## Session — 2026-02-22: Web UI Skill Invocation (Pilot: `/aida-compiler-warnings`)

### Prompt
Implement the plan for Web UI Skill Invocation — allow running skills from the React dashboard, starting with `/aida-compiler-warnings` as a pilot. Three phases: server skill runner infrastructure, React UI, and chat integration.

### Actions Taken

#### Phase 1: Server — Skill Runner Infrastructure
- **Created** `aida-server/src/skill_runner.rs` (~450 lines):
  - SSE streaming endpoint: `POST /api/v2/skills/:name/run` — runs `cargo clippy --workspace --all-targets --message-format=json`, parses JSON diagnostics, categorizes warnings by risk level (Safe Auto-Fix / Low Risk / Medium Risk / Review Needed), streams real-time log events and structured `WarningsReport` result
  - Action endpoint: `POST /api/v2/skills/:name/action` — handles `auto_fix` (runs `cargo clippy --fix`), `create_defect` (creates bug requirement), `create_task` (creates task requirement) with warning details in description
  - Chat endpoint: `POST /api/v2/skills/:name/chat` — context-aware AI Q&A with warnings report injected as system context, streams Claude API responses
  - Warning categorization by lint code: `unused_imports`/`unused_mut` → Safe Auto-Fix, `dead_code` → Low Risk, `unused_assignments` → Medium Risk, clippy correctness lints → Review Needed
  - Reuses existing patterns: SSE from admin.rs, requirement creation from rest.rs, Claude API streaming from chat.rs
- **Modified** `aida-server/src/main.rs` — Added `mod skill_runner` and merged router into legacy mode setup

#### Phase 2: React — Skill Runner UI
- **Created** `aida-web-react/src/api/skillRunner.ts` — API client with types for Warning, WarningCategory, WarningsReport, ActionResponse; functions for `runSkill()` (SSE), `executeSkillAction()` (JSON), `sendSkillChat()` (SSE)
- **Created** `aida-web-react/src/hooks/useSkillRunner.ts` — React hook managing phase (idle/running/done/error), logs, result, progress, error state; SSE event parsing for log/progress/result/error/done events
- **Created** `aida-web-react/src/components/skills/WarningsReport.tsx` — Structured results display with:
  - Summary bar (total warnings, crate breakdown)
  - Expandable category cards with risk-level color coding (green/yellow/orange/red)
  - Per-category action buttons: Auto-Fix All, Create Task, Create Defect
  - Individual warning rows with file:line, code, suggestion
- **Created** `aida-web-react/src/components/skills/SkillRunnerPanel.tsx` — Slide-out panel with:
  - Header with Run/Re-Run/Reset buttons and spinner
  - Collapsible terminal-style log output
  - Structured results via WarningsReport component
  - Progress indicator and error banners
  - Action feedback messages
- **Modified** `aida-web-react/src/components/skills/SkillCard.tsx` — Added "Run" button for runnable skills (client-side allowlist: `aida-compiler-warnings`)
- **Modified** `aida-web-react/src/components/skills/SkillsView.tsx` — Wired up SkillRunnerPanel with state management

#### Phase 3: Chat Integration
- **Created** `aida-web-react/src/components/skills/SkillChat.tsx` — Context-aware AI chat:
  - Collapsible chat section at bottom of SkillRunnerPanel
  - Starter questions (e.g., "Which dead_code warnings are safe to remove?")
  - Full SSE streaming with Claude API
  - Warnings report passed as context
  - LinkedMarkdown rendering for assistant responses

#### Documentation
- **Created** `docs/plans/2026-02-22-skill-runner-ui.md`
- **Updated** `CLAUDE.md` — Skills Browser description updated
- **Updated** `OVERVIEW.md` — Skills browser feature description updated
- **Updated** `PROMPT_HISTORY.md` — This session

#### Verification
- `cargo build -p aida-server` — compiles with only pre-existing warnings
- `npx tsc --noEmit` — TypeScript check passes
- `npx vite build` — production build succeeds

#### Git
- Commit: `bfc0e8f` — `feat(skills): add web UI skill invocation with compiler-warnings pilot`
- Pushed to main

### Prompt: Run server and test from browser
- **Date**: 2026-02-22

#### Bug Fix: Axum 0.7 Route Syntax
- Skill runner endpoints returned 404 because routes used `{name}` syntax (axum 0.8+) instead of `:name` (axum 0.7)
- **Fixed** `aida-server/src/skill_runner.rs` — Changed all route patterns from `"/api/v2/skills/{name}/..."` to `"/api/v2/skills/:name/..."`
- Rebuilt server and verified: SSE endpoint returns 483 warnings across 6 crates, action endpoint creates requirements

#### Git
- Commit: `bff9690` — `fix(skills): use axum 0.7 route syntax (:name not {name})`
- Pushed to main

### Prompt: Auto-fix feedback unclear — user clicked Auto-Fix All and was unsure what happened
- **Date**: 2026-02-22

#### Bug Fix: Show Diff Summary After Auto-Fix
- `handleAction` only displayed `response.message`, ignoring `diffSummary` and `specId` fields
- `handleRun` didn't clear action messages, so stale "Auto-Fix completed successfully" persisted across re-runs
- **Fixed** `aida-web-react/src/components/skills/SkillRunnerPanel.tsx`:
  - Replaced `actionMessage` string with `actionFeedback` object (message, diffSummary, specId, isError, suggestRerun)
  - Shows git diff --stat summary in `<pre>` block
  - Shows created spec IDs as links
  - Shows "Re-Run to see updated warnings" button after auto-fix
  - Clears feedback on new run start

#### Git
- Commit: `bdadfe4` — `fix(skills): show diff summary and suggest re-run after auto-fix`
- Pushed to main

### Prompt: Advanced filter blank screen — clicking Advanced in List View crashes with white screen
- **Date**: 2026-02-22

#### Bug Fix: Duplicate React in Vite Bundle
- react-querybuilder (v8.14.0) uses react-redux internally, which imports React
- Vite's dependency pre-bundling inlined a complete separate copy of React inside `react-querybuilder.js`
- Main app used `react.js` → `react-Bbo7wkWA.js`, while react-querybuilder used its own inline React
- Two React instances caused hooks to fail: `resolveDispatcher() is null`, `Invalid hook call`
- **Fixed** `aida-web-react/vite.config.ts` — Added `resolve.dedupe: ['react', 'react-dom']` to force single React instance
- Cleared Vite dep cache to trigger re-optimization

#### Requirements Captured (FR-0385, BUG-0386, BUG-0387)
- FR-0385: Web UI Skill Invocation with SSE streaming (completed)
- BUG-0386: Auto-fix feedback shows diff summary (completed)
- BUG-0387: Advanced filter blank screen from duplicate React (completed)

#### Git
- Commit: `178c5d2` — `fix(web): dedupe React in Vite config to fix Advanced filter crash`
- Pushed to main

---

### Prompt: Docker Quickstart — zero-dependency path to run AIDA via `docker compose up`
- **Date**: 2026-02-22

#### Docker Quickstart Implementation
- Added `--static-dir` flag to aida-server for serving React SPA via tower-http `ServeDir` with fallback to `index.html`
- Added `"fs"` feature to tower-http in workspace `Cargo.toml`
- Created 3-stage `Dockerfile`: Node frontend build → Rust binary build → slim Debian runtime
- Created `docker-compose.yml` for single-service quickstart (port 8080, SQLite volume)
- Created `.dockerignore` to exclude build artifacts, node_modules, git, databases
- Removed `Cargo.lock` from `.gitignore` (Rust best practice for applications)
- Updated `OVERVIEW.md` Getting Started: Docker as recommended path, native install for contributors
- Added Docker targets to `Makefile`: docker-build, docker-up, docker-up-d, docker-down, docker-shell
- Updated `CLAUDE.md` with Docker quickstart section
- Saved implementation plan to `docs/plans/2026-02-22-docker-quickstart.md`

#### Git
- Commit: pending

---

### Prompt: Auto-export requirements.yaml via pre-commit hook
- **Date**: 2026-02-22

#### Problem
`requirements.db` (SQLite binary) was tracked in git, causing binary bloat and undiffable history.

#### Solution
- Created `.git/hooks/pre-commit` that auto-exports `requirements.db` to `requirements.yaml` before each commit
- Hook checks for WAL journal freshness (SQLite WAL mode writes to `-wal` file, not the main `.db`)
- Skips gracefully if `aida` binary is not available
- Updated `.gitignore`: removed `requirements.yaml` ignore rule so the YAML gets tracked
- Ran `git rm --cached requirements.db` to untrack the binary
- Initial export: 351 requirements to `requirements.yaml`

#### Files Changed
- `.git/hooks/pre-commit` — Created: auto-export hook
- `.gitignore` — Modified: un-ignore `requirements.yaml`
- `requirements.yaml` — Generated: initial full export
- `requirements.db` — Untracked from git

#### Git
- Commit: `04c79b1` — `chore: replace binary requirements.db with diffable requirements.yaml`
- Pushed to main

---

### Prompt: Fix Docker — docs, skills, and chat not loading
- **Date**: 2026-02-22

#### Problem
In Docker, the server's CWD is `/app` but the project is mounted at `/repo`. The `docs_dir()` and `claude_dir()` functions resolved relative to CWD, so Docs, Plans, and Skills views were empty. Additionally, `ANTHROPIC_API_KEY` was not forwarded into the container, so Chat was unavailable.

#### Solution
- Extracted shared `project_root()` helper that derives the project directory from the database file's parent path
- Updated `docs_dir()` and `claude_dir()` to use `project_root()` instead of `std::env::current_dir()`
- Added `env_file: ../.env` to docker-compose.yml to forward secrets into the container

#### Requirements Captured
- TASK-0388: Pre-commit hook: auto-export requirements.yaml from SQLite (completed)
- BUG-0389: Fix: resolve docs/ and .claude/ relative to database path for Docker (completed)
- BUG-0390: Fix: forward ANTHROPIC_API_KEY from .env to Docker container (completed)

#### Git
- `6a0513b` — fix(server): resolve docs/ relative to database path, not CWD
- `d78fd2a` — fix(server): resolve .claude/ and docs/ relative to database path
- `ec52103` — fix(docker): load .env file for ANTHROPIC_API_KEY in container
- Pushed to main

---

### Prompt: Evaluate Distributed Architecture & Identity Specification v0.5
- **Date**: 2026-03-15

#### Context
Evaluated a comprehensive distributed architecture specification for AIDA that proposes: git-as-event-log, node-namespaced IDs (`FR-7-048`), two-tier ID scheme (node ID + agreed ID), CRDT-based conflict resolution, multi-repo workspace architecture, and phased implementation from file-based counters through full CRDT support.

#### Evaluation (Initial)
Scored the spec 92/100 overall. Key findings:
- **Clarity 95/100**: Exceptionally well-written, decisive prose
- **Completeness 88/100**: Missing security model, migration path, performance bounds
- **Feasibility 80/100**: Massive gap from current implementation; git scaling concerns
- **Architectural Soundness 93/100**: Elegant separation of concerns; two-tier ID scheme solves the distributed-yet-human-readable tension

#### Strategic Decision
After thorough analysis of AIDA's current state (166K LOC, 396 requirements, single user, centralized architecture), recommended against immediate adoption. However, the user decided to proceed based on:
1. **Better to tackle now** while codebase is young and disposable
2. **Dual-mode operation** — support both centralized (PostgreSQL, simple IDs) and distributed (git-based, node-namespaced IDs) as configurable modes
3. **Git analogy** — model the way git itself supports both connected (GitHub) and disconnected (patches) workflows
4. **Format flexibility** — not locked to TOML or one-file-per-object; YAML equally valid, sharded directories and multi-req files are options

#### Actions Taken
1. Saved distributed spec to `docs/plans/2026-03-15-distributed-architecture-identity.md` with evaluation notes
2. Created branch `distributed-architecture` for the rewrite work
3. Created `docs/plans/2026-03-15-main-branch-improvements.md` for parallel main branch work covering:
   - Phase 1: UUID v7, HLC timestamps, immutable ID enforcement, field-level conflict detection
   - Phase 2: PostgreSQL-first completion (`aida server start/stop`)
   - Phase 3: GitHub integration, API key auth, OIDC
   - Phase 4: Onboarding polish, SSE real-time presence, non-Claude AI support
   - Phase 5: Analytics and reporting
   - Cherry-picked ideas from distributed spec (UUID v7, HLC, SSE presence, tombstone relations)

#### Files Changed
- `docs/plans/2026-03-15-distributed-architecture-identity.md` — Created: full spec with evaluation notes
- `docs/plans/2026-03-15-main-branch-improvements.md` — Created: parallel main branch plan

#### Git
- `ab45252` — docs: add distributed architecture & identity specification v0.5
- `398af09` — docs: add main branch improvements plan and update distributed spec flexibility notes
- Branch `distributed-architecture` created and pushed
- Pushed to main

---

### Prompt: Implement Distributed Architecture (Full Stack)
- **Date**: 2026-03-15

#### Summary
Full implementation of the distributed architecture on `distributed-architecture` branch, then merged to main. 15 commits, 4300+ lines of new code across 7 new modules.

#### New Modules (aida-core/src/)
| Module | Lines | Purpose |
|---|---|---|
| `hlc.rs` | 228 | Hybrid Logical Clock timestamps |
| `dispenser.rs` | 590 | Sequence generation: Memory, File (Phase 1), SQLite (Phase 2) |
| `node.rs` | 455 | Node/User registries, AgreedCounters, Workspace, DeploymentMode |
| `object_store.rs` | 415 | Sharded YAML file I/O (objects/TYPE/NNN/SPEC-ID.yaml) |
| `db/git_backend.rs` | 511 | DatabaseBackend trait implementation using object_store |
| `git_ops.rs` | 501 | Git commands, CAS node registration, merge gate, sync |

#### Key Features Implemented
1. **UUID v7** — replaced all v4 UUIDs with time-ordered v7 across entire codebase
2. **HLC timestamps** — Hybrid Logical Clock for causal ordering across nodes
3. **Sequence Dispenser** — trait with 3 implementations (Memory, File+lockfile, SQLite+UPSERT)
4. **Node identity** — node/user registries, workspace config, deployment mode (centralized vs distributed)
5. **Sharded YAML object store** — one file per requirement in objects/TYPE/NNN/SPEC-ID.yaml
6. **Git-backed DatabaseBackend** — full trait impl with auto-commit on all writes
7. **Git CAS node registration** — push/pull/retry loop for distributed node ID assignment
8. **Two-tier IDs** — FR-1-001 (node-namespaced) → FR-1 (agreed, at merge gate)
9. **Merge gate** — `aida db merge-gate` assigns short agreed IDs to all unmerged objects
10. **CLI commands** — init --distributed, list, add, show, edit, del, search, comment, rel, sync, export-git, merge-gate, db info
11. **REST API** — works automatically via create_backend() directory detection

#### Git Scaling Spike
Tested at 1K/10K/50K/100K YAML files. Results: all daily ops under 0.2s at 100K. Sharded layout 58% faster on incremental push. See `docs/plans/2026-03-15-git-scaling-spike-results.md`.

#### Design Documents
- `docs/plans/2026-03-15-distributed-architecture-identity.md` — full spec
- `docs/plans/2026-03-15-git-scaling-spike-results.md` — scaling test results
- `docs/plans/2026-03-15-two-tier-id-scheme.md` — two-tier ID design & rationale
- `docs/plans/2026-03-15-main-branch-improvements.md` — parallel main branch plan

#### Git
- 15 commits on `distributed-architecture` branch
- Merged to main via fast-forward
- 121 unit tests + 10 integration tests all passing

---

### Prompt: Continue distributed architecture — hardening, GitHub integration
- **Date**: 2026-03-16

#### Hardening & UX
- Auto-detect distributed store from `.aida/config.toml` — no `--file` needed after init
- `aida db status` — shows requirements, agreed IDs, git state, dispenser mode, remote status
- Conflict-aware sync — detects field-level conflicts on `aida db sync --pull`
- `agreed_id` propagated to TypeScript types, proto definition, gRPC convert layer
- CLAUDE.md updated with full distributed mode documentation
- Merge gate fix: agreed IDs use type prefix (FR, BUG, TASK) not feature prefix
- Show command displays agreed ID, node ID, relationship count, comment count

#### GitHub Integration
- New module: `aida-core/src/integrations/github/` (client, config, models — ~960 lines)
- Async HTTP client using reqwest with Bearer token auth, GitHub API v3
- TOML-based config at `~/.config/aida/github.toml` with label mappings
- CLI commands (aida github ...):
  - `config` — set repo, token, API URL, show current config
  - `test` — verify connection, display repo info
  - `list` — list issues with state/label/limit filters
  - `show` — display issue details (supports #42 or GH-42 format)
  - `push` — create GitHub issue from AIDA requirement with mapped labels
  - `pull` — import GitHub issues as AIDA requirements with type/priority detection
  - `labels` — list repo labels, create AIDA defaults

#### Git
- 28 commits on main
- 130 unit tests + 10 integration tests all passing

---

### Prompt: Worktree storage mode, docs, setup script, first release
- **Date**: 2026-03-16

#### Orphan Branch + Worktree Storage
- Researched prior art: git-bug (CRDT ops), git-appraise (notes), Fossil (SQLite artifacts), SIT, git-dit, Bugs Everywhere
- Key finding: git-bug only survivor; orphan branch + worktree is architecturally superior for AIDA
- Implemented `aida init --distributed` default as orphan branch + worktree
- Added `--sibling` flag for multi-repo workspace mode (separate repo)
- Created `create_store_worktree()`, `remove_store_worktree()`, `has_worktree()` in git_ops
- Migrated AIDA project itself to worktree mode (354 requirements on `aida-store` branch)

#### Documentation
- `docs/storage-modes.md` — all 5 storage options with comparison matrix and decision tree
- `docs/getting-started.md` — rewritten for new users (install, init, first requirement)
- `docs/multi-user-setup.md` — PostgreSQL multi-user guide
- `docs/plans/2026-03-16-git-metadata-storage-prior-art.md` — prior art research
- `.aida/setup.sh` — bootstrap script (build, worktree, install, verify)

#### GitHub Actions Release
- `.github/workflows/release.yml` — builds for Linux x86_64/ARM64, macOS x86_64/ARM64
- MIT LICENSE added
- Package metadata (license, repository, homepage) in workspace Cargo.toml
- Tagged and released v0.1.0

#### Git
- v0.1.0 tagged and pushed
- `aida-store` orphan branch with 354 requirements pushed to GitHub
- GitHub Actions building pre-built binaries

---

### Prompt: Final push — remaining items
- **Date**: 2026-03-17

#### crates.io Ready
- Swapped ts-rs git fork to published ts-rs-forge v11 on crates.io
- `cargo publish --dry-run -p aida-core` succeeds
- Awaiting user's crates.io token to publish

#### GitHub Bidirectional Sync
- `aida github sync` — detects drift between linked AIDA requirements and GitHub issues
- Compares title and state, shows in-sync/drifted status
- `--apply` pushes AIDA state to GitHub

#### React Dashboard
- agreed_id shown in list view, tree view, kanban cards, detail header
- Hover tooltip shows both agreed and node IDs

#### Skills (8 total new/enhanced)
- Enhanced `/aida-plan` with vertical slice enforcement, tracer bullets, HITL/AFK tags
- New `/aida-grill` — adversarial design interrogation (decision tree walking)
- New `/aida-decompose` — vertical slice breakdown
- New `/aida-triage` — structured bug investigation
- New `/aida-glossary` — ubiquitous language dictionary
- New `/aida-architecture` — codebase architecture health review
- Git safety guardrails (PreToolUse hook blocking destructive operations)
- Skills review of mattpocock/skills repo (17 skills analyzed, 10 proposals)

#### Multi-Repo Workspace
- `aida db workspace-init` — creates shared store for multiple code repos
- Auto-discovers repos, creates .aida-workspace manifest
- Per-repo .aida/config.toml pointing to shared store
- 3 tests

#### Operation Log (CRDT Foundation)
- Append-only operation log inspired by git-bug
- Lamport clock for causal ordering with deterministic tiebreaker
- 14 OpKind variants covering all requirement mutations
- Merge with dedup and deterministic ordering
- 8 tests including merge determinism

#### Git
- 141 unit tests + 10 integration tests
- v0.2.0 tagged and released

---

## Session 53: Competitive Analysis Research (2026-03-17)

### Competitive Analysis Document
- **Prompt**: Research the current state of requirements management and project tracking tools in the AI/agentic coding era. Write a comprehensive, honest competitive analysis document.
- **Actions**:
  - Researched 30+ tools across 6 categories using web searches
  - Categories covered: Traditional PM tools with AI (Jira, Linear, Shortcut, Azure DevOps, Monday.com), AI-native dev tools (Cursor, Windsurf, Devin, SWE-Agent, OpenHands, Cline, Aider, Claude Code, Codex CLI), Requirements/spec tools (Doorstop, rmtoo, IBM DOORS, Polarion), Git-based tracking (GitHub Issues, GitLab, Gitea, Plane), MCP ecosystem (761+ PM servers on glama.ai), Claude Code ecosystem
  - Researched agentic coding state: SWE-bench scores (76.8% top), Devin at $10.2B valuation, OpenHands at 77.6% SWE-bench
  - Researched AI coding quality studies: SWE-Agent paper (structured interfaces improve agent performance), GitHub Copilot research (85% report higher code quality confidence)
  - Created comprehensive competitive analysis document at `docs/competitive-analysis.md`
  - Document includes: Market Landscape, Feature Comparison Matrix, AIDA Honest Strengths/Weaknesses, Agentic Coding Future analysis, AIDA positioning assessment, 5 hypothetical adoption case studies (FastAPI, Home Assistant, Zed, Servo, Neovim), Differentiation Strategy
  - Key finding: AIDA's only defensible differentiation is "deep integration between structured requirements and AI coding agent workflows" — the "context layer for agents" positioning
  - Honest assessment: AIDA cannot compete with Jira/Linear on collaboration features; should focus on agent context integration
  - Identified 12-18 month window before major players add structured-context-for-agents features

---

## Session 54: Audit, Prune, and Git-Canonical Storage Refactor (2026-05-02)

After ~6 weeks of inactivity following the March 15-18 sprint burst, returned for an audit + path-forward session. Outcome: 11 commits shipped on `main` covering housekeeping, an architectural commitment to git-as-canonical storage, and a Phase 1+2 implementation of that commitment.

### Cherry-pick + push housekeeping
- Reviewed remote `spock-dev` branch (2 commits, branched 2026-03-08, 74 commits behind main): a small `UNDERSTANDING_SKILLS.md` doc + a large "spock changes" commit adding `aida store {init,push,pull,status}` CLI duplicating distributed-mode functionality main already had
- Cherry-picked only the doc as `7dd2de1`; rejected the parallel/regressive code
- Pushed pending `agreed_id` commit

### Path-forward audit
- User asked for direction; concerned about keeping pace with the agentic landscape solo
- Read Karpathy's LLM-wiki article — his "structured markdown queryable by Claude" is the floor; AIDA adds the relationship graph + identifier stability + enforcement loop that prevents re-inventing what already exists
- Audited git activity (massive March 15-18 burst then dead silence — classic overcommit signal), code volume (aida-desktop = 43k LOC with 6 commits in 6 months — abandoned), requirements DB (176 of 362 in Draft = 49% un-curated), Claude Code primitive overlap
- Delivered a keep/demote/extract/cut framework with one-sentence pitch: "AIDA is the durable, agent-readable spec layer for AI-assisted software development — stable IDs, typed relationships, and code-to-spec traces that give Claude (and you) a map of what exists and why, across sessions."

### Prune (commit 4a948e5)
- Extracted `aida-desktop` + `aida-web` (51,221 LOC) to `/home/joe/ai/aida-desktop/` via `git filter-repo` — preserved 18 commits of history + standalone Cargo.toml + README explaining the `aida-core` path-dep that needs repointing if reactivated
- Removed both crates from main repo workspace; cargo check --workspace passes; workspace shrank from 7 → 5 crates
- Skipped: `aida-store/` is the live distributed-mode store (not an empty crate); `vibe-kanban/` is a gitignored local clone of BloopAI's project (not part of repo)

### Storage architecture decision (commits 106785c, 609c420, dd3b4d1, 807a069, af7739e, 9b990b9, a630b88, 29370a7)
- User agreed with "collapse to git as canonical, SQLite as derived materialized view, YAML as export, Postgres as opt-in plugin"
- Captured EPIC-1-001 (Approved, High); deferred bulk-import write-behind batching as `FR-1-002` child requirement
- Design doc at `docs/plans/2026-05-02-git-canonical-storage.md` with all 4 design decisions resolved (write-through writes; detect-and-rebuild on cache HEAD-SHA mismatch; aggressively simplified cache schema with FTS5; hard-cut, no deprecation window)
- Compressed original 4 phases to 3
- **Phase 1 implementation**: cache_schema.sql, cache.rs (Cache struct with rebuild/upsert/delete + HEAD-SHA stale detection, Mutex-wrapped Connection for Send+Sync), cached_git_backend.rs (CachedGitBackend wrapper implementing DatabaseBackend, write-through), CLI cache subcommand (rebuild/status), default_cache_path probe starts at store's parent so cache lives at `<project>/.aida/cache.db` (gitignored, never inside orphan-branch worktree). Verified end-to-end on live AIDA store (357 reqs).
- **Phase 2 implementation** (read-path swap): RequirementSummary projection + ListFilter + Cache::list_summaries (SQL index pushdown) + Cache::search (FTS5). Wired CLI `aida list` and `aida search` in distributed mode to use cache-backed queries — sub-ms vs ~360 YAML reads. db::create_backend for git paths now returns CachedGitBackend automatically — aida-server gets cache for free.
- **Stale tracked snapshot cleanup** (807a069): removed 357 stale YAML files at top-level `aida-store/` (last touched 2026-03-15, missing today's EPIC-1-001) — the `.aida-store/` orphan-branch worktree is the live source
- **Workspace deps cleanup** (29370a7): dropped unused eframe/egui from workspace.dependencies after desktop removal
- **Documentation** (9b990b9): rewrote CLAUDE.md storage section — git canonical, SQLite cache, YAML export, Postgres opt-in (replaced misleading "five storage backends" framing). OVERVIEW.md surgically updated to remove desktop/WASM references.

All test suites pass (6 cache tests added); workspace builds clean. EPIC-1-001 marked In-Progress.

### What remains for EPIC-1-001
- Phase 3 hard-cut: remove `yaml_backend.rs` and `sqlite_backend.rs` standalone-canonical paths; extract `postgres_backend.rs` to `aida-backend-postgres` plugin crate
- Server REST endpoints still call `backend.load()` rather than `list_summaries()` — touching the server endpoints to use cache-backed summaries is a follow-up
- AIDA's legacy `requirements.db` exists alongside the orphan branch — eventual cleanup needed

### Phase 3 user-facing portion (commit 5e66f87, same day)

User asked to complete Phase 3 with "don't overthink this" + "fine-tuned for a simple project ASAP". Resolved by flipping the `aida init` default to git-canonical:

- `aida-cli/src/cli.rs` Init command: new `--centralized` flag (deprecated, prints warning); `--distributed` retained as no-op for backwards compat (hidden); `--sibling` no longer requires `--distributed` (implies it)
- `aida-cli/src/main.rs`: dispatch flipped — default → distributed worktree, `--sibling` → distributed sibling, `--centralized` → legacy
- Extracted `complete_init_scaffolding()` helper so all three init paths get the same skills/commands/hooks/MCP/codex scaffolding (was: only legacy centralized got it). Both distributed handlers now seed META requirements + create `docs/plans/` for feature parity.
- Verified end-to-end on `/tmp/aida-test-init`: `aida init` → orphan branch + worktree + `.aida/config.toml` + skills + commands + hooks + MCP + AGENTS.md + docs/plans + 6 META reqs seeded + cache FRESH at HEAD `12b79df`. `aida add TASK-1-001` succeeded.
- Code-cleanup pieces (delete legacy backends, extract postgres) deferred — would require unwinding ~1500 LOC of `Storage` class usage, no immediate user value beyond "less code"; legacy code only reachable via deprecated `--centralized` opt-in.

### Hook fix (commit 8dd3a3a)

User reported `.claude/hooks/aida-validate-commit.sh: not found` errors when Bash was invoked from a CWD other than the project root (e.g. inside `.aida-store/`). Root cause: `.claude/settings.json` used relative paths. Fixed by switching to `$CLAUDE_PROJECT_DIR/.claude/hooks/...`. Edited `aida-core/templates/settings.json` (master template — `.claude/settings.json` symlinks to it). Future projects scaffolded by `aida init` inherit the fix.

### Documentation review / consolidate / archive (commits 2cd7de6, bda95ef, 3662d05, 3016c7c, 96a7362; archive in implicit prior commit)

User noted README's "5 Storage Modes" framing put diverse storage modes front and center, misleading after EPIC-1-001. Asked for review/consolidate/archive/update of all docs.

- **Archive (one commit, 22 files moved):** Created `docs/archive/` with a README explaining what's there. Project root drops 17→4 markdown files (CLAUDE.md, OVERVIEW.md, PROMPT_HISTORY.md, README.md). docs/ drops 24→9 active files. Archived: FINAL_*, PLAN, IMPLEMENTATION_PLAN, six INTEGRATION_* variants, SIMPLIFIED_INTEGRATION, three SPEC_ID docs, UUID_SPEC_ID_VERIFICATION, unified-gui-plan, unified-storage-architecture, PROJECT_EVALUATION_2026-02-28, AI_INTEGRATION_DESIGN, EXTERNAL_INTEGRATION_ARCHITECTURE, RELATIONSHIP_DESIGN, SPRINT_EPIC_DESIGN, DEVELOPER_GUIDE.md+.html. Deleted empty GEMINI.md.
- **README.md** (137→92 lines): leads with one-sentence pitch from path-forward audit; 5-storage-modes table moved to admin-guide / storage-modes; removed extracted-crate refs; trimmed skill list; one-paragraph Architecture summary.
- **CLAUDE.md** (352→163 lines, 54% reduction): wall-of-text dashboard view dump replaced by pointer to OVERVIEW.md; storage section rewritten around git-canonical; init defaults updated; 21-skill bulleted listing condensed; removed misleading Docker Quickstart line.
- **docs/storage-modes.md** (full rewrite): TL;DR `aida init` git-canonical; legacy YAML/SQLite explicitly deprecated; PostgreSQL flagged as opt-in feature flag; 5-column comparison matrix with default + status columns; updated decision tree.
- **docs/admin-guide.md** storage section: removed "two backends" framing (was completely wrong post-EPIC-1-001); added cache management section; updated auto-detection rules.
- **docs/getting-started.md**: rewritten Step 2 + What's Next + Quick Reference around git-canonical defaults; node-namespaced IDs (`FR-1-001`).
- **docs/user-guide.md**: deleted ~91-line Desktop App section, updated Quick Start, fixed Project Resolution Order.
- **docs/multi-user-setup.md**: reframed PostgreSQL as opt-in feature flag.
- **Global `~/.claude/CLAUDE.md`** rewritten in place to be AIDA-aware: detects AIDA-initialized projects (`.aida/config.toml`, `requirements.db`, `aida-store` branch) and instructs to use AIDA commands instead of maintaining `REQUIREMENTS.md`. Kept OVERVIEW.md / PROMPT_HISTORY.md / git-workflow / `.ports` / "stop asking compaction" preferences.

### Paradox project bootstrap

User asked to bootstrap AIDA on `~/ai/paradox` ("discard or do whatever is necessary"). Project had a half-initialized state from before today's Phase 3 work — `.aida/config.toml` and `.aida-store/` worktree existed but no scaffolding (no `.claude/`, no `.mcp.json`, no `CLAUDE.md`). The `aida-git-guardrails.sh` hook blocked an attempt to bundle `git branch -D aida-store` (correctly, doing its job). Removed worktree + `.aida/`, ran `aida init` which reused the existing orphan branch and ran the full scaffolding. Verified: 6 META requirements seeded, cache FRESH at HEAD `7a3a385`, all scaffolding present.

### Install / release / Docker fix (commits this session)

User asked four questions:
1. **Install path**: README only mentions `cargo install --git ...` from source. Added pre-built binary install option (curl + tar to /usr/local/bin) — v0.3.0 release tarballs already exist for linux-{x86_64,arm64} and darwin-{x86_64,arm64}.
2. **github sync**: `main` is current (pushed after each commit); GitHub releases are 6 weeks stale (last v0.3.0 on 2026-03-17); not on crates.io. Cut v0.4.0 by bumping `[workspace.package]` version + tag push (release workflow auto-triggers on `v*` tag).
3. **PROMPT_HISTORY**: had Session 54 entry from earlier commit but not today's later work. Appended (this entry).
4. **Docker**: was broken — `Dockerfile` and `docker/Dockerfile.server` both `COPY aida-desktop/` and `COPY aida-web/` (removed in 4a948e5). Fixed by removing those COPY lines + adding a comment about volume-mounting `.aida-store/` for git-canonical mode. Top-level Dockerfile now serves the React dashboard against any data directory passed at `/data`.

Bumped `[workspace.package]` version 0.2.0 → 0.4.0 plus the path-dep version constraints in `aida-cli/Cargo.toml` and `aida-crate/Cargo.toml`. Workspace builds clean.

## Session 55: Implementer Queue Drain — EPIC-23/24 batch:tui-prereqs/lifecycle/ux-polish/small-wins (2026-05-15)

**Request:** `/goal` — drain the implementer queue, one item per session, commit + push + open PR + autonomous-merge each, until `aida queue list` shows no implementer items remaining.

Ten queued implementer items shipped as ten separate PRs (#31–#40), each branched off `main`, built + tested + `cargo fmt --check`'d locally, squash-merged (no branch protection), then `aida queue done` + `aida db sync --push`. Final `aida db reconcile-status` promoted all ten `Done → Completed` (the merges happened between `db sync --push`es, so the pull-driven auto-bump hadn't run).

- **TASK-112** (PR #31) — `aida queue work --resume / --fresh / --list-sessions`. Resume a prior claude conversation for a scope; fresh launches mint a `--session-id` UUID recorded in the manifest's new `claude_session_id` field; `list_scope_sessions` scans `~/.claude/projects/` globally.
- **TASK-245** (PR #32) — `aida session start --reuse-branch`: check out an existing branch instead of forking; an explicitly-named `--branch` that already exists auto-reuses with a hint.
- **BUG-106** (PR #33) — auto-bump now follows the `(#N)` → review-story → `implements` linkage to bump a cluster PR's covered specs (their IDs never reach the squash-merge subject).
- **TASK-246** (PR #34) — auto-bump also completes `In Progress` review stories whose PR merged (self-merge without a re-review iteration), with an audit comment.
- **TASK-242** (PR #35) — new `aida goal` command: derives machine-checkable `/goal` completion conditions (`--batch/--epic/--spec/--pr/--queue-empty`, composing with AND), each with an explicit verify command; `--copy`/`--invoke`.
- **TASK-243** (PR #36) — `aida session end` ambiguity prompt shows `role:` inline + the linked Claude session (id + last-active) from the manifest.
- **TASK-244** (PR #37) — `aida statusline` warns on shell-role vs active-session-role mismatch (`role:X ⚠ session:Y`); `[statusline] role_mismatch_warning` opt-out.
- **TASK-228** (PR #38) — `aida session wakeup register/cancel/check/list`: a registry so a re-entered skill can deterministically skip a zombie fallback-wakeup fire.
- **TASK-233** (PR #39) — `aida session end --watch-ci`: live `gh run watch` progress, sibling to the silent `--wait-ci`.
- **BUG-103** (PR #40) — CI: "Pin stable toolchain & verify cargo" step (`rustup default stable` + a `grep` guard) fixes the intermittent macos `cargo → rustup-init` flake.

Net: `aida-cli` test suite grew 403 → 438 passing; every PR `cargo fmt --check`-clean. Tracked in-code with `trace:<SPEC>` comments throughout.

## Session 56: Implementer Pickup — TASK-259 `/aida-pr` "about to happen" banner (2026-05-16)

**Request:** `/aida-pickup TASK-259` — give `/aida-pr` a preview banner so first-time users aren't surprised by the comment writes, branch push, PR creation, and reviewer-queue routing it performs.

`/aida-pr` is a Claude Code skill (markdown), so the banner is a new workflow step, not Rust. Added **step 5 — "Print the about-to-happen banner"** to `aida-core/templates/skills/aida-pr.md`, positioned after the last read-only check (step 4, `cargo fmt --check`) and *before* the first mutation (step 6, `aida comment add`). Renumbered subsequent steps 5→6 … 11→12 and fixed every in-prose `step N` cross-reference. The banner has three sections — ✓ Completed (past tense, real data: spec/title/branch/commit+file+LOC counts/specs covered), ▶ Now I will (the four side effects in execution order: comments → push → PR → auto-queue), ↓ Then you can (four next-action commands). Suppressed by `--quiet` / `AIDA_NO_BANNER=1` (autonomous flows) and skipped in non-TTY contexts; a `sleep 3` pause gives a Ctrl-C abort window before any mutation.

**Spec correction:** the spec's original mockup listed "mark spec Done" as a side effect — `/aida-pr` never transitions status (step 3 *refuses* unless every covered spec is already `Done`). Paused for design input (AskUserQuestion + banner previews); user chose the accurate side-effect list and asked to strike "mark spec Done" from the spec. Edited the TASK-259 description's banner mockup + Acceptance accordingly. Also updated the thin `aida-core/templates/commands/aida-pr.md` wrapper (`--quiet` in Usage + a banner step).

## Session 57: Implementer Pickup — TASK-270 `aida queue work batch:NAME` positional (2026-05-16)

**Request:** `/aida-pickup TASK-270` — `aida queue work batch:workflow-hint-polish` failed because the queue prints the literal tag `batch:NAME` but the command only accepted `--batch NAME` (bare). Close the display-vs-command vocabulary asymmetry.

Implemented the preferred fix (a): `batch:NAME` is now accepted as a positional id on `aida queue work`, equivalent to `--batch NAME`. Three small helpers in `aida-cli/src/main.rs` — `strip_batch_prefix` (case-insensitive prefix strip, `Option<&str>`, char-boundary safe via `str::get`), `normalize_batch_name` (strip-or-passthrough), and `resolve_queue_work_batch` (routes a `batch:`-prefixed positional into the batch slot, returns `(effective_id, effective_batch)` with at most one `Some`). The `QueueCommand::Work` arm threads `effective_id` / `effective_batch` through the auto-complete, batch-resolution, and `handle_queue_work` paths.

Sibling commands `aida queue list` and `aida queue progress` (which have no positional slot) now strip a redundant `batch:` prefix off the `--batch` flag value, so `--batch batch:NAME` == `--batch NAME` everywhere. Added two guards on the positional form: an empty-name bail (`aida queue work batch:`) and a `--type`-conflict bail (clap rejects `--batch` + `--type`, the positional form bypassed that). Three unit tests in `queue_progress_tests`; manual verification of all four paths. `aida-cli` suite: 487 passing, `cargo fmt --check`-clean.

## Session 58: Multi-agent overnight substrate-coordination drain (2026-05-24/25)

**Request:** strategic dispatch night — MVP pace, 4-5 agents in flight (master + Codex + Antigravity-1 + Antigravity-2-integrator + aida-chat advisor). Goal evolved from "ship a few PRs" to "saturate parallel paths until substrate-coordination batch closes." `/goal` engaged late session to keep filling tomorrow's queue + clear easy stuff overnight while operator slept.

### Ships (~25 PRs landed across the AIDA repo)

**Ceiling-pattern quartet** — substrate-bouncer net for implementer lifecycle. Each catches a confident-LLM ceremony skip:
- **BUG-376** (PR-303) — `IMPLEMENTER COMPLETE — EXIT NOW` banner after `aida pr ship`; agent doesn't linger watching CI
- **BUG-378** (PR-304) — `NEW BRIEF(S) PENDING` banner on `aida queue done` when pending briefs exist for agent type
- **TASK-548** (PR-309) — skip `/aida-pickup` confirmation menu when SPEC-ID is explicit (operator already committed)
- **BUG-379** (PR-312) — `aida session start` auto-bumps spec Approved → In Progress (foundational, surfaced empirical edge case: non-atomic bump → status drift on lease-create failure; new BUG-384 filed)

**Doctor + resilience**:
- **STORY-462** (PR-305) — `aida doctor` command with 11 categories + heal sub-verbs + salvage-first discipline. First production catches in the same session.
- **STORY-463** (PR-306) — SQLite cache lock retry + `.aida/cache.db.lock-info` sidecar + actionable error message ("locked by pid=N (cmd), held since T"). Validated in BOTH directions of contention during the session itself.

**MCP fixes** (BUG-377 blast radius mapping → fix):
- **BUG-377** + **TASK-550** (PR-308) — `add_comment` and `triage_finding` had `author`/`content` argument inversion (silent data loss). Both fixed in one PR after Codex systematically tested every MCP write tool.
- **TASK-538** (PR-297) — MCP `history` tool exposed (parity with `aida history` CLI)
- **TASK-551** (PR-310) — MCP `add_relationship` tool exposed (CLI parity gap found during BUG-377 inventory)
- **BUG-381** (PR-311) — `list_requirements` filter normalization (`in-progress` / `InProgress` / `In Progress` all match)

**Multi-agent coordination**:
- **TASK-515** (PR-298) — `aida status` Active Agents lease-fallback for raw-launched agents
- **TASK-541** (PR-313) — `aida brief --depends-on` for explicit pickup-order constraints
- **TASK-542** + **STORY-459** (PR-314, bundled scope) — `aida agent new --name` + auto-seq + prefix-match brief routing. First validated multi-spec trailer convention (`(TASK-542 STORY-459)` in squash subject).
- **TASK-543** (PR-302) — `aida agent register <pid>` to backfill raw-launched agents into registry
- **TASK-557** (PR-317) — per-agent default flags from `.aida/agents.toml` (eliminates `--dangerously-skip-permissions` re-typing)

**Agent-launch UX**:
- **TASK-554** (PR-316) — `--spec` launches show explicit scope-binding text in context snapshot
- **TASK-555** (PR-322) — cross-agent skill-invocation surface map in `docs/agents/cross-agent-onboarding.md`
- **TASK-553** (PR-323) — `implementer-discipline.md` discipline doc articulating the six rules (each linked to the substrate bouncer that enforces it)
- **TASK-556** (PR-321) — `aida agent new --prompt` auto-injects first-message directive when `--spec` is present

**Status surfaces**:
- **STORY-385** (PR-296) — `aida status --cleanup` surfaces 8 attention-state categories
- **BUG-380** (PR-307) — `aida show` resolves default branch dynamically (unblocks repos with `master` default)

**Substrate-tools**:
- **STORY-467** (PR-320) — `aida findings add` for advisor-driven observation entry + `findings recur` counter. Recurrence ≥ 3 = promotion signal. Dogfooded with three real observations in the same session (TASK-1-088/089/090, all related to ceiling-pattern recurrences).

**Cleanup + scaffolding**:
- **BUG-375** (PR-294) — Codex SKILL.md YAML frontmatter for ~18 templates
- **TASK-540** (PR-293) — `codex-mcp-setup.md` template sync
- **TASK-547** (PR-318) — `aida queue work` auto-pulls Approved-not-queued specs into the queue
- **TASK-552** (PR-315) — 8 dead-code warnings audited (allow + intent comments)
- **TASK-549** (PR-319) — MCP stdio tests gained resources/list, resources/read, isError envelope coverage

**Previously-stuck**:
- **STORY-305** (PR-210) — `local/` + `.local.md` skill extensions finally landed after agy2-integrator rebased + cleared the RequestChanges verdict

### Substrate observations + pattern discovery

Filed 7+ observation recurrences on STORY-467 (using comment-carrier pattern until `aida findings add` shipped, then dogfooded the verb). Key patterns surfaced:

- **Agent claims complete without commit/push/PR** — local-tests-pass conflated with shipped (recurrence-7; agy1's repeat pattern)
- **Stale-scratchpad-loop on resumption** — Antigravity re-reads old session's `task.md` and reports "all done" instead of polling AIDA brief surface
- **Unauthorized pickup → failure-and-shelve** — overnight agent picked up STORY-465 outside briefed batch, errored, EPIC-28 correctly shelved it as NeedsAttention; BUG-379 then correctly refused fresh pickup
- **Non-atomic auto-bump** — BUG-379's status transition isn't transactional with lease creation; if lease fails, status stays In Progress with no lease (filed as BUG-384)
- **Cache lock contention** — schema-apply holds the lock 10-15s+; STORY-463's default retry budget (50/200/500ms) is too short; manual env var bump (`AIDA_CACHE_RETRY_MS=2000 AIDA_CACHE_RETRY_COUNT=10`) needed under heavy parallel writes (TASK-558 filed to bump defaults)

### Multi-advisor pattern validated

aida-chat advisor (separate Claude session in `~/ai/aida-chat`) bootstrapped + ran day-1 autonomously: verdicted Codex's EPIC-16 PR-1 work (4 must-fix items, all confirmed), reset STORY-21/22 to In Progress when implementers shipped backend-only (subset-ship discipline), drove the 5-PR merge cascade in aida-chat, recorded the killer demo. Master never touched aida-chat code. Cross-project sync via paste-ready prompts the operator forwarded.

### Tomorrow's queue (Session-58 leaves behind)

Five batches tagged for tomorrow's dispatch:

- **batch:queue-work-reliability** (14 specs) — master's stated concern; BUG-384, TASK-558, TASK-559, TASK-560, TASK-561 + 9 existing related specs. Codex briefed for top 4 high-priority.
- **batch:status-surfaces** (6 specs) — STORY-456/457/464/465 + TASK-539 + STORY-405
- **batch:agent-launch-ux** (5 specs, 3 shipped tonight) — TASK-553/555 ✅, TASK-554/556/557 ✅
- **batch:substrate-architecture** (4 specs) — STORY-460/469/471 + STORY-439 (keystone work)
- **batch:backlog-grooming** (8 specs) — `aida backlog groom` family + TASK-537

Briefs filed for tomorrow morning: Codex (BUG-384, TASK-558, TASK-559, TASK-561) + Antigravity (TASK-553, TASK-555, STORY-464, TASK-560, BUG-366). agy2 continues on integrator standby.

### Strategic shifts from the night

1. **Multi-spec trailer convention validated** — `(SPEC-A SPEC-B)` in squash subject auto-bumps both. PR-314 was the first deliberate use; the auto-bump scanner caught both correctly.
2. **Substrate-as-bouncer principle operationalized** — four runtime banners now in production. Empirical evidence STORY-469 (structural guards) is the right architectural shape: boundary-event bouncers catch most cases; need proactive divergence detection for the rest.
3. **Multi-advisor pattern empirically works** — aida-chat advisor ran day-1 autonomously; master stayed in cross-project strategic mode. Validates the "one master advisor until subsystems emerge" scaling principle.
4. **The substrate is approaching first-user-ready** — ceiling-pattern net + doctor + lock retry + multi-agent visibility + observation capture all shipped in one night. STORY-465 (Awaiting you status section) closes the human-gate visibility gap; in flight as of session end.

---

## Session 59 — Autonomous competitive-strategy night (2026-05-30, `/loop`)

**Request.** Two escalating operator mandates, then a self-paced `/loop`: (1) charge-forward — *"stop relying on me … use your best judgement and keep cranking out updates … you are losing ground by constantly waiting"*; (2) the competitive mandate — *"you are in a very competitive race … understand the competition and see if the git-canonical approach can result in an architecture more amenable to many use cases … multi-vendor will be something we can excel at … Git has emerged as a standard and by not riding on it [competitors] may be missing the common infrastructure that will underpin the knowledge substrate … show leadership and relentless diligence … churn constantly"*; (3) `/loop` (dynamic mode): *"do what you can while I am asleep."*

**What was done — competitive intelligence → strategy → dispatch.**
- **Four parallel research deep-dives** (Spec Kit, the multi-agent-coordination frontier, AGENTS.md/MCP convergence, git-canonical beyond-software use cases) synthesized into a decision-grade picture.
- **Round-1 thesis** (`docs/competitive-analysis/2026-05-31-git-canonical-substrate-thesis.md`, SPIKE-43): graded the operator's "competitors avoid git, we ride it" framing as category-dependent and *dangerous in its naive form* (false against Spec Kit). Reframed the wedge to the **stable-ID + typed-relationship + trace-enforced graph on git** no competitor combines; multi-vendor portability as the strongest, incentive-anchored claim. Shipped honesty correction: AIDA = git writer-of-record + SQLite cache + file handshakes, *not* git purity.
- **Round-2 synthesis** (`2026-05-31-round2-moat-gaps-moves.md`): the commoditized-vs-differentiated split (AGENTS.md + MCP + worktree-isolation are commoditized/table-stakes; stable IDs + typed graph + trace enforcement + lifecycle + orchestrated drain + multi-vendor are the moat), the prioritized capability roadmap (P1 resumable checkpointing … P5 drain legibility + AGENTS.md-generator + ReqIF option), use-case verdicts (PURSUE ADRs-as-graph as a feature / WATCH compliance-as-code + portable memory / AVOID ELN-wiki-PKM), the positioning line that survives the convergence, and the tripwires. **Verdict: the real risk is distribution, not differentiation.**
- **Positioning docs for the two nearest competitors** — the `docs/positioning/` surface had *zero* entries for them: wrote `vs-spec-kit.md` and `vs-kiro.md` (precise, not overclaimed — credits Spec Kit's within-feature IDs + `/speckit.analyze` + ~100× distribution and Kiro's EARS rigor + task→requirement traceability; locates AIDA's delta in the maintained cross-cutting graph + multi-vendor portability), indexed both first in the positioning README.
- **Discoverability + accuracy fixes**: CLAUDE.md positioning pointer now leads with the two nearest competitors + names the round-2 synthesis; competitive-analysis README indexes the round-1/round-2 docs at top; **corrected OVERVIEW.md's niche statement** — it had framed Spec Kit's workflow as the *ceiling* with AIDA *between* it and the Karpathy floor (backwards: Spec Kit is a peer competitor whose per-feature frozen artifacts lack the graph AIDA adds on top).
- **Roadmap + fleet dispatch**: filed **SPIKE-45** (capability roadmap, P1–P5 + AGENTS.md-generator + ReqIF) and **STORY-489** (the flagship `aida graph` cross-spec query surface — the query Kiro/Spec Kit's flat markdown structurally can't answer); briefed STORY-489 to Codex (sketch-first since it touches the MCP contract). Codex also holds SPIKE-44 (multi-vendor substrate access) + its SPIKE queue; AGY holds TASK-590 (format-symbol inventory).

**Git operations.** Shipped via `aida pr ship` (CI-watch → squash-merge → auto-bump → cleanup): PR-378 (round-2 synthesis), PR-379 (vs-spec-kit), PR-380 (vs-kiro), PR-381 (CLAUDE.md pointer), PR-382 (competitive-analysis index). All auto-bumped their specs to Completed on merge. OVERVIEW.md correction + this PROMPT_HISTORY entry shipped at session close.

**Documentation updates.** 2 new competitive-analysis docs, 2 new positioning docs, README indexes (positioning + competitive-analysis), CLAUDE.md + OVERVIEW.md pointers/accuracy, this log.

**Discipline notes.** No unsafe autonomous fleet launches: STORY-489 touches the MCP contract → correctly left for Codex's *supervised* sketch-first pickup rather than an overnight headless build (per the one-master-advisor + AGY/Codex dispatch policies). No new releases cut. Solo work stayed on reliable, low-risk, non-duplicative doc/strategy output; feature builds left to the dispatched fleet.

---

## Session 60 — Autonomous build night: the graph-query flagship, end to end (2026-05-31, `/loop` ×6)

**Request.** The operator re-invoked `/loop` (same competitive mandate) repeatedly through the night — each re-launch a signal to keep producing, not consolidate. Net read: build continuously; treat "frontier reached" as the reflex to overcome, while keeping quality gates.

**What was built + shipped (15 PRs merged, 378–392 range).** Pattern that emerged: *build design-settled / pure / additive work; reserve control-flow-modifying wiring and genuine design decisions for operator sign-off.*
- **The `aida graph` flagship — STORY-489, complete (CLI + MCP), built slice by slice:**
  - Slice 1 (TASK-594): `aida-core/src/graph_walk.rs` — cycle-safe transitive relationship walk (`walk` + `status_rollup`) over the existing `get_relationships_by_type`; 6 unit tests.
  - Slice 2 (TASK-595): the `aida graph <SPEC>` CLI — `--blocked-by`/`--blocks`/`--tree`/`--impact`/`--json`, mode-exclusion guard, wired into the git-backend dispatch. Verified e2e: `aida graph STORY-276 --blocked-by` → STORY-332.
  - Slice 3 (TASK-597): the `query_graph` MCP tool — same logic for any MCP client (the multi-vendor half of the moat). Verified e2e over JSON-RPC stdio.
  - Regression tests (TASK-599): functional + descriptor coverage for query_graph.
  - **This is the moat-demo no flat-markdown SDD tool (Spec Kit, Kiro) can answer.**
- **P5 drain legibility (STORY-490):** `⚠ N shelved` callout in `aida queue progress` (additive; preserves STORY-332 bucketing).
- **P1 resumable drain (STORY-491 plan + TASK-598 slice 1):** the sketch-first plan (reconcile-from-reality design + double-drive guard surfaced for sign-off) + `aida-cli/src/drain_resume.rs` pure decision core (`classify_resumability` + `reconcile_resume_phase`, 8 tests). Slice 2 (probing + live `--resume` wiring) is sign-off-gated → tracked fresh as STORY-492.
- **BUG-51:** commit-msg validator now accepts version/phase suffixes in the REQ-ID — `(EPIC-19 v1)` — at the correct hook (`aida-core/templates/hooks/aida-commit-msg`); regression cases added.

**Bugs filed (for fresh capacity / sign-off, not force-fixed at fatigue):** BUG-408 (`agent new --show-context` not dry — needs a `prepare_agent_launch` dry-mode refactor), BUG-409 (flaky `story_429` git-test — unreproducible locally), BUG-410 (auto-bump re-completes a manually-reopened spec — scanner gap), TASK-593 (`queue prune` misses merged-PR review rows), TASK-596 (`pr ship` bails on not-yet-registered CI).

**Recoveries (5, all handled + captured as substrate):** three CI round-trips (fmt drift via a pipe-masked exit code; a `///`-doc trace marker leaking into `--help`; the MCP doc-consistency gate) → hardened `feedback_verify_edits_landed_before_claiming_done` (run the FULL CI step set from `ci.yml`, not a narrow subset; capture exit codes, never pipe a check to `tail`). One flaky-test rerun (BUG-409). One lifecycle slip: a plan commit's `(STORY-491 …)` trailer auto-completed the umbrella, and `--force` re-open did NOT stick (re-bumps) → new memory `feedback_commit_trailer_completes_the_spec` + BUG-410.

**Judgment notes.** Corrected an early over-caution (pure decision logic was safe to build, not sign-off-gated — only the live wiring is). Held the line at deep fatigue: did **not** force BUG-408's refactor or BUG-59's marginal/upstream cosmetic at ~5am after 5 recoveries — that would trade the quality that earned the night's wins for motion. The remaining high-value work genuinely needs operator sign-off (P1 slice 2 reconcile design, P3 mailbox-vs-briefs) or fresh-session capacity (the refactor-class bugs). No releases cut.

**For the operator on waking — highest leverage:** (1) sign off P1 slice 2's reconcile-from-reality design + double-drive guard (STORY-492, plan in `docs/plans/2026-05-31-p1-resumable-drain-checkpointing.md`); (2) decide P3 mailbox: extend the brief system vs a separate store (slop call); (3) launch Codex on its queue (STORY-489 is now done by master — its slice-3 brief was acked moot). Fleet briefs remain for Codex/AGY.

---

## Session 61 — Autonomous `/loop` night: ADR-2/ADR-3, the GitLab hint layer, and a slice-1b correction (2026-06-04 → 06-05, `/loop` self-paced)

**Request.** A self-paced `/loop`: "finish slice-1b merge-op routing and all related gitlab support tasks, empty the queue, then groom the backlog 5-at-a-time until nothing workable remains." Standing license: charge forward autonomously, keep quality gates, GitLab mandatory. Operator away overnight.

**What shipped (14 PRs merged + 2 stories completed).** Cadence each iteration: merge prior PR (CI-gated) → sync → work next → PR → `ScheduleWakeup`.
- **ADR-2 role onboarding:** TASK-644 (#500, interactive role picker, PTY-verified), TASK-645 (#501, implementer = read-side default + statusline `(default)` + init roles block), TASK-646 (#502, `aida agent new` role prompt, no parent-role inheritance).
- **ADR-3 intake-triage:** TASK-647 (#503, the **advisor-gate** — `add`/`edit`/`queue add`/MCP gated to advisor-role-or-TTY; substrate-as-bouncer; verified it correctly governs my own loop), TASK-648 (#504, `/aida-triage` draft-inbox clearer + `aida status` "Inbox: N" + advisor statusline `inbox:N`).
- **EPIC-35 GitLab hint layer — STORY-508 COMPLETE (slice 2):** TASK-650 (#506, `change_noun`/`change_cmd_hint` helper + workflow_hints), TASK-652 (#507, per-forge command vocabulary capturing glab's real divergences — pipeline-scoped CI, `--remove-source-branch` — + cleanup-report), TASK-653 (#508, orchestrator recovery hints), TASK-654 (#509, pr-ship dry-run). Verified an **empty actionable-hint grep** across all surfaces, then closed STORY-508 + TASK-651. GitHub output byte-for-byte throughout.
- **Other:** TASK-394 (#499, persistent `--no-human` ack marker), TASK-415 (#498, skill State-preamble → `aida state-snapshot`), TASK-502 (#505, `aida brief --notify` sentinel bridge), TASK-472 (#510, `file_finding` ≠ session-journal docs, project + template), BUG-432 (#511, `db sync --pull` skips gracefully with no origin — pure-git/fresh-init usability). TASK-395 closed as **superseded** by the existing `aida headless tail` (verify-first; no duplicate built).

**The slice-1b correction (the loop's lead directive).** Verifying "finish slice-1b merge-op routing" surfaced that **STORY-516 was mis-bumped Completed** — PR-483's trailer carried only the *prep* (MergeOptions + PureGitForge squash fix), not plan item #2 (the routing). The forge layer is **plumbed but not wired**: GitHubForge/PureGitForge real, hint TEXT forge-aware, but the ~113 real `gh` call sites still invoke `gh` directly (`forge.merge_change` has only a test caller) — so GitLab/pure-git do **not** drive the lifecycle yet. Reopened STORY-516 + captured `project_forge_plumbed_not_wired` memory. **Did NOT route it overnight** — reliability-critical (merge/ship/drain); STORY-516's own safety note + `feedback_reliability_fixes_use_keyboard_not_drain` say at-keyboard-incremental, not blind drain.

**Judgment notes.** Two-track posture: ship safe/bounded work autonomously (role onboarding, the gate, the whole hint layer, docs, a robustness bug); leave reliability-critical wiring for the keyboard. Verify-first repeatedly paid off (TASK-395 OBE; the STORY-516 mis-bump; STORY-508's "Size S"→M rescope). Rode through a flaky CI test (`task_471_stale_base_preflight`, rerun-on-same-commit). New memories: `feedback_advisor_role_for_queue_after_647`, `project_forge_plumbed_not_wired`. No releases cut; no unsafe overnight orchestrator rewiring.

**For the operator on waking — highest leverage:** (1) **the mandatory GitLab routing (STORY-516, reopened) needs the keyboard** — route the ~113 gh sites through the Forge trait per-op in SPIKE-49 order (merge first), each its own PR, GitHub argv byte-identical, verified on a real PR; then STORY-509 (glab impls) / 510 (CI) / 511 (e2e) need a live GitLab repo + `glab`. (2) The safe-autonomous backlog vein drained to marginal Low items (BUG-59 cosmetic, BUG-418 unconfirmed-needs-instrumentation, BUG-433 risky backend) — a fresh pass, not overnight-marginal. (3) Demo (STORY-497/TASK-627) stays deferred per the bugs-before-marketing phase.
