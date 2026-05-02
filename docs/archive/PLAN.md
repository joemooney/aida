# Drag-and-Drop Attachments Feature Plan

## Overview

Add the ability to attach files to requirements via drag-and-drop in the GUI. Files are stored in an `attachments/<spec_id>/` directory structure.

## Requirements

1. Drag a file over the Detail View panel → highlight panel with visual feedback
2. Drop file → copy to `attachments/<spec_id>/` folder
3. Store attachment metadata in the requirement
4. Display attachments in a new "Attachments" tab
5. Optionally git add/commit the attachment if in a git repo

## Implementation Plan

### Phase 1: Data Model Updates (aida-core/src/models.rs)

1. Add `Attachment` struct (similar to existing `UrlLink`):
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Attachment {
    pub id: Uuid,
    pub filename: String,           // Original filename
    pub stored_path: String,        // Relative path: attachments/<spec_id>/<filename>
    pub mime_type: Option<String>,  // Optional MIME type
    pub size_bytes: u64,            // File size
    pub added_at: DateTime<Utc>,
    pub added_by: Option<String>,   // User handle
    pub description: Option<String>,
}
```

2. Add `attachments: Vec<Attachment>` field to `Requirement` struct

3. Add `Attachment::new()` constructor

### Phase 2: Storage Updates (aida-core/src/storage.rs)

1. Add helper method `get_attachments_dir(&self, spec_id: &str) -> PathBuf`
   - Returns `<db_parent>/attachments/<spec_id>/`
   - Creates directory if it doesn't exist

2. Add `add_attachment(&mut self, spec_id: &str, source_path: &Path) -> Result<Attachment>`
   - Creates attachments directory
   - Copies file to destination
   - Returns Attachment metadata

3. Add `remove_attachment(&mut self, spec_id: &str, attachment_id: &Uuid) -> Result<()>`
   - Removes file from disk
   - Cleans up empty directories

### Phase 3: SQLite Backend Updates (aida-core/src/db/sqlite_backend.rs)

- The `attachments` field will be stored as JSON (like comments, history)
- No schema changes needed - just ensure serialization handles the new field

### Phase 4: GUI - Drag-and-Drop Detection (aida-desktop/src/app.rs)

1. Add `DetailTab::Attachments` variant to the enum

2. Add state tracking for drag-hover:
```rust
detail_panel_drag_hover: bool,
```

3. In `draw_detail_view()`:
   - Check `ctx.input(|i| !i.raw.hovered_files.is_empty())` for hover state
   - Check `ctx.input(|i| i.raw.dropped_files.clone())` for drops
   - When hovering over detail panel, set `detail_panel_drag_hover = true`
   - Draw highlight overlay when `detail_panel_drag_hover` is true

4. Handle file drop:
   - Get `DroppedFile` from `dropped_files`
   - Get file path or bytes
   - Call storage method to save attachment
   - Add attachment to requirement
   - Save requirement
   - Show success notification

### Phase 5: GUI - Attachments Tab (aida-desktop/src/app.rs)

1. Add "Attachments" tab to `show_detail_tab_menu_popup()`

2. Implement `draw_attachments_tab()`:
   - List attachments with filename, size, date
   - "Open" button to open in system default app
   - "Remove" button with confirmation
   - Drag-drop zone indicator when empty

3. Add hotkey 'A' (with Shift) or use 't a' to switch to Attachments tab

### Phase 6: Git Integration (Optional)

1. Check if db directory is a git repo: `git rev-parse --git-dir`

2. After adding attachment:
   - `git add attachments/<spec_id>/<filename>`
   - `git commit -m "Attach <filename> to <spec_id>"`

3. Make git integration configurable (setting in GUI preferences)

## Files to Modify

1. `aida-core/src/models.rs`
   - Add `Attachment` struct
   - Add `attachments` field to `Requirement`

2. `aida-core/src/storage.rs`
   - Add `get_attachments_dir()`
   - Add `add_attachment()`
   - Add `remove_attachment()`

3. `aida-desktop/src/app.rs`
   - Add `DetailTab::Attachments`
   - Add drag-hover state
   - Implement drag-drop detection in `draw_detail_view()`
   - Implement `draw_attachments_tab()`
   - Add tab menu entry

## Visual Design

### Drag Hover State
- Semi-transparent blue overlay on detail panel
- Text: "📎 Drop file to attach"
- Border highlight

### Attachments Tab
```
┌─────────────────────────────────────────┐
│ 📎 Attachments (3)                      │
├─────────────────────────────────────────┤
│ ├── spec_document.pdf    1.2 MB  [Open] │
│ │   Added 2024-01-15 by @joe            │
│ ├── screenshot.png       245 KB  [Open] │
│ │   Added 2024-01-16 by @joe            │
│ └── notes.txt            12 KB   [Open] │
│     Added 2024-01-17 by @joe            │
├─────────────────────────────────────────┤
│ Drag files here or click [Add File]     │
└─────────────────────────────────────────┘
```

## Implementation Order

1. Phase 1: `Attachment` struct in models.rs
2. Phase 2: Storage methods
3. Phase 3: SQLite serialization verification
4. Phase 4: Drag-drop detection and visual feedback
5. Phase 5: Attachments tab UI
6. Phase 6: Git integration (optional)

## Testing

1. Unit test: `Attachment` serialization/deserialization
2. Integration test: Add/remove attachment via storage
3. Manual test: Drag file onto detail view in GUI
4. Manual test: Open attachment from tab
5. Manual test: Git commit (if enabled)
