# Unified Storage Architecture for AIDA GUI

## Overview

This document describes the architecture for unifying local and remote storage access in aida-gui, enabling a single codebase to compile for both native desktop and WebAssembly (browser) targets.

## Problem Statement

The current aida-gui uses `aida_core::Storage` directly, which requires:
- File system access (`std::fs`)
- File locking (`fs2`)
- SQLite database (`rusqlite`)

These dependencies are not available in WASM. Additionally, the code has extensive conditional compilation scattered throughout.

## Solution: Unified gRPC-Based Storage

**Key Insight**: The storage backend (SQLite, YAML files) should ALWAYS be accessed via gRPC, even for "local" storage on desktop.

### Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                         AIDA GUI                                 │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │                   RequirementsApp                        │    │
│  │                                                          │    │
│  │  Uses: StorageClient (trait)                            │    │
│  └─────────────────────────────────────────────────────────┘    │
│                              │                                   │
│                              ▼                                   │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │              StorageClient (gRPC Client)                 │    │
│  │  - load() -> RequirementsStore                          │    │
│  │  - save(&store) -> Result<()>                           │    │
│  │  - create_requirement(&req) -> Requirement              │    │
│  │  - update_requirement(&req) -> Requirement              │    │
│  │  - delete_requirement(id) -> Result<()>                 │    │
│  │  - add_comment(...) -> Comment                          │    │
│  │  - get_server_status() -> ServerStatus                  │    │
│  └─────────────────────────────────────────────────────────┘    │
└────────────────────────────────│────────────────────────────────┘
                                 │
                          gRPC / gRPC-Web
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────┐
│                      AIDA Server                                 │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │              RequirementsService (gRPC)                  │    │
│  └─────────────────────────────────────────────────────────┘    │
│                              │                                   │
│                              ▼                                   │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │                  Storage Backend                         │    │
│  │  - SQLite Database                                       │    │
│  │  - YAML Files                                            │    │
│  │  - File Locking                                          │    │
│  │  - Attachments                                           │    │
│  └─────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

### Desktop vs Web Deployment

**Native Desktop:**
```
┌──────────────────────────────────────────────────────────────┐
│                     Desktop Application                       │
│  ┌────────────────┐        ┌─────────────────────────────┐   │
│  │   GUI Thread   │        │    Embedded Server Thread    │   │
│  │                │ gRPC   │                              │   │
│  │  StorageClient │◄──────►│  AIDA Server (localhost)    │   │
│  │                │        │                              │   │
│  └────────────────┘        │  ┌─────────────────────┐    │   │
│                            │  │   Storage Backend    │    │   │
│                            │  │   (SQLite/YAML)      │    │   │
│                            │  └─────────────────────┘    │   │
│                            └─────────────────────────────┘   │
└──────────────────────────────────────────────────────────────┘
```

**Web Browser:**
```
┌──────────────────────────────────┐    ┌──────────────────────────┐
│          Web Browser              │    │     Remote Server        │
│  ┌────────────────────────────┐  │    │  ┌────────────────────┐  │
│  │        WASM GUI             │  │    │  │   AIDA Server      │  │
│  │                             │  │    │  │                    │  │
│  │  StorageClient (gRPC-Web)  ─┼──┼────┼──│► RequirementsService│  │
│  │                             │  │    │  │                    │  │
│  └────────────────────────────┘  │    │  │  Storage Backend   │  │
└──────────────────────────────────┘    │  └────────────────────┘  │
                                        └──────────────────────────┘
```

## Implementation Plan

### Phase 1: StorageClient Trait

Create `aida-gui/src/storage/mod.rs`:

```rust
/// Unified storage client trait for both local and remote backends
pub trait StorageClient: Send + Sync {
    /// Load the entire requirements store
    fn load(&self) -> Result<RequirementsStore>;

    /// Save the entire requirements store
    fn save(&self, store: &RequirementsStore) -> Result<()>;

    /// Get the display path/address for this storage
    fn display_path(&self) -> String;

    /// Check if this is a remote connection
    fn is_remote(&self) -> bool;

    /// Create a new requirement
    fn create_requirement(&self, req: &Requirement) -> Result<Requirement>;

    /// Update an existing requirement
    fn update_requirement(&self, req: &Requirement) -> Result<Requirement>;

    /// Delete a requirement by ID
    fn delete_requirement(&self, id: &str) -> Result<()>;

    /// Add a comment to a requirement
    fn add_comment(&self, req_id: &str, content: &str, author: &str, parent_id: Option<&str>) -> Result<Comment>;

    /// Add a relationship between requirements
    fn add_relationship(&self, source_id: &str, target_id: &str, rel_type: &RelationshipType, created_by: &str) -> Result<()>;

    /// Get server status
    fn get_server_status(&self) -> Result<ServerStatus>;
}
```

### Phase 2: gRPC Client Implementation

The existing `remote.rs` already has most of this - refactor it to implement the trait.

### Phase 3: Embedded Server for Desktop

For native desktop builds:
1. Start embedded AIDA server in a background thread
2. Server listens on a random available port (localhost only)
3. GUI connects via gRPC to localhost:port

```rust
#[cfg(feature = "native")]
pub fn start_embedded_server(db_path: PathBuf) -> Result<(JoinHandle<()>, u16)> {
    // Find available port
    let port = find_available_port()?;

    // Spawn server thread
    let handle = std::thread::spawn(move || {
        // Start tokio runtime in this thread
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            aida_server::run_server(port, db_path).await
        });
    });

    Ok((handle, port))
}
```

### Phase 4: Refactor app.rs

Replace:
- `self.storage: Storage` → `self.storage: Box<dyn StorageClient>`
- Remove all direct file system calls
- Remove Storage-specific types from imports
- Use trait methods instead

### Benefits

1. **Single Codebase**: No conditional compilation in business logic
2. **Clean Architecture**: GUI is pure presentation layer
3. **Testability**: Can mock StorageClient for testing
4. **Flexibility**: Easy to add new storage backends
5. **Consistency**: Same behavior across platforms

### Migration Path

1. Create StorageClient trait
2. Implement for remote gRPC client
3. Create embedded server wrapper for native
4. Gradually migrate app.rs to use trait
5. Remove direct Storage usage
6. Test both native and web builds

### Not in Scope (Future Work)

- Offline support for web (would need IndexedDB caching)
- Conflict resolution UI (currently native-only)
- Session/lock management via gRPC (needs proto extension)
- Attachment upload/download via gRPC (needs proto extension)
