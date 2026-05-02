# Simplified Integration Plan

## New Approach: requirements-manager as the ONLY Requirements System

Since ai-provenance is brand new with no migration concerns, we can **remove its native requirements system entirely** and depend solely on requirements-manager.

## Key Simplifications

### What We Remove from ai-provenance

1. **Delete native requirements storage**
   - Remove `.ai-prov/requirements/` directory structure
   - Remove `RequirementManager` class
   - Remove `requirements/models.py` (Requirement, TestCase, TraceLink)
   - Remove requirement CLI commands

2. **Keep only traceability links**
   - Keep `Trace:` tag parsing in commits
   - Keep test case tracking
   - Keep file → requirement links (stored in git notes)

3. **Delegate all requirements CRUD to requirements-manager**
   - No Python requirements models
   - No JSON storage
   - Just read SPEC-IDs from commit metadata

### What We Keep in ai-provenance

- ✅ Git integration (hooks, notes)
- ✅ Commit metadata parsing
- ✅ Inline code stamping
- ✅ AI attribution tracking
- ✅ Query and reporting commands
- ✅ Traceability matrix generation (reads from requirements.yaml)
- ✅ Validation commands

## New Architecture

```
┌─────────────────────────┐
│  requirements-manager   │  ← Single Source of Truth
│  (Rust)                 │     - All requirements CRUD
│                         │     - Feature management
│  requirements.yaml      │     - Multi-project registry
└───────────┬─────────────┘
            │
            │ ai-provenance reads directly
            │ No export needed!
            ▼
┌─────────────────────────┐
│  ai-provenance          │  ← Pure Traceability Layer
│  (Python)               │     - Git metadata only
│                         │     - Parses requirements.yaml
│  Git notes store:       │     - Links commits to SPEC-IDs
│  - SPEC-ID references   │     - Generates reports
│  - AI metadata          │
└─────────────────────────┘
```

## Data Flow

### Creating a Requirement

```bash
# ONLY way to create requirements
requirements-manager add \
  --title "JWT Authentication" \
  --description "Implement JWT tokens" \
  --feature Authentication

# Output: Created requirement f7d250bf... (SPEC-001)
```

### Linking Code to Requirements

```bash
# Stamp file (ai-provenance)
ai-prov stamp src/auth.py --tool claude --conf high --trace SPEC-001

# Commit (ai-provenance)
ai-prov commit -m "feat: JWT tokens" --trace SPEC-001 --tool claude

# This creates git note with:
# {
#   "ai_tool": "claude",
#   "trace": ["SPEC-001"],  ← Just the SPEC-ID reference
#   ...
# }
```

### Generating Reports

```bash
# ai-provenance reads requirements.yaml directly
ai-prov trace-matrix

# Process:
# 1. Load requirements.yaml (via PyYAML)
# 2. Read git notes for all commits
# 3. Match SPEC-IDs in notes to requirements
# 4. Generate matrix
```

## Implementation Changes

### Phase 1: Clean Up ai-provenance (2 hours)

**Remove Files:**
```bash
cd /home/joe/ai/ai-provenance

# Delete native requirements system
rm -rf src/ai_provenance/requirements/
rm -rf .ai-prov/requirements/  # if exists
```

**Update Files:**

1. **src/ai_provenance/cli/main.py**
   - Remove `@cli.group() requirement` commands
   - Keep `--trace` option parsing (just references SPEC-IDs)

2. **Create new lightweight YAML reader**

`src/ai_provenance/requirements.py` (NEW, simplified):
```python
"""Simple requirements.yaml reader."""

from pathlib import Path
from typing import List, Dict, Optional
import yaml


def load_requirements(yaml_path: str = "requirements.yaml") -> List[Dict]:
    """Load requirements from requirements-manager YAML file."""
    path = Path(yaml_path)

    if not path.exists():
        return []

    with open(path) as f:
        data = yaml.safe_load(f)

    return data.get("requirements", [])


def get_requirement_by_uuid(uuid: str, yaml_path: str = "requirements.yaml") -> Optional[Dict]:
    """Get requirement by UUID."""
    reqs = load_requirements(yaml_path)
    for req in reqs:
        if req.get("id") == uuid:
            return req
    return None


def get_spec_id_for_uuid(uuid: str, mapping_path: str = ".requirements-mapping.yaml") -> Optional[str]:
    """Get SPEC-ID for UUID from mapping file."""
    path = Path(mapping_path)

    if not path.exists():
        return None

    with open(path) as f:
        data = yaml.safe_load(f)

    return data.get("mappings", {}).get(uuid)


def get_uuid_for_spec_id(spec_id: str, mapping_path: str = ".requirements-mapping.yaml") -> Optional[str]:
    """Get UUID for SPEC-ID from mapping file."""
    path = Path(mapping_path)

    if not path.exists():
        return None

    with open(path) as f:
        data = yaml.safe_load(f)

    # Reverse lookup
    mappings = data.get("mappings", {})
    for uuid, sid in mappings.items():
        if sid == spec_id:
            return uuid

    return None
```

That's it! No complex models, no storage layer, just simple YAML reading.

3. **Update reporters/traceability.py**

```python
def generate_traceability_matrix(repo_path: str = ".", format: str = "md") -> str:
    """Generate traceability matrix."""
    from ai_provenance.requirements import load_requirements, get_spec_id_for_uuid
    from ai_provenance.git_integration.notes import get_all_notes

    # Load requirements from requirements.yaml
    requirements = load_requirements()

    # Load mapping
    uuid_to_spec = {}
    for req in requirements:
        uuid = req["id"]
        spec_id = get_spec_id_for_uuid(uuid)
        if spec_id:
            uuid_to_spec[uuid] = spec_id

    # Get all git notes
    notes = get_all_notes(repo_path)

    # Build traceability links
    matrix = {}
    for req in requirements:
        uuid = req["id"]
        spec_id = uuid_to_spec.get(uuid, f"SPEC-???")

        # Find commits that reference this SPEC-ID
        commits = []
        files = set()
        tests = set()

        for commit_sha, note_data in notes.items():
            trace_ids = note_data.get("trace", [])
            if spec_id in trace_ids:
                commits.append(commit_sha)
                files.update(note_data.get("files", []))
                tests.update(note_data.get("tests", []))

        matrix[spec_id] = {
            "title": req["title"],
            "status": req["status"],
            "feature": req.get("feature", ""),
            "commits": commits,
            "files": list(files),
            "tests": list(tests),
        }

    # Format output
    return format_matrix(matrix, format)
```

### Phase 2: Add Export to requirements-manager (3 hours)

Only need to generate the mapping file now (no JSON export needed):

**File: requirements-manager/src/export.rs**

```rust
pub fn generate_mapping_file(
    store: &RequirementsStore,
    output_path: &Path,
) -> Result<()> {
    let mut mappings = HashMap::new();
    let mut next_spec_number = 1;

    // Load existing mapping if present
    if output_path.exists() {
        let existing: MappingFile = serde_yaml::from_str(&fs::read_to_string(output_path)?)?;
        mappings = existing.mappings;
        next_spec_number = existing.next_spec_number;
    }

    // Generate SPEC-IDs for new requirements
    for req in &store.requirements {
        let uuid = req.id.to_string();
        if !mappings.contains_key(&uuid) {
            let spec_id = format!("SPEC-{:03}", next_spec_number);
            mappings.insert(uuid, spec_id);
            next_spec_number += 1;
        }
    }

    // Save mapping
    let mapping_file = MappingFile {
        mappings,
        next_spec_number,
    };

    let yaml = serde_yaml::to_string(&mapping_file)?;
    fs::write(output_path, yaml)?;

    Ok(())
}
```

**CLI command:**

```rust
Command::Export { format, output } => {
    match format.as_str() {
        "mapping" => {
            let output_path = output.unwrap_or_else(|| PathBuf::from(".requirements-mapping.yaml"));
            export::generate_mapping_file(&store, &output_path)?;
            println!("✓ Generated mapping file: {}", output_path.display());
        }
        _ => { /* other formats */ }
    }
}
```

### Phase 3: Update Documentation (1 hour)

Much simpler now!

**User workflow:**

```bash
# 1. Create requirements (requirements-manager)
requirements-manager add -i

# 2. Generate mapping file
requirements-manager export --format mapping

# 3. Work on code with AI assistance

# 4. Stamp and commit (ai-provenance)
ai-prov stamp src/file.py --tool claude --conf high --trace SPEC-001
ai-prov commit -m "feat: implement feature" --trace SPEC-001

# 5. Generate reports (ai-provenance reads requirements.yaml directly)
ai-prov trace-matrix
```

## File Structure (Simplified)

```
project-root/
├── requirements.yaml                  # requirements-manager (source of truth)
├── .requirements-mapping.yaml         # UUID → SPEC-ID only
└── .git/
    └── refs/notes/ai-provenance      # Git notes with SPEC-ID references
```

No `.ai-prov/requirements/` directory needed!

## Benefits of Simplified Approach

1. **Single source of truth**: requirements.yaml is the ONLY place requirements exist
2. **No duplication**: No export, no JSON files, no sync issues
3. **Less code**: Remove entire requirements module from ai-provenance
4. **Faster**: Direct YAML reading is simpler than export → import
5. **Cleaner separation**: requirements-manager = CRUD, ai-provenance = git metadata
6. **Easier testing**: Fewer components to test
7. **Better maintainability**: One place to update requirements

## Migration Steps for ai-provenance

### Step 1: Create requirements for ai-provenance project

```bash
cd /home/joe/ai/ai-provenance

# Register project
requirements-manager db register \
  --name ai-provenance \
  --path $(pwd)/requirements.yaml \
  --description "AI code provenance tracking"

# Create initial requirements based on REQUIREMENTS.md
requirements-manager add \
  --title "Git Integration" \
  --description "Support git hooks, notes, and filters" \
  --feature Core \
  --priority High \
  --status InProgress

# ... add more from REQUIREMENTS.md
```

### Step 2: Generate mapping

```bash
requirements-manager export --format mapping
# Creates .requirements-mapping.yaml
```

### Step 3: Remove native requirements code

```bash
# Delete requirements module
rm -rf src/ai_provenance/requirements/

# Remove requirement commands from CLI
# (edit cli/main.py)
```

### Step 4: Add simple YAML reader

```bash
# Create new lightweight module
# src/ai_provenance/requirements.py (see code above)
```

### Step 5: Update traceability reporter

```bash
# Update reporters/traceability.py to read requirements.yaml
```

### Step 6: Test

```bash
# Verify trace-matrix works
ai-prov trace-matrix

# Should show requirements from requirements.yaml
```

## Estimated Effort

| Phase | Tasks | Time |
|-------|-------|------|
| Clean up ai-provenance | Remove requirements module, add YAML reader | 2 hours |
| Export in requirements-manager | Generate mapping file | 3 hours |
| Update documentation | READMEs, examples | 1 hour |
| **Total** | | **6 hours** |

**Down from 15 hours!**

## What About SPEC-002?

The spec document `specs/requirements/SPEC-002-requirements-management.md` in ai-provenance describes building requirements management **as a feature**.

With this simplified approach:
- ✅ We still have requirements management (via requirements-manager)
- ✅ We still have traceability (SPEC-IDs in git notes)
- ✅ We still have trace matrix generation
- ✅ We still have requirement → code → test links
- ❌ We just delegate the CRUD to requirements-manager instead of building it in Python

This **fulfills the spec** but with better architecture (dedicated tool vs. built-in).

## Summary

**Old Plan:**
- Export from requirements-manager → JSON files
- Import into ai-provenance → Python storage
- Maintain both systems
- Complex sync

**New Plan:**
- Single source: requirements.yaml
- ai-provenance reads it directly
- Simple YAML parser (50 lines)
- No duplication

This is **much cleaner** and achieves the same goal!
