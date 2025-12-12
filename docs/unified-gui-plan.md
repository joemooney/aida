# Unified GUI Plan: Native + WASM from Single Codebase

## Overview

Refactor `aida-gui` to compile for both native desktop and WebAssembly (browser) targets, replacing the separate `aida-web` crate with a unified codebase.

## Current State

| Aspect | aida-gui (Native) | aida-web (WASM) |
|--------|-------------------|-----------------|
| Lines of code | ~29,000 | ~1,000 |
| Views | 10 (List, Detail, Add, Edit, OrgChart, KanBan, etc.) | 4 (List, Detail, Create, Edit) |
| Storage | Local file OR remote server | Remote only |
| Settings | File-based (~/.config/aida/) | None |
| Features | Full (themes, keybindings, AI, etc.) | Minimal MVP |

## Architecture Goals

1. **Single codebase** - One `aida-gui` crate that compiles to both targets
2. **Platform abstraction** - Trait-based services for platform-specific features
3. **Modular structure** - Break up monolithic app.rs into focused modules
4. **Feature parity** - Web version gets same UI as native (where feasible)

## Platform Abstraction Layer

Create `aida-gui/src/platform/mod.rs`:

```rust
/// Platform-specific services abstraction
pub trait PlatformServices: Send + Sync {
    // File Operations
    fn pick_file(&self, title: &str, filters: &[(&str, &[&str])]) -> Option<PathBuf>;
    fn pick_folder(&self, title: &str) -> Option<PathBuf>;
    fn save_file(&self, title: &str, default_name: &str, data: &[u8]) -> Result<(), String>;

    // Clipboard
    fn copy_to_clipboard(&self, text: &str) -> Result<(), String>;
    fn get_clipboard(&self) -> Option<String>;

    // Settings Storage
    fn load_settings(&self, key: &str) -> Option<String>;
    fn save_settings(&self, key: &str, value: &str) -> Result<(), String>;
    fn get_config_dir(&self) -> Option<PathBuf>;

    // External Actions
    fn open_url(&self, url: &str) -> Result<(), String>;
    fn open_file_external(&self, path: &Path) -> Result<(), String>;

    // System Info
    fn get_hostname(&self) -> String;
}
```

### Native Implementation (`platform/native.rs`)

```rust
#[cfg(not(target_arch = "wasm32"))]
pub struct NativePlatform;

impl PlatformServices for NativePlatform {
    fn pick_file(&self, title: &str, filters: &[(&str, &[&str])]) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title(title);
        for (name, exts) in filters {
            dialog = dialog.add_filter(name, exts);
        }
        dialog.pick_file()
    }

    fn copy_to_clipboard(&self, text: &str) -> Result<(), String> {
        arboard::Clipboard::new()
            .and_then(|mut cb| cb.set_text(text))
            .map_err(|e| e.to_string())
    }

    fn get_config_dir(&self) -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("aida"))
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        open::that(url).map_err(|e| e.to_string())
    }
    // ... etc
}
```

### Web Implementation (`platform/web.rs`)

```rust
#[cfg(target_arch = "wasm32")]
pub struct WebPlatform;

impl PlatformServices for WebPlatform {
    fn pick_file(&self, _title: &str, _filters: &[(&str, &[&str])]) -> Option<PathBuf> {
        // Trigger file input element via JavaScript
        // Returns None - file content comes via callback
        None
    }

    fn copy_to_clipboard(&self, text: &str) -> Result<(), String> {
        use wasm_bindgen_futures::spawn_local;
        // Use navigator.clipboard.writeText()
        let text = text.to_string();
        spawn_local(async move {
            let window = web_sys::window().unwrap();
            let clipboard = window.navigator().clipboard();
            let _ = clipboard.write_text(&text);
        });
        Ok(())
    }

    fn load_settings(&self, key: &str) -> Option<String> {
        let window = web_sys::window()?;
        let storage = window.local_storage().ok()??;
        storage.get_item(key).ok()?
    }

    fn save_settings(&self, key: &str, value: &str) -> Result<(), String> {
        let window = web_sys::window().ok_or("No window")?;
        let storage = window.local_storage()
            .map_err(|_| "No localStorage")?
            .ok_or("No localStorage")?;
        storage.set_item(key, value).map_err(|_| "Failed to save")
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        let window = web_sys::window().ok_or("No window")?;
        window.open_with_url(url).map_err(|_| "Failed to open")?;
        Ok(())
    }

    fn get_config_dir(&self) -> Option<PathBuf> {
        None // Not applicable for web
    }
    // ... etc
}
```

## Modularization Plan

Break `app.rs` (29K lines) into focused modules:

```
aida-gui/src/
├── main.rs              # Entry point (conditional for native/wasm)
├── lib.rs               # Library exports
├── app.rs               # Core RequirementsApp struct (reduced)
├── platform/
│   ├── mod.rs           # PlatformServices trait
│   ├── native.rs        # Native implementation
│   └── web.rs           # WASM implementation
├── views/
│   ├── mod.rs
│   ├── list.rs          # List view
│   ├── detail.rs        # Detail view with tabs
│   ├── edit.rs          # Add/Edit forms
│   ├── kanban.rs        # Kanban board
│   ├── timeline.rs      # Timeline view
│   ├── settings.rs      # Settings panels
│   └── ...
├── components/
│   ├── mod.rs
│   ├── requirement_card.rs
│   ├── status_badge.rs
│   ├── priority_badge.rs
│   ├── comment_tree.rs
│   ├── relationship_editor.rs
│   ├── tag_picker.rs
│   └── ...
├── state/
│   ├── mod.rs
│   ├── filters.rs       # Filter state & logic
│   ├── selection.rs     # Selection management
│   ├── forms.rs         # Form state
│   └── notifications.rs # Toast/notification state
├── services/
│   ├── mod.rs
│   ├── search.rs        # Search & filtering logic
│   ├── ai.rs            # AI evaluation integration
│   ├── keybindings.rs   # Keyboard handling
│   └── themes.rs        # Theme management
└── storage/
    ├── mod.rs           # StorageBackend abstraction
    ├── local.rs         # Local file storage
    └── remote.rs        # gRPC remote storage
```

## Cargo.toml Changes

```toml
[package]
name = "aida-gui"
version.workspace = true

[lib]
crate-type = ["cdylib", "rlib"]

[features]
default = ["native"]
native = ["rfd", "arboard", "dirs", "open", "hostname"]
web = ["wasm-bindgen", "wasm-bindgen-futures", "web-sys", "js-sys", "console_log", "console_error_panic_hook"]
remote = ["tonic", "tokio"]

[dependencies]
# Core (both targets)
eframe = { version = "0.29", default-features = false, features = ["default_fonts", "glow"] }
egui = "0.29"
aida-core = { path = "../aida-core" }
serde = { version = "1.0", features = ["derive"] }
serde_yaml = "0.9"

# Native-only
rfd = { version = "0.15", optional = true }
arboard = { version = "3.4", optional = true }
dirs = { version = "5.0", optional = true }
open = { version = "5.3", optional = true }
hostname = { version = "0.4", optional = true }

# Web-only
wasm-bindgen = { version = "0.2", optional = true }
wasm-bindgen-futures = { version = "0.4", optional = true }
web-sys = { version = "0.3", optional = true, features = [...] }
js-sys = { version = "0.3", optional = true }
console_log = { version = "1", optional = true }
console_error_panic_hook = { version = "0.1", optional = true }

# Remote (both, but optional)
tonic = { version = "0.12", optional = true, default-features = false, features = ["prost", "codegen"] }
tonic-web-wasm-client = { version = "0.6", optional = true }
```

## Implementation Phases

### Phase 1: Platform Abstraction (Foundation)
1. Create `platform/` module with trait and implementations
2. Replace direct `rfd`, `arboard`, `dirs`, `open` calls with platform trait
3. Add conditional compilation gates
4. Verify native still works

### Phase 2: Entry Point Unification
1. Create `lib.rs` exporting `RequirementsApp`
2. Native `main.rs` with native entry point
3. WASM entry via `#[wasm_bindgen(start)]` in lib.rs
4. Build with `trunk` for web, `cargo build` for native

### Phase 3: Modularize Views
1. Extract each view to separate module
2. Pass app state via references, not monolithic struct
3. Define shared component traits
4. Reduce app.rs to orchestration only

### Phase 4: Shared Components
1. Extract reusable widgets (badges, cards, pickers)
2. Standardize styling across views
3. Create component library usable by both targets

### Phase 5: Settings & Storage
1. Abstract settings storage (file vs localStorage)
2. Implement web localStorage adapter
3. Add IndexedDB for offline data caching (optional)

### Phase 6: Feature Parity
1. Port remaining views to web (KanBan, Timeline, etc.)
2. Implement web-compatible keyboard shortcuts
3. Add web-specific features (PWA, notifications)

### Phase 7: Deprecate aida-web
1. Remove `aida-web` crate from workspace
2. Update documentation
3. Update Makefile targets

## Build Commands

```bash
# Native build
cargo build -p aida-gui --features native

# Web build
trunk build --release

# Web dev server
trunk serve --port 8088

# Both features (for IDE support)
cargo check -p aida-gui --features "native,web"
```

## Migration Strategy

- **Incremental** - Each phase produces working code
- **Feature flags** - Use `#[cfg(feature = "native")]` and `#[cfg(target_arch = "wasm32")]`
- **No breaking changes** - Native GUI continues to work throughout
- **Testing** - Verify both targets after each phase

## Estimated Effort

| Phase | Effort | Risk |
|-------|--------|------|
| 1. Platform Abstraction | Medium | Low |
| 2. Entry Point | Small | Low |
| 3. Modularize Views | Large | Medium |
| 4. Shared Components | Medium | Low |
| 5. Settings/Storage | Medium | Low |
| 6. Feature Parity | Large | Medium |
| 7. Deprecate aida-web | Small | Low |

Total: Significant refactoring effort, but each phase delivers value.

## Success Criteria

1. Single `cargo build` produces native binary
2. Single `trunk build` produces WASM bundle
3. Both targets share >90% of UI code
4. Web client has feature parity with native (minus file system features)
5. No regression in native GUI functionality
