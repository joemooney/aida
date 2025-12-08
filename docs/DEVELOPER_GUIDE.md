# AIDA Developer's Guide

A comprehensive guide for developers maintaining and extending the AIDA (AI Design Assistant) requirements management system.

**Related Documentation:**
- [User Guide](user-guide.md) - End-user documentation for using AIDA
- [Administrator's Guide](admin-guide.md) - For project configuration and administration

## Table of Contents

1. [Project Overview](#1-project-overview)
2. [Project Structure](#2-project-structure)
3. [Core Data Model](#3-core-data-model)
4. [GUI Architecture](#4-gui-architecture)
5. [Layout System](#5-layout-system)
6. [Theme System](#6-theme-system)
7. [State Management](#7-state-management)
8. [Keyboard System](#8-keyboard-system)
9. [Filtering & Perspectives](#9-filtering--perspectives)
10. [Configuration & Persistence](#10-configuration--persistence)
11. [Common Development Tasks](#11-common-development-tasks)
12. [Code Patterns & Conventions](#12-code-patterns--conventions)
13. [Troubleshooting](#13-troubleshooting)
14. [Code Traceability & Git Hooks](#14-code-traceability--git-hooks)

---

## 1. Project Overview

AIDA is a Rust-based requirements management system with both CLI and GUI interfaces. It stores requirements as YAML files and supports:

- Hierarchical requirements with parent/child relationships
- Multiple requirement types (Functional, User Story, Epic, Bug, etc.)
- Threaded comments with reactions
- Custom fields per requirement type
- Flexible filtering and multiple view perspectives
- Full theme customization
- Configurable keyboard shortcuts

### Technology Stack

| Component | Technology |
|-----------|------------|
| Language | Rust 2021 Edition |
| GUI Framework | egui 0.29 / eframe 0.29 |
| Serialization | serde / serde_yaml |
| Markdown | egui_commonmark |
| IDs | uuid v4 |
| Timestamps | chrono |

---

## 2. Project Structure

AIDA is a Cargo workspace with three crates:

```
aida/
├── Cargo.toml                 # Workspace definition
├── aida-core/                 # Shared library
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs             # Module exports
│       ├── models.rs          # Core data structures (~3500 lines)
│       ├── storage.rs         # YAML file I/O
│       ├── registry.rs        # Multi-project management
│       ├── project.rs         # Project resolution
│       ├── export.rs          # Export functionality
│       └── scaffolding.rs     # Claude Code scaffolding & git hooks
├── aida-cli/                  # Command-line interface
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── cli.rs             # Clap argument definitions
│       └── prompts.rs         # Interactive prompts
├── aida-gui/                  # GUI application
│   ├── Cargo.toml
│   ├── assets/                # Fonts and licenses
│   └── src/
│       ├── main.rs            # Entry point (~20 lines)
│       └── app.rs             # Main application (~13,400 lines)
└── docs/
    ├── DEVELOPER_GUIDE.md     # This file
    ├── user-guide.md          # End-user documentation
    └── RELATIONSHIP_DESIGN.md # Relationship system design
```

### Crate Dependencies

```
aida-gui ──depends──> aida-core
aida-cli ──depends──> aida-core
```

---

## 3. Core Data Model

### 3.1 Requirement Structure

Located in `aida-core/src/models.rs`:

```rust
pub struct Requirement {
    pub id: Uuid,                              // Primary key
    pub spec_id: Option<String>,               // Human-readable ID (e.g., "FR-0001")
    pub title: String,
    pub description: String,
    pub status: RequirementStatus,             // Draft, Approved, Completed, Rejected
    pub priority: RequirementPriority,         // High, Medium, Low
    pub req_type: RequirementType,             // Functional, Story, Epic, etc.
    pub owner: String,
    pub feature: String,                       // Grouping/categorization
    pub tags: HashSet<String>,
    pub dependencies: Vec<Uuid>,               // Legacy - use relationships instead
    pub relationships: Vec<Relationship>,      // Parent/Child/Verifies/etc.
    pub comments: Vec<Comment>,                // Threaded comments
    pub history: Vec<ChangeRecord>,            // Audit trail
    pub urls: Vec<UrlLink>,                    // External links
    pub custom_fields: HashMap<String, String>,
    pub created_at: DateTime<Local>,
    pub modified_at: DateTime<Local>,
    pub created_by: Option<String>,
    pub archived: bool,
}
```

### 3.2 Requirement Types

```rust
pub enum RequirementType {
    // Traditional
    Functional,      // FR-xxxx
    NonFunctional,   // NFR-xxxx
    System,          // SR-xxxx
    User,            // UR-xxxx

    // Project Management
    ChangeRequest,   // CR-xxxx
    Bug,             // BUG-xxxx

    // Agile
    Epic,            // EPIC-xxxx
    Story,           // STORY-xxxx
    Task,            // TASK-xxxx
    Spike,           // SPIKE-xxxx
}
```

### 3.3 Relationships

```rust
pub struct Relationship {
    pub relationship_type: RelationshipType,
    pub target_id: Uuid,
}

pub enum RelationshipType {
    Parent,
    Child,
    Duplicate,
    Verifies,
    VerifiedBy,
    References,
    Custom(String),  // User-defined
}
```

### 3.4 RequirementsStore

The container for all project data:

```rust
pub struct RequirementsStore {
    pub name: String,
    pub title: String,
    pub description: String,
    pub requirements: Vec<Requirement>,
    pub users: Vec<User>,
    pub features: Vec<Feature>,
    pub relationship_definitions: Vec<RelationshipDefinition>,
    pub type_definitions: Vec<CustomTypeDefinition>,
    pub id_config: IdConfiguration,
    pub next_feature_number: u32,
    pub next_spec_number: u32,
    pub prefix_counters: HashMap<String, u32>,
    // ... more fields
}
```

---

## 4. GUI Architecture

### 4.1 Application Entry Point

`aida-gui/src/main.rs`:

```rust
fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Aida - AI Design Assistant",
        options,
        Box::new(|cc| Ok(Box::new(RequirementsApp::new(cc)))),
    )
}
```

### 4.2 RequirementsApp Structure

The main application struct (`aida-gui/src/app.rs`) contains 200+ fields organized into logical groups:

```rust
pub struct RequirementsApp {
    // === Core State ===
    storage: Storage,
    store: RequirementsStore,
    current_view: View,
    selected_idx: Option<usize>,

    // === Layout ===
    layout_mode: LayoutMode,

    // === Form State ===
    form_title: String,
    form_description: String,
    form_status: RequirementStatus,
    // ... many more form fields

    // === Filtering ===
    filter_types: HashSet<RequirementType>,
    filter_features: HashSet<String>,
    filter_text: String,
    // ... filter fields for both root and children

    // === Settings ===
    user_settings: UserSettings,

    // === Pending Operations ===
    pending_delete: Option<usize>,
    pending_view_change: Option<View>,
    pending_save: bool,
    // ... more pending operations

    // === UI State ===
    show_settings: bool,
    show_filter_dialog: bool,
    tree_collapsed: HashMap<Uuid, bool>,
    // ... UI state fields
}
```

### 4.3 View Enum

```rust
pub enum View {
    List,    // Requirements list (default)
    Detail,  // Single requirement details
    Add,     // New requirement form
    Edit,    // Edit existing requirement
}
```

### 4.4 Main Update Loop

The `eframe::App::update()` method follows this pattern:

```rust
impl eframe::App for RequirementsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. Apply theme and styles
        self.user_settings.theme.apply(ctx);
        self.configure_font_sizes(ctx);

        // 2. Handle keyboard input
        self.handle_keyboard_input(ctx);

        // 3. Process pending operations
        self.process_pending_operations();

        // 4. Render UI based on layout mode
        match self.layout_mode {
            LayoutMode::ListDetailsSide => self.render_side_layout(ctx),
            LayoutMode::ListDetailsStacked => self.render_stacked_layout(ctx),
            LayoutMode::SplitListDetails => self.render_split_layout(ctx),
            LayoutMode::SplitListOnly => self.render_split_list_only(ctx),
            LayoutMode::ListOnly => self.render_list_only(ctx),
        }

        // 5. Render dialogs/modals
        self.render_dialogs(ctx);
    }
}
```

---

## 5. Layout System

### 5.1 Layout Modes

AIDA supports 5 distinct layout configurations:

```rust
pub enum LayoutMode {
    ListDetailsSide,    // List left, Details right (default)
    ListDetailsStacked, // List top, Details bottom
    SplitListDetails,   // Two lists top, Details bottom
    SplitListOnly,      // Two lists side-by-side
    ListOnly,           // Single list, no details
}
```

### 5.2 Layout Implementation Patterns

#### Side-by-Side Layout (ListDetailsSide)

Uses `ui.columns()` for equal-width columns:

```rust
fn render_side_layout(&mut self, ctx: &egui::Context) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.columns(2, |columns| {
            // Left column: List
            columns[0].push_id("list_column", |ui| {
                self.show_list_panel(ui);
            });

            // Right column: Details
            columns[1].push_id("detail_column", |ui| {
                self.show_detail_view_internal(ui, false);
            });
        });
    });
}
```

#### Stacked Layout (ListDetailsStacked)

Uses `TopBottomPanel` for resizable vertical split:

```rust
fn render_stacked_layout(&mut self, ctx: &egui::Context) {
    // Top panel: List (resizable, 40% default)
    egui::TopBottomPanel::top("list_panel")
        .resizable(true)
        .default_height(ctx.screen_rect().height() * 0.4)
        .min_height(100.0)
        .show(ctx, |ui| {
            self.show_list_panel(ui);
        });

    // Central panel: Details (remaining space)
    egui::CentralPanel::default().show(ctx, |ui| {
        self.show_detail_view_internal(ui, true); // stacked=true
    });
}
```

### 5.3 Stacked Detail View Layout

When `stacked=true`, the detail view uses a horizontal split:

```rust
fn show_detail_view_internal(&mut self, ui: &mut egui::Ui, stacked: bool) {
    // Title bar with styled background
    egui::Frame::none()
        .fill(self.user_settings.theme.title_bar_bg())
        .show(ui, |ui| {
            // Title and action buttons
        });

    if stacked {
        // LEFT: Metadata panel (25% width, resizable)
        egui::SidePanel::left("detail_metadata_panel")
            .resizable(true)
            .default_width(available_width * 0.25)
            .show_inside(ui, |ui| {
                // ID, Status, Priority, Type, etc.
            });

        // RIGHT: Tabs and content (75% width)
        egui::CentralPanel::default()
            .show_inside(ui, |ui| {
                // Tab bar: Description, Comments, Links, History
                // Tab content with ScrollArea
            });
    } else {
        // Vertical layout: metadata grid, then tabs below
    }
}
```

### 5.4 Panel Types Reference

| egui Panel | Use Case |
|------------|----------|
| `TopBottomPanel::top()` | Fixed or resizable top section |
| `TopBottomPanel::bottom()` | Fixed bottom section |
| `SidePanel::left()` | Fixed or resizable left section |
| `SidePanel::right()` | Fixed or resizable right section |
| `CentralPanel` | Fills remaining space (use last) |

### 5.5 Preventing Content Overflow

When using `columns()`, set clip rectangles to prevent overflow:

```rust
ui.columns(2, |columns| {
    let clip_rect = columns[0].clip_rect();
    columns[0].set_clip_rect(clip_rect);

    // Render content that respects boundaries
});
```

---

## 6. Theme System

### 6.1 Theme Enum

```rust
pub enum Theme {
    Dark,                        // Default dark theme
    Light,                       // Light theme
    HighContrastDark,            // High contrast for accessibility
    SolarizedDark,               // Solarized color scheme
    Nord,                        // Nord color scheme
    Custom(Box<CustomTheme>),    // Fully customizable
}
```

### 6.2 CustomTheme Structure

```rust
pub struct CustomTheme {
    // Background colors
    pub window_fill: ThemeColor,
    pub panel_fill: ThemeColor,
    pub extreme_bg: ThemeColor,
    pub faint_bg: ThemeColor,

    // Text colors
    pub text_color: ThemeColor,
    pub hyperlink_color: ThemeColor,
    pub warn_fg: ThemeColor,
    pub error_fg: ThemeColor,

    // Widget colors (multiple states)
    pub widget_bg: ThemeColor,
    pub widget_inactive_bg: ThemeColor,
    pub widget_hovered_bg: ThemeColor,
    pub widget_active_bg: ThemeColor,

    // Selection
    pub selection_bg: ThemeColor,
    pub selection_fg: ThemeColor,

    // Strokes and borders
    pub widget_stroke_width: f32,
    pub widget_stroke_color: ThemeColor,

    // Visual properties
    pub widget_rounding: f32,
    pub window_rounding: f32,
    pub item_spacing: (f32, f32),

    // Detail view title bar
    pub title_bar_bg: ThemeColor,
    pub title_bar_text: Option<ThemeColor>,
    pub title_bar_font_size: f32,

    pub dark_mode: bool,
}
```

### 6.3 Theme Application

Themes are applied each frame in the update loop:

```rust
impl Theme {
    pub fn apply(&self, ctx: &egui::Context) {
        match self {
            Theme::Dark => ctx.set_visuals(egui::Visuals::dark()),
            Theme::Light => ctx.set_visuals(egui::Visuals::light()),
            Theme::Custom(custom) => {
                let mut visuals = if custom.dark_mode {
                    egui::Visuals::dark()
                } else {
                    egui::Visuals::light()
                };

                // Apply all custom colors
                visuals.window_fill = custom.window_fill.to_egui();
                visuals.panel_fill = custom.panel_fill.to_egui();
                // ... apply all properties

                ctx.set_visuals(visuals);
            }
            // Other themes...
        }
    }
}
```

### 6.4 Adding a New Built-in Theme

1. Add variant to `Theme` enum
2. Implement in `Theme::apply()` match arm
3. Add to theme selection UI in settings
4. Implement helper methods (`title_bar_bg()`, etc.)

```rust
// Example: Adding a "Monokai" theme
pub enum Theme {
    // ... existing
    Monokai,
}

impl Theme {
    pub fn apply(&self, ctx: &egui::Context) {
        match self {
            Theme::Monokai => {
                let mut visuals = egui::Visuals::dark();
                visuals.window_fill = egui::Color32::from_rgb(39, 40, 34);
                visuals.panel_fill = egui::Color32::from_rgb(39, 40, 34);
                // ... set Monokai colors
                ctx.set_visuals(visuals);
            }
            // ...
        }
    }

    fn title_bar_bg(&self) -> egui::Color32 {
        match self {
            Theme::Monokai => egui::Color32::from_rgb(49, 50, 44),
            // ...
        }
    }
}
```

---

## 7. State Management

### 7.1 Pending Operations Pattern

To avoid borrow checker conflicts during the update loop, mutations are queued:

```rust
// Instead of immediate mutation:
// self.store.requirements.remove(idx);  // Can't borrow mutably while iterating

// Queue the operation:
self.pending_delete = Some(idx);

// Process after UI rendering:
fn process_pending_operations(&mut self) {
    if let Some(idx) = self.pending_delete.take() {
        self.store.requirements.remove(idx);
        self.save();
    }

    if let Some(view) = self.pending_view_change.take() {
        self.current_view = view;
    }

    if self.pending_save {
        self.pending_save = false;
        self.save();
    }
}
```

### 7.2 Form State Management

```rust
// Load requirement into form
fn load_form_from_requirement(&mut self, idx: usize) {
    if let Some(req) = self.store.requirements.get(idx) {
        self.form_title = req.title.clone();
        self.form_description = req.description.clone();
        self.form_status = req.status.clone();
        // ... copy all fields

        self.store_original_form_values(); // For cancel/undo
    }
}

// Clear form for new requirement
fn clear_form(&mut self) {
    self.form_title = String::new();
    self.form_description = String::new();
    self.form_status = RequirementStatus::Draft;
    // ... reset all fields
}

// Save form to requirement
fn save_form_to_requirement(&mut self, idx: usize) {
    if let Some(req) = self.store.requirements.get_mut(idx) {
        req.title = self.form_title.clone();
        req.description = self.form_description.clone();
        // ... copy all fields
        req.modified_at = Local::now();
    }
}
```

### 7.3 Selection State

```rust
// Primary list selection
selected_idx: Option<usize>,

// Split view: second list selection
split_selected_idx: Option<usize>,

// Which list has keyboard focus
focused_list: FocusedList,  // List1 or List2

// Lock navigation between lists
navigation_locked: bool,
```

---

## 8. Keyboard System

### 8.1 Key Context

```rust
pub enum KeyContext {
    Global,           // Always active
    RequirementsList, // When viewing list
    DetailView,       // When viewing details
    Form,             // When in add/edit form
}
```

### 8.2 Key Actions

```rust
pub enum KeyAction {
    // Navigation
    NavigateUp, NavigateDown,
    PageUp, PageDown,
    Home, End,

    // Views
    EnterList, EnterDetail, EnterAdd, EnterEdit,
    CloseDetail,

    // Actions
    Save, Delete, Archive,

    // Layout
    CycleLayout, ToggleFilterPanel,

    // Zoom
    ZoomIn, ZoomOut, ResetZoom,
    // ... more actions
}
```

### 8.3 KeyBinding Structure

```rust
pub struct KeyBinding {
    pub key: Option<egui::Key>,
    pub ctrl: bool,
    pub shift: bool,
    pub context: KeyContext,
}

impl KeyBinding {
    pub fn key(key: egui::Key, context: KeyContext) -> Self {
        Self { key: Some(key), ctrl: false, shift: false, context }
    }

    pub fn with_ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }

    pub fn matches(&self, input: &egui::InputState, current_context: KeyContext) -> bool {
        // Check context compatibility
        if !self.context_matches(current_context) {
            return false;
        }

        // Check key and modifiers
        if let Some(key) = self.key {
            input.key_pressed(key)
                && input.modifiers.ctrl == self.ctrl
                && input.modifiers.shift == self.shift
        } else {
            false
        }
    }
}
```

### 8.4 Adding a New Keyboard Shortcut

1. Add action to `KeyAction` enum
2. Add default binding in `KeyBindings::default()`
3. Handle the action in keyboard input processing

```rust
// 1. Add action
pub enum KeyAction {
    // ...
    ToggleDarkMode,
}

// 2. Add default binding
impl Default for KeyBindings {
    fn default() -> Self {
        let mut bindings = HashMap::new();
        // ...
        bindings.insert(
            KeyAction::ToggleDarkMode,
            KeyBinding::key(egui::Key::D, KeyContext::Global).with_ctrl()
        );
        Self { bindings }
    }
}

// 3. Handle in update loop
fn handle_keyboard_input(&mut self, ctx: &egui::Context) {
    let input = ctx.input(|i| i.clone());
    let context = self.current_key_context();

    if self.user_settings.keybindings.is_pressed(KeyAction::ToggleDarkMode, &input, context) {
        self.toggle_dark_mode();
    }
}
```

---

## 9. Filtering & Perspectives

### 9.1 Perspective System

```rust
pub enum Perspective {
    Flat,         // Simple list, no hierarchy
    ParentChild,  // Tree based on Parent/Child relationships
    Verification, // Tree based on Verifies/VerifiedBy
    References,   // Tree based on References relationships
}

pub enum PerspectiveDirection {
    TopDown,  // Parents at top, children below
    BottomUp, // Children at top, parents below
}
```

### 9.2 Two-Level Filtering

AIDA supports independent filters for root and child requirements:

```rust
// Root-level filters (top-level requirements in tree)
filter_types: HashSet<RequirementType>,
filter_features: HashSet<String>,
filter_prefixes: HashSet<String>,
filter_statuses: HashSet<RequirementStatus>,
filter_priorities: HashSet<RequirementPriority>,

// Child-level filters
children_filter_types: HashSet<RequirementType>,
children_filter_features: HashSet<String>,
// ... same fields with children_ prefix

// Option to use same filters for children
children_same_as_root: bool,
```

### 9.3 Filter Application

```rust
fn passes_filters(&self, req: &Requirement, is_root: bool) -> bool {
    let (types, features, statuses, priorities) = if is_root || self.children_same_as_root {
        (&self.filter_types, &self.filter_features,
         &self.filter_statuses, &self.filter_priorities)
    } else {
        (&self.children_filter_types, &self.children_filter_features,
         &self.children_filter_statuses, &self.children_filter_priorities)
    };

    // Check each filter
    (types.is_empty() || types.contains(&req.req_type))
        && (features.is_empty() || features.contains(&req.feature))
        && (statuses.is_empty() || statuses.contains(&req.status))
        && (priorities.is_empty() || priorities.contains(&req.priority))
        && self.passes_text_filter(req)
        && (self.show_archived || !req.archived)
}
```

### 9.4 Tree Rendering

```rust
fn show_tree_list(&mut self, ui: &mut egui::Ui) {
    // Find root requirements (no parents in current perspective)
    let roots = self.find_tree_roots();

    for root_idx in roots {
        if self.passes_filters(&self.store.requirements[root_idx], true) {
            self.show_tree_node(ui, root_idx, 0); // depth=0
        }
    }
}

fn show_tree_node(&mut self, ui: &mut egui::Ui, idx: usize, depth: usize) {
    let req = &self.store.requirements[idx];
    let children = self.get_children(idx);
    let has_children = !children.is_empty();

    // Indentation
    ui.add_space(depth as f32 * self.user_settings.theme.indent());

    // Collapse/expand toggle
    if has_children {
        let collapsed = self.tree_collapsed.get(&req.id).copied().unwrap_or(false);
        if ui.button(if collapsed { "▶" } else { "▼" }).clicked() {
            self.tree_collapsed.insert(req.id, !collapsed);
        }
    }

    // Render requirement item
    self.show_requirement_item(ui, idx);

    // Render children if expanded
    if has_children && !self.tree_collapsed.get(&req.id).copied().unwrap_or(false) {
        for child_idx in children {
            if self.passes_filters(&self.store.requirements[child_idx], false) {
                self.show_tree_node(ui, child_idx, depth + 1);
            }
        }
    }
}
```

---

## 10. Configuration & Persistence

### 10.1 User Settings

**Location**: `~/.local/share/aida-gui/settings.yaml` (XDG compliant)

```rust
pub struct UserSettings {
    pub name: String,
    pub email: String,
    pub handle: String,
    pub font_size: f32,           // Base font size (default 14.0)
    pub ui_heading_level: u8,     // 1-6 for UI headings
    pub perspective: Perspective,
    pub theme: Theme,
    pub keybindings: KeyBindings,
    pub show_status_icons: bool,
    pub status_icons: StatusIconConfig,
    pub priority_icons: PriorityIconConfig,
    pub view_presets: Vec<ViewPreset>,
}
```

### 10.2 Requirements Storage

**Format**: YAML file (typically `requirements.yaml`)

```yaml
name: "project-name"
title: "Project Title"
description: "Project description"

requirements:
  - id: "550e8400-e29b-41d4-a716-446655440000"
    spec_id: "FR-0001"
    title: "User Authentication"
    description: "Users must be able to log in"
    status: Draft
    priority: High
    req_type: Functional
    # ... more fields

users:
  - name: "John Doe"
    email: "john@example.com"

type_definitions:
  - name: "Functional"
    display_name: "Functional"
    prefix: "FR"
    statuses: [Draft, Approved, Completed, Rejected]
    built_in: true
```

### 10.3 Project Registry

**Location**: `~/.requirements.config`

```yaml
projects:
  my-project:
    path: /home/user/projects/my-project/requirements.yaml
    description: "My awesome project"
  another-project:
    path: /path/to/another/requirements.yaml
    description: "Another project"

default_project: my-project
```

---

## 11. Common Development Tasks

### 11.1 Adding a New Requirement Field

1. **Add to Requirement struct** (`aida-core/src/models.rs`):
```rust
pub struct Requirement {
    // ...
    pub new_field: String,
}
```

2. **Update serialization** (if needed, serde handles it automatically)

3. **Add to form state** (`aida-gui/src/app.rs`):
```rust
pub struct RequirementsApp {
    // ...
    form_new_field: String,
}
```

4. **Update form loading/saving**:
```rust
fn load_form_from_requirement(&mut self, idx: usize) {
    // ...
    self.form_new_field = req.new_field.clone();
}

fn save_form_to_requirement(&mut self, idx: usize) {
    // ...
    req.new_field = self.form_new_field.clone();
}
```

5. **Add to form UI**:
```rust
fn show_form(&mut self, ui: &mut egui::Ui) {
    // ...
    ui.label("New Field:");
    ui.text_edit_singleline(&mut self.form_new_field);
}
```

6. **Add to detail view** (if visible):
```rust
fn show_detail_view_internal(&mut self, ui: &mut egui::Ui, stacked: bool) {
    // In metadata grid:
    ui.label("New Field:");
    ui.label(&req.new_field);
    ui.end_row();
}
```

### 11.2 Adding a New Layout Mode

1. **Add to LayoutMode enum**:
```rust
pub enum LayoutMode {
    // ...
    NewLayout,
}
```

2. **Update cycle_layout()**:
```rust
fn cycle_layout(&mut self) {
    self.layout_mode = match self.layout_mode {
        // ...
        LayoutMode::ListOnly => LayoutMode::NewLayout,
        LayoutMode::NewLayout => LayoutMode::ListDetailsSide,
    };
}
```

3. **Add rendering function**:
```rust
fn render_new_layout(&mut self, ctx: &egui::Context) {
    // Implement layout using egui panels
}
```

4. **Add to update() match**:
```rust
match self.layout_mode {
    // ...
    LayoutMode::NewLayout => self.render_new_layout(ctx),
}
```

5. **Update layout icon/tooltip**:
```rust
fn layout_icon(&self) -> &str {
    match self.layout_mode {
        // ...
        LayoutMode::NewLayout => "🆕",
    }
}
```

### 11.3 Adding a New Dialog

1. **Add state fields**:
```rust
pub struct RequirementsApp {
    show_new_dialog: bool,
    new_dialog_field: String,
}
```

2. **Create rendering function**:
```rust
fn show_new_dialog(&mut self, ctx: &egui::Context) {
    if !self.show_new_dialog {
        return;
    }

    egui::Window::new("New Dialog")
        .collapsible(false)
        .resizable(true)
        .show(ctx, |ui| {
            ui.label("Field:");
            ui.text_edit_singleline(&mut self.new_dialog_field);

            ui.horizontal(|ui| {
                if ui.button("OK").clicked() {
                    // Process
                    self.show_new_dialog = false;
                }
                if ui.button("Cancel").clicked() {
                    self.show_new_dialog = false;
                }
            });
        });
}
```

3. **Call from update()**:
```rust
fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
    // ... main UI rendering

    // Dialogs (render last, on top)
    self.show_new_dialog(ctx);
}
```

4. **Add trigger** (button, menu item, or keyboard shortcut)

---

## 12. Code Patterns & Conventions

### 12.1 egui Immediate Mode Pattern

egui uses immediate mode - UI is rebuilt every frame:

```rust
// BAD: Storing widget state externally
if self.button_was_clicked {
    do_something();
}

// GOOD: Check response immediately
if ui.button("Click me").clicked() {
    do_something();
}
```

### 12.2 Borrowing Patterns

```rust
// BAD: Can't borrow mutably while iterating
for req in &self.store.requirements {
    if ui.button("Delete").clicked() {
        self.delete_requirement(req.id); // Borrow conflict!
    }
}

// GOOD: Collect indices, mutate after
let mut to_delete = None;
for (idx, req) in self.store.requirements.iter().enumerate() {
    if ui.button("Delete").clicked() {
        to_delete = Some(idx);
    }
}
if let Some(idx) = to_delete {
    self.store.requirements.remove(idx);
}

// BEST: Use pending operations pattern
if ui.button("Delete").clicked() {
    self.pending_delete = Some(idx);
}
```

### 12.3 ID Management

```rust
// Use push_id for widgets that might have duplicate labels
ui.push_id(req.id, |ui| {
    if ui.button("Edit").clicked() {
        // This button is uniquely identified
    }
});

// Or use id_salt for scroll areas, grids, etc.
egui::ScrollArea::vertical()
    .id_salt("unique_scroll_area_name")
    .show(ui, |ui| { /* ... */ });
```

### 12.4 Layout Building

```rust
// Horizontal layout
ui.horizontal(|ui| {
    ui.label("Label:");
    ui.text_edit_singleline(&mut self.field);
    if ui.button("Submit").clicked() { /* ... */ }
});

// Vertical layout (default)
ui.vertical(|ui| {
    ui.label("Line 1");
    ui.label("Line 2");
});

// Grid layout
egui::Grid::new("my_grid")
    .num_columns(2)
    .spacing([10.0, 6.0])
    .striped(true)
    .show(ui, |ui| {
        ui.label("Key:");
        ui.label("Value");
        ui.end_row();
    });
```

---

## 13. Troubleshooting

### 13.1 Common Issues

#### Content Clipping in Layouts

**Problem**: Content is cut off or not visible

**Solution**: Check for constraining wrappers like `ui.horizontal()` around panels:

```rust
// BAD: horizontal() constrains height to single row
ui.horizontal(|ui| {
    egui::SidePanel::left("panel").show_inside(ui, |ui| {
        // Content clipped!
    });
});

// GOOD: Panels don't need horizontal wrapper
egui::SidePanel::left("panel").show_inside(ui, |ui| {
    // Full height available
});
```

#### Duplicate Widget IDs

**Problem**: Widgets don't respond or behave strangely

**Solution**: Ensure unique IDs:

```rust
// Use push_id with unique value
for (i, item) in items.iter().enumerate() {
    ui.push_id(i, |ui| {
        ui.button("Click"); // Now unique per iteration
    });
}
```

#### Borrow Checker Conflicts

**Problem**: Cannot borrow `self` mutably while iterating

**Solution**: Use pending operations pattern (see 7.1)

### 13.2 Debugging Tips

1. **Print Debug**: Use `dbg!()` macro or `println!()` for quick debugging

2. **egui Inspector**: Enable egui's built-in debug tools:
```rust
ctx.set_debug_on_hover(true);
```

3. **Visual Debugging**: Add colored frames to see layout boundaries:
```rust
egui::Frame::none()
    .fill(egui::Color32::RED.linear_multiply(0.1))
    .show(ui, |ui| {
        // Content to debug
    });
```

### 13.3 Performance Considerations

1. **Avoid cloning large data** in the render loop
2. **Use `ScrollArea`** for long lists
3. **Collapse complex UI** when not visible
4. **Cache computed values** when possible

---

## 14. Code Traceability & Git Hooks

AIDA supports inline code traceability comments and can generate Git hooks to validate them.

### 14.1 Trace Comment Format

When implementing requirements, add inline trace comments to link code back to requirement IDs:

```rust
// trace:FR-0042 | ai:claude:high
fn implement_feature() {
    // Implementation
}
```

**Format:** `// trace:<SPEC-ID> | ai:<tool>:<confidence>`

| Component | Values | Description |
|-----------|--------|-------------|
| `SPEC-ID` | FR-0042, SR-0001, etc. | Requirement identifier |
| `tool` | claude, copilot, chatgpt | AI tool that generated the code |
| `confidence` | high, med, low | Level of AI contribution |

**Confidence Levels:**
- `high`: >80% AI-generated code
- `med`: 40-80% AI with human modifications
- `low`: <40% AI, mostly human written

**Language-specific comment styles:**
- Rust/C/JavaScript: `// trace:...`
- Python/Shell: `# trace:...`
- HTML: `<!-- trace:... -->`

### 14.2 Git Hooks

AIDA scaffolding can generate Git hooks to validate code traceability.

#### commit-msg Hook

Validates commit messages when committing files with trace comments:

- **Warns** if staged files have trace comments but commit lacks `[AI:tool:conf]` tag
- **Suggests** adding `Trace: SPEC-ID` for requirements referenced in staged files
- **Non-blocking** by default (warnings only)

**Suggested commit message format:**
```
[AI:claude:high] feat(auth): add JWT refresh endpoint

Trace: FR-0042, FR-0043
```

#### pre-commit Hook

Validates trace comments before allowing commits:

- **Checks** that all referenced spec IDs exist in the AIDA database
- **Warns** about orphaned trace comments
- **Non-blocking** by default

### 14.3 Enabling Git Hooks

Git hooks are generated as part of project scaffolding:

1. Open AIDA GUI
2. Go to **Settings > AI tab**
3. Click **"Scaffold Project"**
4. Enable **"Generate Git Hooks"** checkbox
5. Optionally enable **"Include pre-commit hook"**
6. Click **"Apply"**

Or use the `ScaffoldConfig` programmatically:

```rust
use aida_core::scaffolding::{ScaffoldConfig, Scaffolder};

let config = ScaffoldConfig {
    generate_git_hooks: true,
    include_commit_msg_hook: true,
    include_pre_commit_hook: true,
    ..Default::default()
};

let scaffolder = Scaffolder::new(project_root, config);
let preview = scaffolder.preview(&store);
scaffolder.apply(&preview)?;
```

### 14.4 Hook Configuration

The generated hooks are located in `.git/hooks/` and are non-blocking by default. To make them blocking:

**commit-msg:** Edit `.git/hooks/commit-msg` and change `exit 0` to `exit 1` where indicated.

**pre-commit:** Edit `.git/hooks/pre-commit` and uncomment the blocking exit logic.

### 14.5 ScaffoldConfig Options

```rust
pub struct ScaffoldConfig {
    // ... other fields ...

    /// Generate git hooks for traceability validation
    pub generate_git_hooks: bool,        // default: true

    /// Include commit-msg hook for AI attribution validation
    pub include_commit_msg_hook: bool,   // default: true

    /// Include pre-commit hook for trace comment validation
    pub include_pre_commit_hook: bool,   // default: false (optional)
}
```

**Note:** Git hooks are only generated if a `.git` directory exists in the project root.

---

## Appendix A: File Location Reference

| File | Purpose |
|------|---------|
| `aida-core/src/models.rs` | Core data structures |
| `aida-core/src/storage.rs` | YAML persistence |
| `aida-gui/src/app.rs` | Main GUI application |
| `aida-gui/src/main.rs` | Entry point |
| `~/.local/share/aida-gui/settings.yaml` | User settings |
| `~/.requirements.config` | Project registry |
| `requirements.yaml` | Project data (per project) |

## Appendix B: Key Line Number Reference (app.rs)

| Section | Approximate Lines |
|---------|-------------------|
| ThemeColor | 269-315 |
| CustomTheme | 319-609 |
| Theme enum | 627-787 |
| KeyBinding system | 824-1226 |
| UserSettings | 1320-1428 |
| LayoutMode | 1500-1580 |
| RequirementsApp struct | 1759-2031 |
| RequirementsApp impl | 2043+ |

---

*Last Updated: December 2025*
*AIDA Version: 0.1.0*
