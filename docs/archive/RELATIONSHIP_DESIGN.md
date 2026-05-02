# Relationship Definition Specification

This document outlines the design for a flexible, configurable relationship system for the Requirements Manager.

## Current State

The current implementation has hardcoded relationship types:

```rust
pub enum RelationshipType {
    Parent,      // Hierarchical parent-child
    Child,       // Inverse of Parent
    Duplicate,   // Symmetric duplicate
    Verifies,    // Test/verification link
    VerifiedBy,  // Inverse of Verifies
    References,  // General reference (no inverse)
    Custom(String),  // User-defined (no constraints)
}
```

### Limitations
- No constraints on which requirement types can have certain relationships
- No cardinality constraints (one-to-one, one-to-many, many-to-many)
- Custom relationships have no metadata (inverse, description, etc.)
- No validation rules for relationship creation
- Relationships are not managed/configurable at project level

---

## Proposed Design

### 1. RelationshipDefinition Model

A new model to define relationship types with full metadata:

```rust
/// Defines a relationship type and its constraints
pub struct RelationshipDefinition {
    /// Unique identifier for this relationship type
    pub name: String,

    /// Human-readable display name
    pub display_name: String,

    /// Description of what this relationship means
    pub description: String,

    /// The inverse relationship name (if any)
    /// e.g., "parent" has inverse "child"
    pub inverse: Option<String>,

    /// Whether this relationship is symmetric (A->B implies B->A with same type)
    /// e.g., "duplicate" is symmetric
    pub symmetric: bool,

    /// Cardinality constraints
    pub cardinality: Cardinality,

    /// Source type constraints (which requirement types can be the source)
    /// Empty means all types allowed
    pub source_types: Vec<String>,

    /// Target type constraints (which requirement types can be the target)
    /// Empty means all types allowed
    pub target_types: Vec<String>,

    /// Whether this is a built-in relationship (cannot be deleted)
    pub built_in: bool,

    /// Color for visualization (optional)
    pub color: Option<String>,

    /// Icon/symbol for the relationship (optional)
    pub icon: Option<String>,
}

/// Cardinality constraints for relationships
pub enum Cardinality {
    /// One source to one target (1:1)
    OneToOne,
    /// One source to many targets (1:N)
    OneToMany,
    /// Many sources to one target (N:1)
    ManyToOne,
    /// Many sources to many targets (N:N)
    ManyToMany,
}
```

### 2. Built-in Relationship Definitions

Default relationships that ship with the system:

| Name | Display | Inverse | Symmetric | Cardinality | Description |
|------|---------|---------|-----------|-------------|-------------|
| `parent` | Parent | `child` | No | N:1 | Hierarchical decomposition |
| `child` | Child | `parent` | No | 1:N | Child of parent requirement |
| `verifies` | Verifies | `verified_by` | No | N:N | Test/verification relationship |
| `verified_by` | Verified By | `verifies` | No | N:N | Verified by test requirement |
| `duplicate` | Duplicate | - | Yes | N:N | Marks as duplicate |
| `references` | References | - | No | N:N | General reference link |
| `depends_on` | Depends On | `dependency_of` | No | N:N | Dependency relationship |
| `dependency_of` | Dependency Of | `depends_on` | No | N:N | Inverse dependency |
| `implements` | Implements | `implemented_by` | No | N:N | Implementation relationship |
| `implemented_by` | Implemented By | `implements` | No | N:N | Inverse implementation |

### 3. Type Constraint Examples

Some relationships make sense only between certain requirement types:

```yaml
relationship_definitions:
  - name: verifies
    source_types: [Functional, NonFunctional]  # Tests verify these
    target_types: [System, User]  # These are what get verified

  - name: implements
    source_types: [System]  # System reqs implement...
    target_types: [User, Functional]  # ...user/functional reqs
```

### 4. Storage in requirements.yaml

Add a new top-level section for relationship definitions:

```yaml
relationship_definitions:
  - name: parent
    display_name: Parent
    description: Hierarchical parent requirement
    inverse: child
    symmetric: false
    cardinality: many_to_one
    source_types: []
    target_types: []
    built_in: true

  - name: blocks
    display_name: Blocks
    description: This requirement blocks another
    inverse: blocked_by
    symmetric: false
    cardinality: many_to_many
    source_types: []
    target_types: []
    built_in: false
    color: "#ff6b6b"
```

### 5. API/CLI Commands

New commands for relationship management:

```bash
# List all relationship definitions
req rel-def list

# Add a new relationship definition
req rel-def add --name "blocks" \
    --display-name "Blocks" \
    --description "This requirement blocks another" \
    --inverse "blocked_by" \
    --cardinality many-to-many

# Edit a relationship definition
req rel-def edit "blocks" --color "#ff6b6b"

# Remove a custom relationship definition
req rel-def remove "blocks"

# Show relationship definition details
req rel-def show "blocks"
```

### 6. GUI Integration

- **Settings > Relationships Tab**: Manage relationship definitions
  - List all definitions with edit/delete buttons
  - Add new relationship button
  - Form for editing: name, display name, inverse, cardinality, type constraints
  - Cannot delete built-in relationships

- **Links Tab Enhancement**:
  - Dropdown shows only valid relationship types based on source/target types
  - Visual indicators (colors/icons) for different relationship types
  - Cardinality warnings when exceeding limits

### 7. Validation Rules

When creating/editing relationships:

1. **Type Constraint Validation**: Check if source/target requirement types are allowed
2. **Cardinality Validation**: Warn or prevent exceeding cardinality limits
3. **Self-Reference Prevention**: Some relationships shouldn't allow self-references
4. **Cycle Detection**: For hierarchical relationships (parent/child), detect cycles
5. **Inverse Auto-Creation**: When adding bidirectional, use the correct inverse type

### 8. Migration Path

1. Add `relationship_definitions` to RequirementsStore with built-in defaults
2. Migrate existing `Custom(String)` relationships to definitions if matching
3. Keep backward compatibility: unknown relationship types become Custom
4. Provide `req migrate-relationships` command to clean up

---

## Implementation Phases

### Phase 1: Core Model
- [ ] Add `RelationshipDefinition` struct to models.rs
- [ ] Add `Cardinality` enum
- [ ] Add `relationship_definitions: Vec<RelationshipDefinition>` to RequirementsStore
- [ ] Initialize with built-in defaults on load
- [ ] Update serialization/deserialization

### Phase 2: Validation
- [ ] Add validation function for relationship creation
- [ ] Check type constraints
- [ ] Check cardinality constraints
- [ ] Update `add_relationship()` to use validation

### Phase 3: CLI Commands
- [ ] Add `rel-def` subcommand with list/add/edit/remove/show
- [ ] Update `rel add` to validate against definitions

### Phase 4: GUI Integration
- [ ] Add Relationships tab to Settings dialog
- [ ] Add relationship definition management UI
- [ ] Update Links tab to respect constraints
- [ ] Add visual indicators for relationship types

---

## Questions for Discussion

1. **Cardinality Enforcement**: Should cardinality violations be errors or warnings?
   - Recommendation: Warnings with option to enforce strictly per-relationship

2. **Custom Relationship Auto-Definition**: When using `Custom("foo")`, should we auto-create a definition?
   - Recommendation: Yes, with default settings (no constraints, N:N cardinality)

3. **Type Constraints Scope**: Should constraints use RequirementType enum or also features?
   - Recommendation: Start with RequirementType, extend to features later if needed

4. **Relationship Categories**: Should we group relationships (Structural, Traceability, Dependency)?
   - Recommendation: Add optional `category` field for UI grouping

5. **Inheritance**: Should child requirements inherit parent's relationships?
   - Recommendation: No automatic inheritance, but provide "copy relationships" feature

---

## Example Use Cases

### Use Case 1: Traceability Matrix
Define `traces_to` relationship between System requirements and Test Cases:
```yaml
- name: traces_to
  source_types: [System]
  target_types: [Functional]  # Where test cases live
  cardinality: many_to_many
```

### Use Case 2: Feature Dependencies
Track which features depend on others:
```yaml
- name: feature_depends
  display_name: Feature Dependency
  source_types: []  # Any type
  target_types: []  # Any type
  cardinality: many_to_many
```

### Use Case 3: Approval Chain
One approver per requirement:
```yaml
- name: approved_by
  cardinality: many_to_one  # Many reqs, one approver each
```
