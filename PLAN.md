# REQ-0231: Concurrent Edit Protection Implementation Plan

## Problem Analysis

Currently, when the GUI and CLI/server access the same `requirements.yaml` file:
1. Both load the entire file into memory
2. Each makes changes independently
3. Whichever saves last overwrites all changes from the other

While there IS file locking (`fs2::FileExt`) during load/save operations, the locks are short-lived. The real issue is that both processes hold stale copies in memory and don't detect external changes before overwriting.

## Proposed Solution: SQLite as Primary Storage

Switch to SQLite as the primary storage backend with per-record versioning. YAML becomes an export/import format only.

### Why SQLite?

1. **Built-in concurrency**: WAL mode allows concurrent readers with one writer
2. **Per-record locking**: Only lock what you're modifying, not the whole database
3. **Optimistic locking**: Easy with a `version` column - increment on each update
4. **ACID transactions**: Guaranteed consistency
5. **Already implemented**: `SqliteBackend` exists but isn't the default

### Key Changes

#### 1. Add Version Column to SQLite Schema (schema_v2.sql)
```sql
-- Add version column for optimistic locking
ALTER TABLE requirements ADD COLUMN version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE users ADD COLUMN version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE metadata ADD COLUMN store_version INTEGER NOT NULL DEFAULT 1;
```

#### 2. Implement Optimistic Locking in SqliteBackend

When updating a requirement:
```rust
fn update_requirement(&self, req: &Requirement, expected_version: i64) -> Result<(), StorageError> {
    let rows = conn.execute(
        "UPDATE requirements SET ..., version = version + 1
         WHERE id = ? AND version = ?",
        params![req.id.to_string(), expected_version]
    )?;

    if rows == 0 {
        return Err(StorageError::VersionConflict {
            id: req.id,
            expected_version,
            current_version: self.get_current_version(req.id)?
        });
    }
    Ok(())
}
```

#### 3. Migrate GUI to Use SQLite Backend

Update `aida-gui/src/app.rs`:
- Change default backend from YAML to SQLite (`.aida.db`)
- Store version with each loaded requirement
- On save, check version and handle conflicts
- Provide "Export to YAML" option in File menu

#### 4. Update CLI to Default to SQLite

Update `aida-cli/src/main.rs`:
- Default to `.aida.db` if no path specified
- Support `--format yaml|sqlite` flag for explicit choice
- Auto-migrate YAML to SQLite on first use (with backup)

#### 5. Add File Change Detection for GUI

Even with SQLite, the GUI should detect if another process modified the database:
- Poll `store_version` in metadata table periodically
- Show notification when external changes detected
- Offer to reload (merge or replace)

## Implementation Steps

### Phase 1: Schema and Backend Updates
1. Create schema_v2.sql with version columns
2. Add migration logic in SqliteBackend::init_schema()
3. Add `version` field to Requirement and User models
4. Implement versioned update/save in SqliteBackend

### Phase 2: Conflict Detection
1. Add `VersionConflict` variant to StorageError
2. Implement `update_with_version()` method
3. Add `get_current_version()` helper
4. Create conflict resolution UI components

### Phase 3: GUI Migration
1. Change default storage to SQLite
2. Add "Export to YAML" menu option
3. Add "Import from YAML" menu option
4. Add periodic version check (every 2 seconds)
5. Show conflict dialog when version mismatch detected

### Phase 4: CLI Migration
1. Update default file detection
2. Add migration command: `aida migrate --from yaml --to sqlite`
3. Auto-backup YAML before migration

### Phase 5: Testing
1. Unit tests for optimistic locking
2. Integration tests for concurrent updates
3. Manual testing with GUI + CLI simultaneously

## Migration Strategy

For existing users:
1. First run with SQLite: detect existing `requirements.yaml`
2. Prompt: "Migrate to SQLite for better concurrency? (YAML will be backed up)"
3. If yes: copy to `requirements.yaml.bak`, migrate to `requirements.db`
4. If no: continue using YAML (with current limitations)

## Files to Modify

- `aida-core/src/db/schema.sql` → Add version columns
- `aida-core/src/db/sqlite_backend.rs` → Implement versioned updates
- `aida-core/src/db/traits.rs` → Add version-aware methods
- `aida-core/src/models.rs` → Add version field to Requirement/User
- `aida-core/src/storage.rs` → Add VersionConflict error type
- `aida-gui/src/app.rs` → Switch to SQLite, add polling
- `aida-cli/src/main.rs` → Switch to SQLite default
- New: `aida-core/src/db/migration_v2.rs` → YAML→SQLite migration

## Rollback Plan

If issues arise:
1. SQLite file can always be exported to YAML
2. YAML backend remains fully functional
3. User can switch back with `--format yaml`
