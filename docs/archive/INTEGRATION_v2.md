# Integration v2: Simplified Architecture

## Overview

This document describes the **simplified integration** between requirements-manager (Rust) and ai-provenance (Python).

Since ai-provenance is a brand new project with no migration concerns, we **remove its native requirements system entirely** and use requirements-manager as the single source of truth.

## Architecture

```
┌─────────────────────────────┐
│   requirements-manager      │
│   (Rust CLI)                │
│                             │
│   ✓ Create requirements     │  ← ONLY place to manage requirements
│   ✓ Edit requirements       │
│   ✓ Feature organization    │
│   ✓ Multi-project registry  │
│                             │
│   requirements.yaml         │  ← Single source of truth
└──────────────┬──────────────┘
               │
               │ Direct read (no export needed!)
               │
               ▼
┌─────────────────────────────┐
│   ai-provenance             │
│   (Python CLI)              │
│                             │
│   ✓ Read requirements.yaml  │  ← Simple YAML parser
│   ✓ Git metadata tracking   │
│   ✓ Link commits to SPEC-IDs│
│   ✓ Traceability reports    │
│   ✓ AI attribution         │
│                             │
│   Git notes:                │
│   refs/notes/ai-provenance  │
└─────────────────────────────┘
```

## What ai-provenance Does NOT Do

- ❌ Store requirements in JSON files
- ❌ Maintain its own requirements database
- ❌ Provide requirement CRUD commands
- ❌ Duplicate requirements data

## What ai-provenance DOES Do

- ✅ Read requirements.yaml directly (simple YAML parsing)
- ✅ Parse SPEC-IDs from git commit metadata
- ✅ Link commits/files to requirements (via git notes)
- ✅ Generate traceability matrices
- ✅ Track AI attribution per requirement
- ✅ Calculate AI % by requirement
- ✅ Validate test coverage

## File Structure

```
project-root/
├── requirements.yaml              # Source of truth (requirements-manager)
├── .requirements-mapping.yaml     # UUID → SPEC-ID mapping
├── .git/
│   └── refs/notes/ai-provenance  # Git notes with SPEC-ID references
└── src/
    └── *.py                      # Code with ai:tags
```

**No `.ai-prov/requirements/` directory!**

## Workflows

### Workflow 1: Create and Track Requirements

```bash
# 1. Create requirement (requirements-manager ONLY)
requirements-manager add \
  --title "JWT Authentication System" \
  --description "Implement JWT-based auth with refresh tokens" \
  --feature Authentication \
  --priority High

# Output: Created requirement <uuid>

# 2. Generate SPEC-ID mapping
requirements-manager export --format mapping
# Creates/updates .requirements-mapping.yaml
# Assigns SPEC-001, SPEC-002, etc.

# 3. Work on code (AI-assisted)

# 4. Stamp file with metadata (ai-provenance)
ai-prov stamp src/auth.py \
  --tool claude \
  --conf high \
  --trace SPEC-001

# 5. Commit with traceability (ai-provenance)
ai-prov commit \
  -m "feat(auth): implement JWT token generation" \
  --tool claude \
  --conf high \
  --trace SPEC-001 \
  --test TC-101

# This creates git note:
# {
#   "ai_tool": "claude",
#   "confidence": "high",
#   "trace": ["SPEC-001"],  ← Reference to requirement
#   "tests": ["TC-101"],
#   "files": ["src/auth.py"]
# }

# 6. Generate traceability matrix (ai-provenance)
ai-prov trace-matrix

# Process:
# - Reads requirements.yaml
# - Reads .requirements-mapping.yaml
# - Reads git notes
# - Matches SPEC-IDs
# - Shows: SPEC-001 → commits → files → tests → AI %
```

### Workflow 2: Update Requirement Status

```bash
# Code is complete and tested

# Update status (requirements-manager)
requirements-manager edit <uuid> --status Completed

# Verify traceability
ai-prov trace-matrix
# Now shows SPEC-001 as "Completed"
```

### Workflow 3: Multi-Project Development

```bash
# Register multiple projects
requirements-manager db register \
  --name ai-provenance \
  --path ~/ai/ai-provenance/requirements.yaml

requirements-manager db register \
  --name web-app \
  --path ~/projects/web-app/requirements.yaml

# Work on ai-provenance
cd ~/ai/ai-provenance
export REQ_DB_NAME=ai-provenance

requirements-manager list
requirements-manager export --format mapping
ai-prov trace-matrix

# Work on web-app
cd ~/projects/web-app
export REQ_DB_NAME=web-app

requirements-manager list
requirements-manager export --format mapping
ai-prov trace-matrix
```

## Implementation Details

### requirements-manager Changes

**Add mapping export command:**

```rust
// src/cli.rs
Export {
    #[arg(long, default_value = "mapping")]
    format: String,

    #[arg(long, short)]
    output: Option<PathBuf>,
}
```

```rust
// src/export.rs
pub fn generate_mapping_file(
    store: &RequirementsStore,
    output_path: &Path,
) -> Result<()> {
    // Load existing mapping or create new
    // For each requirement UUID → assign SPEC-{N}
    // Save to YAML
}
```

**Mapping file format:**

```yaml
mappings:
  f7d250bf-5b3e-4ec3-8bd5-2bee2c4b7bb9: SPEC-001
  013cc55c-3123-42b2-81df-d930cf9eb288: SPEC-002
next_spec_number: 3
```

### ai-provenance Changes

**Remove:**
- `src/ai_provenance/requirements/` (entire module)
- Requirement CLI commands from `cli/main.py`
- `.ai-prov/requirements/` directory

**Add:**

Simple YAML reader (`src/ai_provenance/requirements.py`):

```python
"""Lightweight requirements.yaml reader."""

from pathlib import Path
from typing import List, Dict, Optional
import yaml


def load_requirements(yaml_path: str = "requirements.yaml") -> List[Dict]:
    """Load requirements from requirements-manager YAML."""
    path = Path(yaml_path)
    if not path.exists():
        return []

    with open(path) as f:
        data = yaml.safe_load(f)

    return data.get("requirements", [])


def load_mapping(mapping_path: str = ".requirements-mapping.yaml") -> Dict[str, str]:
    """Load UUID → SPEC-ID mapping."""
    path = Path(mapping_path)
    if not path.exists():
        return {}

    with open(path) as f:
        data = yaml.safe_load(f)

    return data.get("mappings", {})


def get_requirement_by_spec_id(spec_id: str) -> Optional[Dict]:
    """Get requirement by SPEC-ID."""
    requirements = load_requirements()
    mapping = load_mapping()

    # Reverse lookup: SPEC-ID → UUID
    uuid = None
    for u, s in mapping.items():
        if s == spec_id:
            uuid = u
            break

    if not uuid:
        return None

    # Find requirement by UUID
    for req in requirements:
        if req["id"] == uuid:
            return req

    return None
```

**Update traceability reporter:**

```python
# src/ai_provenance/reporters/traceability.py

from ai_provenance import requirements as req_reader

def generate_traceability_matrix(repo_path: str = ".", format: str = "md") -> str:
    # Load requirements and mapping
    requirements = req_reader.load_requirements()
    mapping = req_reader.load_mapping()

    # Get git notes
    notes = get_all_notes(repo_path)

    # Build matrix
    matrix = {}
    for req_data in requirements:
        uuid = req_data["id"]
        spec_id = mapping.get(uuid, "SPEC-???")

        # Find commits referencing this SPEC-ID
        commits = []
        files = set()
        tests = set()

        for commit_sha, note_data in notes.items():
            if spec_id in note_data.get("trace", []):
                commits.append(commit_sha)
                files.update(note_data.get("files", []))
                tests.update(note_data.get("tests", []))

        # Calculate AI %
        ai_percent = calculate_ai_percentage(files)

        matrix[spec_id] = {
            "title": req_data["title"],
            "status": req_data["status"],
            "feature": req_data.get("feature", ""),
            "commits": commits,
            "files": list(files),
            "tests": list(tests),
            "ai_percent": ai_percent,
        }

    return format_matrix(matrix, format)
```

## Benefits

### Compared to Original Integration Plan

| Aspect | Original Plan | Simplified Plan |
|--------|---------------|-----------------|
| ai-provenance requirements module | 500+ lines | **50 lines** |
| Export mechanism | Complex JSON export | **Simple mapping only** |
| Data duplication | requirements.yaml + JSON files | **requirements.yaml only** |
| Sync issues | Possible | **None** |
| Implementation time | 15 hours | **6 hours** |
| Maintenance | Two systems | **One system** |

### For Users

1. **Single command** to create requirements (requirements-manager)
2. **No export needed** for most operations (just mapping file)
3. **No confusion** about where requirements live
4. **Faster** - no JSON generation, just YAML parsing
5. **Cleaner** - one file to manage

### For Developers

1. **Less code** to maintain
2. **Simpler architecture**
3. **Clear boundaries** - requirements-manager = CRUD, ai-provenance = git
4. **Easier testing**
5. **No sync bugs**

## Migration for ai-provenance Project

### Step 1: Create requirements.yaml

```bash
cd /home/joe/ai/ai-provenance

# Register project
requirements-manager db register \
  --name ai-provenance \
  --path $(pwd)/requirements.yaml

# Import from REQUIREMENTS.md (manual or scripted)
# For each functional requirement:
requirements-manager add \
  --title "Inline Metadata Support" \
  --description "Support inline metadata comments in all languages" \
  --feature "Core-Tracking" \
  --priority High \
  --status InProgress
```

### Step 2: Generate mapping

```bash
requirements-manager export --format mapping
# Creates .requirements-mapping.yaml with SPEC-001, SPEC-002, etc.
```

### Step 3: Update codebase

```bash
# Remove old requirements module
rm -rf src/ai_provenance/requirements/

# Create new lightweight reader
# (see code above)

# Update reporters
# (update traceability.py to use new reader)

# Remove requirement commands from CLI
# (edit cli/main.py)
```

### Step 4: Test

```bash
# Should work with requirements.yaml
ai-prov trace-matrix

# Commit with traceability
ai-prov commit -m "refactor: simplify requirements integration" --trace SPEC-001
```

## Commands Reference

### requirements-manager (Requirements CRUD)

```bash
# Add requirement
requirements-manager add -i

# List requirements
requirements-manager list [--feature FEATURE] [--status STATUS]

# Show requirement
requirements-manager show <UUID>

# Edit requirement
requirements-manager edit <UUID> --status Completed

# Generate SPEC-ID mapping
requirements-manager export --format mapping

# Multi-project
requirements-manager db register --name NAME --path PATH
requirements-manager db list
```

### ai-provenance (Git Metadata & Traceability)

```bash
# Initialize repo
ai-prov init

# Stamp file
ai-prov stamp FILE --tool TOOL --conf CONF --trace SPEC-ID

# Commit with metadata
ai-prov commit -m MSG --tool TOOL --trace SPEC-ID --test TEST-ID

# Generate traceability matrix
ai-prov trace-matrix [--format md|json|html]

# Query
ai-prov query --ai-percent
ai-prov query --unreviewed
ai-prov query --trace SPEC-001

# Validate
ai-prov validate --require-tests --require-review

# Report
ai-prov report FILE
```

## Next Steps

1. ✅ Review simplified plan
2. ⬜ Implement mapping export in requirements-manager (3 hours)
3. ⬜ Simplify ai-provenance (remove requirements module) (2 hours)
4. ⬜ Update documentation (1 hour)
5. ⬜ Test integration (1 hour)

**Total: ~6 hours** (down from 15!)

## Questions?

- **Q: What if I want to query requirements?**
  - A: Use `requirements-manager list`, `requirements-manager show`

- **Q: What if requirements.yaml doesn't exist?**
  - A: ai-provenance gracefully handles it (empty requirements list)

- **Q: Can I still use git notes for traceability?**
  - A: Yes! That's the core of ai-provenance

- **Q: Do I need to export every time I change requirements?**
  - A: Only when you add NEW requirements (to generate SPEC-IDs). Changes to existing requirements are picked up automatically since ai-provenance reads requirements.yaml directly.

## Summary

This simplified approach:
- ✅ Uses requirements-manager as the ONLY requirements system
- ✅ ai-provenance reads requirements.yaml directly (50 lines of code)
- ✅ No duplication, no sync issues
- ✅ 60% less implementation time
- ✅ Cleaner architecture
- ✅ Same functionality as original plan

**Recommendation: Adopt this simplified approach.**
