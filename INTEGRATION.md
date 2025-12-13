# Integration: requirements-manager + ai-provenance

## Overview

This document describes the integration between **requirements-manager** (Rust-based requirements database) and **ai-provenance** (Python-based AI code tracking system).

## Architecture

```
┌─────────────────────────┐
│  requirements-manager   │  Source of Truth
│  (Rust CLI)             │  - Requirements CRUD
│                         │  - Multi-project registry
│  requirements.yaml      │  - Feature organization
└───────────┬─────────────┘
            │
            │ Export / Bridge
            ▼
┌─────────────────────────┐
│  Integration Layer      │
│  - Format converter     │
│  - ID mapping           │
│  - Sync utilities       │
└───────────┬─────────────┘
            │
            │ Read / Query
            ▼
┌─────────────────────────┐
│  ai-provenance          │  Traceability Layer
│  (Python CLI)           │  - Git integration
│                         │  - Commit metadata
│  Reads requirements     │  - Trace matrix
│  Links commits/files    │  - AI metrics
└─────────────────────────┘
```

## Data Model Mapping

### Requirement Fields

| requirements-manager (Rust) | ai-provenance (Python) | Mapping Strategy |
|----------------------------|------------------------|------------------|
| `id: Uuid` | `id: str` (SPEC-XXX) | Generate SPEC-{N} from UUID hash or feature number |
| `title: String` | `title: str` | Direct copy |
| `description: String` | `description: str` | Direct copy |
| `status: RequirementStatus` | `status: RequirementStatus` | Map enums (Draft→planned, Completed→implemented) |
| `priority: RequirementPriority` | `priority: RequirementPriority` | Map enums (High→high, etc.) |
| `req_type: RequirementType` | `type: RequirementType` | Map enums (Functional→feature, etc.) |
| `feature: String` | `tags: List[str]` | Add feature as tag |
| `owner: String` | `assigned_to: str` | Direct copy |
| `tags: Vec<String>` | `tags: List[str]` | Merge with feature tag |
| `dependencies: Vec<Uuid>` | `related: List[str]` | Convert UUIDs to SPEC-IDs |
| `created_at: DateTime` | `created_at: datetime` | Direct copy |
| `modified_at: DateTime` | `updated_at: datetime` | Direct copy |

### Status Mapping

```
requirements-manager → ai-provenance
─────────────────────────────────────
Draft          → planned
Approved       → planned
InProgress     → in-progress
Completed      → implemented
Rejected       → deprecated
```

### Priority Mapping

```
requirements-manager → ai-provenance
─────────────────────────────────────
High           → high
Medium         → medium
Low            → low
```

### Type Mapping

```
requirements-manager → ai-provenance
─────────────────────────────────────
Functional     → feature
NonFunctional  → enhancement
System         → feature
User           → feature
```

## Integration Workflows

### Workflow 1: Create Requirement → Track in Git

```bash
# 1. Create requirement in requirements-manager
cd /home/joe/ai/ai-provenance
requirements-manager -p ai-provenance add \
  --title "JWT Authentication" \
  --description "Implement JWT-based auth" \
  --status Draft \
  --priority High \
  --feature Authentication

# Output: Created requirement f7d250bf-5b3e-4ec3-8bd5-2bee2c4b7bb9

# 2. Export to ai-provenance format
requirements-manager -p ai-provenance export --format ai-prov

# 3. Work on code (AI-assisted)
# ... make changes ...

# 4. Commit with traceability
ai-prov commit -m "feat(auth): add JWT token generation" \
  --tool claude \
  --conf high \
  --trace SPEC-089

# 5. ai-provenance automatically links commit to requirement
# The trace-matrix will show:
# SPEC-089 → commits: [abc123] → files: [src/auth.py] → tests: [TC-210]
```

### Workflow 2: Query Traceability

```bash
# List requirements from requirements-manager
requirements-manager -p ai-provenance list --feature Authentication

# Generate traceability matrix (reads requirements.yaml)
ai-prov trace-matrix --format md

# Output:
# | Req ID   | Title               | Files        | Commits | Tests  | AI %  |
# |----------|---------------------|--------------|---------|--------|-------|
# | SPEC-089 | JWT Authentication  | src/auth.py  | 3       | TC-210 | 85%   |
```

### Workflow 3: Update Requirement Status

```bash
# Code is implemented and committed with ai-prov
ai-prov commit -m "[AI:claude] feat: complete auth" --trace SPEC-089

# Update requirement status
requirements-manager edit <uuid> --status Completed

# Validate all requirements are tested
ai-prov validate --require-tests
```

## Implementation Plan

### Phase 1: Export Functionality (requirements-manager)

Add export command to requirements-manager:

```rust
// In cli.rs
#[derive(Subcommand)]
enum Command {
    // ... existing commands
    Export {
        /// Output format (json, ai-prov)
        #[arg(long, default_value = "json")]
        format: String,

        /// Output file
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}
```

Export formats:
- **json**: Standard JSON export
- **ai-prov**: Export to ai-provenance's `.ai-prov/requirements/` format with SPEC-ID mapping

### Phase 2: ID Mapping System

Create bidirectional mapping between UUID and SPEC-ID:

```yaml
# .requirements-mapping.yaml (in project root)
mappings:
  f7d250bf-5b3e-4ec3-8bd5-2bee2c4b7bb9: SPEC-001
  013cc55c-3123-42b2-81df-d930cf9eb288: SPEC-002
next_spec_number: 3
```

Algorithm:
- First export: Generate SPEC-{N} sequentially for each requirement
- Subsequent exports: Use existing mapping
- Store mapping in project root or registry

### Phase 3: ai-provenance Adapter

Create adapter in ai-provenance to read from `requirements.yaml`:

```python
# src/ai_provenance/requirements/adapters.py

from pathlib import Path
import yaml
from typing import List, Dict
from ai_provenance.requirements.models import Requirement

class RequirementsManagerAdapter:
    """Adapter to read from requirements-manager YAML files."""

    def __init__(self, yaml_path: str = "requirements.yaml"):
        self.yaml_path = Path(yaml_path)
        self.mapping_path = Path(".requirements-mapping.yaml")

    def load_requirements(self) -> List[Requirement]:
        """Load requirements from requirements.yaml"""
        # Parse YAML
        # Load mapping
        # Convert to ai-provenance Requirement objects
        pass

    def get_spec_id(self, uuid: str) -> str:
        """Convert UUID to SPEC-ID"""
        pass

    def get_uuid(self, spec_id: str) -> str:
        """Convert SPEC-ID to UUID"""
        pass
```

### Phase 4: CLI Integration

Add configuration to ai-provenance:

```yaml
# .ai-prov/config.yaml
requirements:
  source: requirements-manager  # or 'native'
  path: requirements.yaml
  mapping: .requirements-mapping.yaml
```

Update ai-provenance commands:
- `ai-prov trace-matrix` → reads from requirements.yaml via adapter
- `ai-prov requirement list` → delegates to `requirements-manager list`
- `ai-prov requirement show SPEC-001` → looks up UUID, delegates to `requirements-manager show <uuid>`

### Phase 5: Bidirectional Sync (Future)

Add webhook or file-watch to sync changes:
- When `requirements.yaml` changes → regenerate `.ai-prov/requirements/`
- When commit adds new `Trace:` tag → link to requirement in requirements-manager

## File Structure

```
project-root/
├── requirements.yaml              # requirements-manager data
├── .requirements-mapping.yaml     # UUID ↔ SPEC-ID mapping
├── .ai-prov/
│   ├── config.yaml               # ai-provenance config (points to requirements.yaml)
│   ├── requirements/             # Auto-generated from requirements.yaml (cache)
│   │   ├── SPEC-001.json
│   │   └── SPEC-002.json
│   ├── tests/
│   │   └── TC-001.json
│   └── traces/
│       └── requirement_SPEC-001_file_src_auth.py.json
└── .git/
    └── refs/notes/ai-provenance  # Git notes with commit metadata
```

## Environment Variables

Shared environment variables:

- `REQ_DB_NAME`: Project name in requirements-manager registry
- `REQ_REGISTRY_PATH`: Path to requirements registry (default: `~/.requirements.config`)
- `AI_PROV_REQ_SOURCE`: Requirements source (`requirements-manager` or `native`)

## CLI Examples

### Example 1: Create and Track Requirement

```bash
# Create requirement
requirements-manager add -i
# → Title: Implement OAuth 2.0
# → Description: Add OAuth 2.0 authentication
# → Feature: Authentication
# → Created: SPEC-003 (uuid: abc-123-def)

# Work on it with AI
ai-prov stamp src/oauth.py --tool claude --conf high --trace SPEC-003

# Commit
ai-prov commit -m "feat(oauth): add OAuth flow" --trace SPEC-003 --test TC-301

# Update status
requirements-manager edit abc-123-def --status Completed

# Generate report
ai-prov trace-matrix
```

### Example 2: Multi-Project Setup

```bash
# Register both projects
requirements-manager db register \
  --name ai-provenance \
  --path /home/joe/ai/ai-provenance/requirements.yaml \
  --description "AI provenance tracking tool"

requirements-manager db register \
  --name requirements-manager \
  --path /home/joe/ai/req/requirements-manager/requirements.yaml \
  --description "Requirements management CLI"

# Work on ai-provenance project
cd /home/joe/ai/ai-provenance
export REQ_DB_NAME=ai-provenance

requirements-manager list
ai-prov trace-matrix

# Work on requirements-manager project
cd /home/joe/ai/req/requirements-manager
export REQ_DB_NAME=requirements-manager

requirements-manager list
```

## Benefits of Integration

1. **Single Source of Truth**: All requirements in `requirements.yaml`
2. **Rich CLI**: Use requirements-manager for CRUD operations
3. **Multi-Project**: Registry supports multiple projects
4. **Git Integration**: ai-provenance links commits/files to requirements
5. **Traceability**: Full requirement → code → test mapping
6. **AI Metrics**: Track which requirements are AI-generated
7. **Type Safety**: Rust for requirements, Python for git integration

## Migration Path

For existing ai-provenance users:

1. **Install requirements-manager**:
   ```bash
   cd /home/joe/ai/req/requirements-manager
   cargo install --path .
   ```

2. **Import existing requirements**:
   ```bash
   # Convert .ai-prov/requirements/*.json → requirements.yaml
   requirements-manager import --from ai-prov --path .ai-prov/requirements/
   ```

3. **Update ai-provenance config**:
   ```yaml
   # .ai-prov/config.yaml
   requirements:
     source: requirements-manager
   ```

4. **Continue using ai-prov commands** (they now read from requirements.yaml)

## Next Steps

1. ✅ Document integration architecture (this file)
2. ⬜ Implement export command in requirements-manager
3. ⬜ Create ID mapping system
4. ⬜ Build adapter in ai-provenance
5. ⬜ Add integration tests
6. ⬜ Update documentation in both projects
7. ⬜ Create example project demonstrating integration
