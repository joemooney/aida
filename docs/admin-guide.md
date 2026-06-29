# AIDA Administrator's Guide

This guide covers administrative topics including project configuration, storage backends, database schema, migration procedures, and multi-user deployment.

**Related Documentation:**
- [User Guide](user-guide.md) - End-user documentation for using AIDA
- [Developer's Guide](DEVELOPER_GUIDE.md) - For developers maintaining and extending AIDA

---

## Table of Contents

- [Project Configuration](#project-configuration)
- [Storage Backends](#storage-backends)
- [Database Schema](#database-schema)
- [Migration Between Backends](#migration-between-backends)
- [Multi-User Control](#multi-user-control)
- [Backup and Recovery](#backup-and-recovery)
- [Performance Tuning](#performance-tuning)
- [Troubleshooting](#troubleshooting)

---

## Project Configuration

### Project Settings Overview

Project settings are stored within the requirements database file and control how the system behaves. These settings can be configured through the GUI (Settings > Admin) or by directly editing the YAML file.

### ID Configuration

The ID configuration controls how requirement identifiers (SPEC-IDs) are generated.

| Setting | Options | Description |
|---------|---------|-------------|
| **ID Format** | SingleLevel, TwoLevel | Determines ID structure |
| **Numbering Strategy** | GlobalSequential, PerPrefix, PerFeatureType | How numbers are assigned |
| **Digits** | 1-6 | Number of digits in numeric portion |

**ID Format Options:**

- **SingleLevel** (`PREFIX-NNN`): Simple format like `AUTH-001`, `FR-002`
- **TwoLevel** (`FEATURE-TYPE-NNN`): Extended format like `AUTH-FR-001`, `PAY-NFR-001`

**Numbering Strategy Options:**

- **GlobalSequential**: Single counter for all requirements (AUTH-001, FR-002, SEC-003)
- **PerPrefix**: Each prefix maintains its own counter (AUTH-001, FR-001, SEC-001)
- **PerFeatureType**: Each feature+type combination has its own counter (Two Level only)

### Focus-scope drift guard (`[focus] out_of_scope`)

When a worktree has a focus set (`aida focus <epic-or-spec>`, written to
`.aida/focus`; STORY-706) and you START work on a spec OUTSIDE that focus's
subtree, AIDA can nudge you so cross-scope mix-ups are caught at the keyboard
(STORY-717). The policy lives in `.aida/config.toml`:

```toml
[focus]
# off | warn | block  (default: warn)
out_of_scope = "warn"
```

| Value | Behavior at a work-start moment |
|-------|---------------------------------|
| `off` | No-op — the guard is silent. |
| `warn` (default) | Prints a stderr nudge that names the mismatch and suggests the fix (`<spec> is not under your current focus (<focus>). Did you mean 'aida focus <epic>' first?`), then PROCEEDS. |
| `block` | Refuses the work-start with the same message and a non-zero exit, unless `--force`. |

The guard fires at the three **work-start** moments only (never at commit time):

- `aida queue work <spec>`
- `aida agent new <vendor> --spec <spec>`
- `aida edit <spec> --status in-progress`

`--force` ALWAYS overrides regardless of the configured policy (and silences the
`warn` nudge). Membership is the focus epic itself plus its transitive
descendants — the same subtree the focus read-scope (`aida list`, etc.) uses.
A spec inside the focus subtree, or no focus set, is never guarded.

### Feature Configuration

Features organize requirements into logical groups. Each feature has:

- **Number**: Auto-assigned sequential number (e.g., "1", "2", "3")
- **Name**: Human-readable name (e.g., "Authentication", "User-Management")
- **Prefix**: Optional ID prefix for requirements in this feature

Features are stored in the `features` array in the requirements file.

### Type Definitions

Custom requirement types allow defining type-specific workflows. Each type definition includes:

```yaml
type_definitions:
  - name: "ChangeRequest"
    display_name: "Change Request"
    description: "Tracks change requests"
    prefix: "CR"
    statuses:
      - Draft
      - Submitted
      - Under Review
      - Approved
      - Rejected
      - In Progress
      - Implemented
      - Verified
      - Closed
    custom_fields:
      - name: "impact"
        display_name: "Impact"
        field_type: "Select"
        options: ["Low", "Medium", "High"]
        required: true
```

### Relationship Definitions

Custom relationship types can be defined with constraints:

```yaml
relationship_definitions:
  - name: "blocks"
    display_name: "Blocks"
    description: "This requirement blocks another"
    inverse: "blocked_by"
    cardinality: "n:n"
    source_types: []  # Empty means all types allowed
    target_types: []
    color: "#ff6b6b"
```

### Prefix Management

Control which ID prefixes are allowed in the project:

| Setting | Description |
|---------|-------------|
| **allowed_prefixes** | List of permitted prefix strings |
| **restrict_prefixes** | When true, only allowed prefixes can be used |

This enables administrators to enforce naming conventions across the team.

---

## Storage Backends

**As of EPIC-1-001 (2026-05-02), AIDA's canonical model is git-orphan-branch + SQLite cache view.** New projects (`aida init`) get this by default. Other modes are either alternatives for specific scenarios (PostgreSQL for shared server-backed deployments, sibling repo for multi-repo workspaces) or deprecated legacy paths (standalone YAML, standalone SQLite-canonical).

For the full mode comparison, when to use each, and migration guidance, see **[Storage Modes](storage-modes.md)**.

### Quick reference

| Mode | Default? | When |
|------|----------|------|
| Git worktree | ✅ default | Most projects |
| Git sibling | `--sibling` flag | Multi-repo workspaces |
| PostgreSQL | opt-in (`--features postgres`) | Teams wanting a shared server-backed projection |
| Legacy SQLite | `--centralized` (deprecated) | Backwards compat for pre-EPIC-1-001 projects |
| Legacy YAML | manually point `--file requirements.yaml` (deprecated) | Single-file scenarios; not recommended |

### Cache management (git-canonical mode)

The SQLite cache lives at `.aida/cache.db` (gitignored, auto-rebuilt). It's a derived projection — never authoritative.

```bash
aida cache status      # Compare cache HEAD vs git HEAD; show counts; report FRESH/STALE
aida cache rebuild     # Force a full rebuild from git
```

The cache rebuilds automatically on the next read whenever its recorded HEAD SHA differs from the orphan branch's actual HEAD (e.g., after `git pull` brings in remote requirement changes).

### Backend auto-detection

The CLI auto-detects mode based on what's present in the project root:

- `.aida/config.toml` with `mode = "distributed"` → git-canonical via `CachedGitBackend`
- `.aida/config.toml` not present, `requirements.db` exists → legacy SQLite (deprecated path)
- `.aida/config.toml` not present, only `requirements.yaml` → legacy YAML (deprecated path)
- `--file postgres://...` → PostgreSQL (requires `postgres` feature flag)
- `--file <path-to-dir>` → git-canonical at the explicit directory

---

## Database Schema

### YAML Schema

The YAML schema is self-describing. Key top-level fields:

| Field | Type | Description |
|-------|------|-------------|
| `name` | String | Project internal name |
| `title` | String | Display title |
| `description` | String | Project description |
| `requirements` | Array | List of requirement objects |
| `users` | Array | List of user objects |
| `features` | Array | Feature definitions |
| `id_config` | Object | ID generation configuration |
| `type_definitions` | Array | Custom type definitions |
| `relationship_definitions` | Array | Custom relationship types |
| `next_spec_number` | Integer | Next auto-increment number |
| `next_feature_number` | Integer | Next feature number |
| `prefix_counters` | Object | Per-prefix counters (when using PerPrefix numbering) |

### SQLite Schema

The SQLite database uses the following tables:

#### schema_version
Tracks database schema version for migrations.

```sql
CREATE TABLE schema_version (
    version INTEGER NOT NULL
);
```

Current version: **1**

#### requirements
Stores all requirements.

```sql
CREATE TABLE requirements (
    id TEXT PRIMARY KEY NOT NULL,          -- UUID
    spec_id TEXT,                          -- Human-readable ID (SPEC-001)
    prefix_override TEXT,                  -- Custom prefix override
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'Draft',
    priority TEXT NOT NULL DEFAULT 'Medium',
    owner TEXT NOT NULL DEFAULT '',
    feature TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,              -- ISO 8601 timestamp
    created_by TEXT,                       -- User UUID reference
    modified_at TEXT NOT NULL,             -- ISO 8601 timestamp
    req_type TEXT NOT NULL DEFAULT 'Functional',
    dependencies TEXT NOT NULL DEFAULT '[]',    -- JSON array of UUIDs
    tags TEXT NOT NULL DEFAULT '[]',            -- JSON array of strings
    relationships TEXT NOT NULL DEFAULT '[]',   -- JSON array of relationship objects
    comments TEXT NOT NULL DEFAULT '[]',        -- JSON array of comment objects
    history TEXT NOT NULL DEFAULT '[]',         -- JSON array of history entries
    archived INTEGER NOT NULL DEFAULT 0,        -- Boolean (0/1)
    custom_status TEXT,                         -- Custom status for custom types
    custom_fields TEXT NOT NULL DEFAULT '{}',   -- JSON object of custom field values
    urls TEXT NOT NULL DEFAULT '[]'             -- JSON array of URL objects
);

-- Indexes for common queries
CREATE INDEX idx_requirements_spec_id ON requirements(spec_id);
CREATE INDEX idx_requirements_feature ON requirements(feature);
CREATE INDEX idx_requirements_status ON requirements(status);
CREATE INDEX idx_requirements_archived ON requirements(archived);
```

#### users
Stores user accounts.

```sql
CREATE TABLE users (
    id TEXT PRIMARY KEY NOT NULL,          -- UUID
    spec_id TEXT,                          -- Human-readable ID ($USER-001)
    name TEXT NOT NULL,
    email TEXT NOT NULL DEFAULT '',
    handle TEXT NOT NULL,                  -- @mention handle
    created_at TEXT NOT NULL,              -- ISO 8601 timestamp
    archived INTEGER NOT NULL DEFAULT 0    -- Boolean (0/1)
);

CREATE INDEX idx_users_handle ON users(handle);
```

#### metadata
Single-row table for project configuration.

```sql
CREATE TABLE metadata (
    id INTEGER PRIMARY KEY CHECK (id = 1), -- Always 1 (single row)
    name TEXT NOT NULL DEFAULT '',
    title TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    id_config TEXT NOT NULL DEFAULT '{}',              -- JSON object
    features TEXT NOT NULL DEFAULT '[]',               -- JSON array
    next_feature_number INTEGER NOT NULL DEFAULT 1,
    next_spec_number INTEGER NOT NULL DEFAULT 1,
    prefix_counters TEXT NOT NULL DEFAULT '{}',        -- JSON object
    relationship_definitions TEXT NOT NULL DEFAULT '[]', -- JSON array
    reaction_definitions TEXT NOT NULL DEFAULT '[]',     -- JSON array
    meta_counters TEXT NOT NULL DEFAULT '{}',            -- JSON object
    type_definitions TEXT NOT NULL DEFAULT '[]',         -- JSON array
    allowed_prefixes TEXT NOT NULL DEFAULT '[]',         -- JSON array
    restrict_prefixes INTEGER NOT NULL DEFAULT 0         -- Boolean (0/1)
);
```

### Schema Versioning

The SQLite backend includes schema version tracking for future migrations:

1. On database creation, `schema_version` is set to the current version
2. On load, the version is checked
3. If older, migration scripts would be applied (future feature)

---

## Migration Between Backends

### YAML to SQLite Migration

Convert a YAML database to SQLite for better performance:

**CLI Command:**
```bash
# Future CLI support planned
# aida migrate --from requirements.yaml --to requirements.db
```

**Programmatic Migration:**
```rust
use aida_core::db::{migrate_yaml_to_sqlite};

let count = migrate_yaml_to_sqlite(
    "requirements.yaml",
    "requirements.db"
)?;
println!("Migrated {} requirements", count);
```

**Steps:**
1. Back up your YAML file
2. Run migration
3. Update your project registry to point to the new .db file
4. Verify data integrity
5. Keep YAML backup until confident

### SQLite to YAML Migration

Convert SQLite back to YAML (e.g., for version control):

**Programmatic Migration:**
```rust
use aida_core::db::{migrate_sqlite_to_yaml};

let count = migrate_sqlite_to_yaml(
    "requirements.db",
    "requirements.yaml"
)?;
println!("Exported {} requirements", count);
```

### JSON Import/Export

JSON format provides interoperability with other systems:

**Export to JSON:**
```rust
use aida_core::db::{export_to_json};
use aida_core::models::RequirementsStore;

let store = backend.load()?;
export_to_json(&store, "backup.json")?;
```

**Import from JSON:**
```rust
use aida_core::db::{import_from_json};

let store = import_from_json("backup.json")?;
backend.save(&store)?;
```

### Migration Best Practices

1. **Always backup first** - Copy original file before migration
2. **Verify counts** - Check requirement counts match after migration
3. **Spot check data** - Manually verify a few requirements
4. **Test workflows** - Ensure add/edit/delete work with new backend
5. **Update registry** - Point to new file location if changed

---

## Multi-User Control

### File-Based Locking (YAML)

The YAML backend uses file-based locking to prevent concurrent write conflicts:

- Read operations: Shared access allowed
- Write operations: Exclusive lock acquired
- Lock timeout: Configurable (default varies by operation)

**Limitations:**
- Locking only works on the same filesystem
- Network filesystems may not support locking reliably
- Consider SQLite for true multi-user scenarios

### SQLite Concurrent Access

SQLite with WAL mode provides better concurrent access:

- Multiple readers allowed simultaneously
- Single writer at a time (others queue)
- WAL mode enables concurrent reads during writes
- Automatic conflict handling

**Configuration:**
SQLite is automatically configured with:
- WAL mode enabled
- Foreign keys enforced
- Busy timeout for lock waiting

### User Management

Users are managed within the requirements database:

| Field | Description |
|-------|-------------|
| `id` | Internal UUID |
| `spec_id` | Human-readable ID ($USER-001) |
| `name` | Full name |
| `email` | Email address |
| `handle` | @mention handle |
| `archived` | Soft-delete flag |

**User-Requirement Relationships:**
- `created_by` - Who created the requirement
- `assigned_to` - Who is responsible
- `tested_by` - Who verified
- `closed_by` - Who closed/completed

### Access Control Considerations

The system does not currently implement access control. For teams requiring permissions:

1. **File system permissions** - Use OS-level file permissions
2. **Repository access** - Control via Git repository permissions
3. **Network deployment** - Consider a web-based wrapper with authentication

---

## Backup and Recovery

### YAML Backup

```bash
# Simple file copy
cp requirements.yaml requirements.yaml.backup

# Timestamped backup
cp requirements.yaml "requirements.$(date +%Y%m%d_%H%M%S).yaml"

# Git-based backup (recommended)
git add requirements.yaml
git commit -m "Backup requirements"
git push
```

### SQLite Backup

```bash
# File copy (ensure no writes in progress)
cp requirements.db requirements.db.backup

# SQLite backup command (safer)
sqlite3 requirements.db ".backup 'requirements.backup.db'"

# Export to JSON for portable backup
# Use export_to_json function
```

### Recovery Procedures

**From YAML backup:**
1. Stop any running applications
2. Copy backup file to original location
3. Restart applications

**From SQLite backup:**
1. Stop any running applications
2. Copy backup file to original location
3. Restart applications

**From JSON export:**
1. Create new database file
2. Use `import_from_json` to restore
3. Update registry if needed

### Automated Backup Script

```bash
#!/bin/bash
# backup-aida.sh

BACKUP_DIR="$HOME/aida-backups"
SOURCE="$HOME/project/requirements.yaml"
DATE=$(date +%Y%m%d_%H%M%S)

mkdir -p "$BACKUP_DIR"
cp "$SOURCE" "$BACKUP_DIR/requirements.$DATE.yaml"

# Keep last 30 backups
cd "$BACKUP_DIR"
ls -t | tail -n +31 | xargs -r rm
```

---

## Performance Tuning

### YAML Backend Optimization

- **Keep file size reasonable** - Consider SQLite for 500+ requirements
- **Use features** - Logical grouping improves perceived performance
- **Archive old requirements** - Move completed/rejected to archive

### SQLite Backend Optimization

- **Indexes are pre-configured** - No manual tuning needed
- **WAL mode** - Already enabled for best concurrent performance
- **Vacuum periodically** - Reclaim space after many deletions:
  ```bash
  sqlite3 requirements.db "VACUUM;"
  ```

### General Tips

1. **Use filters** - Reduce displayed requirements to improve GUI responsiveness
2. **Archive liberally** - Hide completed requirements from active views
3. **Split large projects** - Consider separate databases for distinct subsystems
4. **Regular cleanup** - Delete orphaned relationships and unused features

---

## Troubleshooting

### Common Issues

#### "Database is locked"
- **Cause:** Another process has exclusive access
- **Solution:** Close other applications using the file, or wait and retry

#### "Failed to parse YAML"
- **Cause:** Malformed YAML syntax
- **Solution:** Use a YAML validator, restore from backup

#### "Schema version mismatch"
- **Cause:** Database created with newer software version
- **Solution:** Update software or migrate database

#### "Corrupt SQLite database"
- **Cause:** Incomplete write, hardware failure
- **Solution:** Restore from backup, run `sqlite3 db.db "PRAGMA integrity_check;"`

### Diagnostic Commands

```bash
# Check YAML syntax
python3 -c "import yaml; yaml.safe_load(open('requirements.yaml'))"

# Check SQLite integrity
sqlite3 requirements.db "PRAGMA integrity_check;"

# View SQLite schema
sqlite3 requirements.db ".schema"

# Count requirements in SQLite
sqlite3 requirements.db "SELECT COUNT(*) FROM requirements;"

# View metadata in SQLite
sqlite3 requirements.db "SELECT * FROM metadata;"
```

### Recovery from Corruption

**YAML corruption:**
1. Restore from Git history: `git checkout HEAD~1 -- requirements.yaml`
2. Or restore from backup file
3. Use JSON export if available

**SQLite corruption:**
1. Try: `sqlite3 corrupt.db ".recover" | sqlite3 recovered.db`
2. Or restore from backup
3. Use JSON export if available

### Getting Help

- Check logs for error messages
- Verify file permissions
- Test with a fresh database file
- Report issues at the project repository

---

## Appendix: Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `AIDA_DB_NAME` | Default project name from registry | (none) |
| `AIDA_FEATURE` | Default feature for new requirements | "Uncategorized" |
| `AIDA_REGISTRY_PATH` | Custom registry file location | `~/.aida.config` |

## Appendix: Registry File Format

The multi-project registry (`~/.aida.config`) uses YAML format:

```yaml
default: "my-project"
projects:
  - name: "my-project"
    path: "/home/user/projects/myproject/requirements.yaml"
    description: "Main project requirements"
  - name: "backend"
    path: "/home/user/projects/backend/requirements.db"
    description: "Backend service requirements"
```

---

*Last updated: December 2025 | AIDA v0.1.0*
