# Adding SPEC-ID as Alternate Key in Requirement Model

## Overview

**Current State**: SPEC-ID exists only in `.requirements-mapping.yaml` (separate mapping file)
**Proposed State**: SPEC-ID stored directly in `Requirement` struct as an optional alternate key

## Benefits

### 1. **Simpler Lookups**
```rust
// Current (requires loading mapping file):
let mapping = MappingFile::load_or_create(path)?;
let uuid = mapping.get_uuid("SPEC-001")?;
let req = store.get_requirement_by_id(&uuid)?;

// Proposed (direct lookup):
let req = store.get_requirement_by_spec_id("SPEC-001")?;
```

### 2. **Self-Contained Requirements**
- Each requirement carries both identifiers
- No external file needed for most operations
- requirements.yaml becomes fully self-documenting

### 3. **Easier Display**
```rust
// Easy to show both IDs in lists:
println!("{} ({}): {}", req.spec_id, req.id, req.title);
// Output: SPEC-001 (607738f8-...): Git Integration
```

### 4. **Better CLI UX**
```bash
# Users can reference either ID:
requirements-manager show SPEC-001
requirements-manager show 607738f8-4764-4943-8a4c-c5943b406d6c

# Both work!
```

### 5. **Backward Compatibility**
- Existing requirements.yaml files will auto-migrate
- Old files without spec_id field will get IDs assigned on load
- Mapping file becomes optional (only for migration/legacy)

## Design

### Modified Requirement Struct

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Requirement {
    /// Unique identifier for the requirement (UUID)
    pub id: Uuid,

    /// Human-friendly specification ID (e.g., "SPEC-001")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_id: Option<String>,

    pub title: String,
    pub description: String,
    pub status: RequirementStatus,
    pub priority: RequirementPriority,
    pub owner: String,
    pub feature: String,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub req_type: RequirementType,
    pub dependencies: Vec<Uuid>,
    pub tags: HashSet<String>,
}
```

### Modified RequirementsStore

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct RequirementsStore {
    pub requirements: Vec<Requirement>,

    #[serde(default = "default_next_feature_number")]
    pub next_feature_number: u32,

    /// Counter for SPEC-ID assignment
    #[serde(default = "default_next_spec_number")]
    pub next_spec_number: u32,
}

impl RequirementsStore {
    /// Gets a requirement by SPEC-ID
    pub fn get_requirement_by_spec_id(&self, spec_id: &str) -> Option<&Requirement> {
        self.requirements.iter().find(|r| {
            r.spec_id.as_ref().map(|s| s.as_str()) == Some(spec_id)
        })
    }

    /// Gets a mutable reference to a requirement by SPEC-ID
    pub fn get_requirement_by_spec_id_mut(&mut self, spec_id: &str) -> Option<&mut Requirement> {
        self.requirements.iter_mut().find(|r| {
            r.spec_id.as_ref().map(|s| s.as_str()) == Some(spec_id)
        })
    }

    /// Assigns SPEC-IDs to requirements that don't have them
    pub fn assign_spec_ids(&mut self) {
        for req in &mut self.requirements {
            if req.spec_id.is_none() {
                req.spec_id = Some(format!("SPEC-{:03}", self.next_spec_number));
                self.next_spec_number += 1;
            }
        }
    }

    /// Gets the next SPEC-ID without assigning it
    pub fn peek_next_spec_id(&self) -> String {
        format!("SPEC-{:03}", self.next_spec_number)
    }

    /// Assigns a specific SPEC-ID to a requirement
    pub fn assign_spec_id_to_requirement(&mut self, uuid: &Uuid, spec_id: String) -> Result<()> {
        // Check if SPEC-ID already exists
        if self.get_requirement_by_spec_id(&spec_id).is_some() {
            anyhow::bail!("SPEC-ID {} already assigned", spec_id);
        }

        // Find requirement and assign
        if let Some(req) = self.get_requirement_by_id_mut(uuid) {
            req.spec_id = Some(spec_id);
            Ok(())
        } else {
            anyhow::bail!("Requirement with UUID {} not found", uuid);
        }
    }
}
```

### Modified Requirement Creation

```rust
impl Requirement {
    /// Creates a new requirement with auto-generated SPEC-ID
    pub fn new_with_spec_id(
        title: String,
        description: String,
        spec_id: String
    ) -> Self {
        let now = Utc::now();
        let default_feature = env::var("REQ_FEATURE")
            .unwrap_or_else(|_| String::from("Uncategorized"));

        Self {
            id: Uuid::new_v4(),
            spec_id: Some(spec_id),  // NEW
            title,
            description,
            status: RequirementStatus::Draft,
            priority: RequirementPriority::Medium,
            owner: String::new(),
            feature: default_feature,
            created_at: now,
            modified_at: now,
            req_type: RequirementType::Functional,
            dependencies: Vec::new(),
            tags: HashSet::new(),
        }
    }
}
```

## Data Format

### New requirements.yaml Format

```yaml
requirements:
- id: 607738f8-4764-4943-8a4c-c5943b406d6c
  spec_id: SPEC-001  # NEW FIELD
  title: Git Integration
  description: Support git hooks, notes, and filters
  status: Draft
  priority: High
  owner: ''
  feature: 1-Core
  created_at: 2025-11-23T03:31:08Z
  modified_at: 2025-11-23T03:31:08Z
  req_type: Functional
  dependencies: []
  tags: []

- id: e367b4cd-d679-482c-b642-a507faafe6f1
  spec_id: SPEC-002  # NEW FIELD
  title: Traceability Matrix
  description: Generate traceability matrices
  status: Draft
  priority: Medium
  owner: ''
  feature: Reporting
  created_at: 2025-11-23T03:31:14Z
  modified_at: 2025-11-23T03:31:14Z
  req_type: Functional
  dependencies: []
  tags: []

next_feature_number: 2
next_spec_number: 3  # NEW FIELD (next available SPEC-ID)
```

## Migration Strategy

### Automatic Migration on Load

```rust
impl Storage {
    pub fn load(&self) -> Result<RequirementsStore> {
        let mut store = self.load_raw()?;

        // Migrate features (existing)
        store.migrate_features();

        // NEW: Migrate SPEC-IDs
        store.assign_spec_ids();

        // Save if any migrations occurred
        if store.requirements.iter().any(|r| r.spec_id.is_some()) {
            self.save(&store)?;
        }

        Ok(store)
    }
}
```

### Import from Existing Mapping File

```rust
pub fn import_spec_ids_from_mapping(
    store: &mut RequirementsStore,
    mapping_path: &Path
) -> Result<()> {
    if !mapping_path.exists() {
        return Ok(());
    }

    let mapping = MappingFile::load_or_create(mapping_path)?;

    for req in &mut store.requirements {
        let uuid_str = req.id.to_string();
        if let Some(spec_id) = mapping.mappings.get(&uuid_str) {
            req.spec_id = Some(spec_id.clone());
        }
    }

    // Update next_spec_number based on highest SPEC-ID
    store.next_spec_number = mapping.next_spec_number;

    println!("Imported SPEC-IDs from {}", mapping_path.display());
    Ok(())
}
```

## Command Changes

### Enhanced CLI Commands

```rust
// Accept either UUID or SPEC-ID
pub fn parse_requirement_id(
    id_str: &str,
    store: &RequirementsStore
) -> Result<Uuid> {
    // Try parsing as UUID first
    if let Ok(uuid) = Uuid::parse_str(id_str) {
        return Ok(uuid);
    }

    // Try as SPEC-ID
    if let Some(req) = store.get_requirement_by_spec_id(id_str) {
        return Ok(req.id);
    }

    anyhow::bail!("Invalid requirement ID: {}", id_str)
}
```

### Updated Commands

```bash
# All these work:
requirements-manager show SPEC-001
requirements-manager show 607738f8-4764-4943-8a4c-c5943b406d6c

requirements-manager edit SPEC-001 --title "New Title"
requirements-manager delete SPEC-042

requirements-manager list --feature Core
# Output now shows both IDs:
# SPEC-001 (607738f8-...): Git Integration [Draft] [High]
```

### New Command: Renumber SPEC-IDs

```rust
// If users want to renumber (optional, rarely needed)
requirements-manager renumber-specs --start 1

// Reassigns all SPEC-IDs sequentially starting from SPEC-001
```

## Compatibility

### Backward Compatibility

✅ **Old requirements.yaml files**:
- Files without `spec_id` field will auto-migrate on load
- SPEC-IDs assigned sequentially (SPEC-001, SPEC-002, ...)
- Saved back with new format

✅ **Existing mapping files**:
- Can import SPEC-IDs from `.requirements-mapping.yaml`
- Preserves existing SPEC-ID assignments
- Command: `requirements-manager import-mapping`

✅ **ai-provenance integration**:
- ai-provenance can still read `spec_id` field directly from requirements.yaml
- No changes needed to Python code (already reads YAML)
- Mapping file becomes optional

### Forward Compatibility

✅ **Unique SPEC-IDs enforced**:
- Cannot assign same SPEC-ID to two requirements
- Validation on load and save

✅ **Persistent numbering**:
- `next_spec_number` tracks highest assigned number
- New requirements get next sequential ID

✅ **Optional field**:
- `spec_id` is `Option<String>` - can be None
- Serialization skips if None (keeps YAML clean)

## Testing Strategy

### Unit Tests

```rust
#[test]
fn test_assign_spec_ids() {
    let mut store = RequirementsStore::new();
    store.add_requirement(Requirement::new("R1".into(), "D1".into()));
    store.add_requirement(Requirement::new("R2".into(), "D2".into()));

    assert!(store.requirements[0].spec_id.is_none());
    assert!(store.requirements[1].spec_id.is_none());

    store.assign_spec_ids();

    assert_eq!(store.requirements[0].spec_id, Some("SPEC-001".into()));
    assert_eq!(store.requirements[1].spec_id, Some("SPEC-002".into()));
    assert_eq!(store.next_spec_number, 3);
}

#[test]
fn test_get_requirement_by_spec_id() {
    let mut store = RequirementsStore::new();
    let mut req = Requirement::new("Test".into(), "Desc".into());
    req.spec_id = Some("SPEC-001".into());
    store.add_requirement(req);

    let found = store.get_requirement_by_spec_id("SPEC-001");
    assert!(found.is_some());
    assert_eq!(found.unwrap().title, "Test");
}

#[test]
fn test_duplicate_spec_id_rejected() {
    let mut store = RequirementsStore::new();
    let mut req1 = Requirement::new("R1".into(), "D1".into());
    req1.spec_id = Some("SPEC-001".into());
    let mut req2 = Requirement::new("R2".into(), "D2".into());
    req2.spec_id = Some("SPEC-001".into());  // Duplicate!

    store.add_requirement(req1);
    let result = store.validate_unique_spec_ids();
    assert!(result.is_ok());

    store.add_requirement(req2);
    let result = store.validate_unique_spec_ids();
    assert!(result.is_err());
}
```

### Integration Tests

```rust
#[test]
fn test_migration_from_old_format() {
    // Create old format YAML (no spec_id field)
    let yaml = r#"
requirements:
- id: 607738f8-4764-4943-8a4c-c5943b406d6c
  title: Test
  description: Test
  status: Draft
  priority: High
  owner: ''
  feature: Core
  created_at: 2025-11-23T03:31:08Z
  modified_at: 2025-11-23T03:31:08Z
  req_type: Functional
  dependencies: []
  tags: []
next_feature_number: 1
"#;

    // Load and migrate
    let mut store: RequirementsStore = serde_yaml::from_str(yaml).unwrap();
    store.assign_spec_ids();

    // Verify SPEC-ID assigned
    assert_eq!(store.requirements[0].spec_id, Some("SPEC-001".into()));
}
```

## Implementation Plan

### Phase 1: Core Model Changes (2 hours)
- [ ] Add `spec_id: Option<String>` to `Requirement` struct
- [ ] Add `next_spec_number: u32` to `RequirementsStore`
- [ ] Implement `assign_spec_ids()` method
- [ ] Implement `get_requirement_by_spec_id()` methods
- [ ] Add unit tests for new functionality

### Phase 2: Migration & Validation (1 hour)
- [ ] Auto-assign SPEC-IDs on load for requirements without them
- [ ] Add SPEC-ID uniqueness validation
- [ ] Create `import_spec_ids_from_mapping()` function
- [ ] Add migration tests

### Phase 3: CLI Integration (1.5 hours)
- [ ] Update `parse_requirement_id()` to accept SPEC-ID
- [ ] Modify `show` command to accept SPEC-ID
- [ ] Modify `edit` command to accept SPEC-ID
- [ ] Modify `delete` command to accept SPEC-ID
- [ ] Update `list` command output to show SPEC-ID
- [ ] Add `import-mapping` command

### Phase 4: Display & UX (0.5 hours)
- [ ] Update all output to show both UUID and SPEC-ID
- [ ] Format as: `SPEC-001 (607738f8-...): Title`
- [ ] Add color coding for SPEC-IDs in terminal output

### Phase 5: Documentation (1 hour)
- [ ] Update README with SPEC-ID examples
- [ ] Update CLAUDE.md with data model changes
- [ ] Add migration guide for existing users
- [ ] Update integration docs

**Total Estimated Time**: 6 hours

## Benefits Summary

✅ **Simpler**: No external mapping file needed for lookups
✅ **Faster**: Direct lookup by SPEC-ID without loading mapping
✅ **Self-documenting**: requirements.yaml shows both identifiers
✅ **Better UX**: Users can use either UUID or SPEC-ID in commands
✅ **Backward compatible**: Old files auto-migrate
✅ **Maintains integration**: ai-provenance can read spec_id directly

## Risks & Mitigations

### Risk: Breaking existing files
**Mitigation**: `spec_id` is optional field, old files load fine

### Risk: SPEC-ID conflicts during manual editing
**Mitigation**: Validation on load/save rejects duplicates

### Risk: Mapping file out of sync
**Mitigation**: Mapping file becomes read-only legacy import source

### Risk: Performance with large files
**Mitigation**: SPEC-ID lookup is O(n) but typically <1000 requirements

## Recommendation

**✅ IMPLEMENT THIS CHANGE**

The benefits significantly outweigh the implementation cost. The system becomes simpler, faster, and more user-friendly while maintaining full backward compatibility.

**Suggested Timeline**:
- Implement in requirements-manager first (6 hours)
- Test with existing integration (1 hour)
- Update ai-provenance to read spec_id directly (optional, 0.5 hours)
- Total: ~7.5 hours

**Priority**: Medium-High
- Not blocking current usage
- Significantly improves UX
- Natural evolution of the system
