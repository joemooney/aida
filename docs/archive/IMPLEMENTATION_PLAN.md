# Implementation Plan: requirements-manager + ai-provenance Integration

## Overview

This document outlines the implementation tasks needed to integrate requirements-manager (Rust) with ai-provenance (Python).

## Implementation Phases

### Phase 1: Export Functionality (requirements-manager)

**Goal**: Add ability to export requirements in ai-provenance-compatible format

#### Task 1.1: Add Export Command to CLI

**File**: `requirements-manager/src/cli.rs`

```rust
#[derive(Subcommand)]
pub enum Command {
    // ... existing commands

    /// Export requirements to different formats
    Export {
        /// Output format (json, ai-prov, csv)
        #[arg(long, short, default_value = "json")]
        format: String,

        /// Output directory or file
        #[arg(long, short)]
        output: Option<PathBuf>,

        /// Include mapping file for ai-prov format
        #[arg(long, default_value = "true")]
        include_mapping: bool,
    },
}
```

**Acceptance Criteria**:
- [ ] CLI accepts `export` command
- [ ] Supports `--format` flag with values: json, ai-prov, csv
- [ ] Supports `--output` flag for custom output path
- [ ] Supports `--include-mapping` flag

**Estimated Effort**: 30 minutes

---

#### Task 1.2: Create Export Module

**File**: `requirements-manager/src/export.rs` (new file)

```rust
use crate::models::{Requirement, RequirementsStore};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct SpecIdMapping {
    pub uuid: String,
    pub spec_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MappingFile {
    pub mappings: HashMap<String, String>,  // UUID -> SPEC-ID
    pub next_spec_number: u32,
}

pub fn export_json(store: &RequirementsStore, output: &Path) -> Result<()> {
    // Export as JSON
}

pub fn export_ai_prov(
    store: &RequirementsStore,
    output_dir: &Path,
    include_mapping: bool
) -> Result<()> {
    // 1. Load or create mapping file
    // 2. For each requirement:
    //    - Convert to ai-provenance format
    //    - Assign SPEC-{N} ID (or use existing from mapping)
    //    - Write to output_dir/SPEC-{N}.json
    // 3. Save mapping file if include_mapping
}

pub fn export_csv(store: &RequirementsStore, output: &Path) -> Result<()> {
    // Export as CSV
}

fn load_or_create_mapping(mapping_path: &Path) -> Result<MappingFile> {
    // Load existing mapping or create new
}

fn uuid_to_spec_id(
    uuid: &str,
    mapping: &mut MappingFile
) -> String {
    // Get existing SPEC-ID or generate new one
    mapping.mappings
        .entry(uuid.to_string())
        .or_insert_with(|| {
            let spec_id = format!("SPEC-{:03}", mapping.next_spec_number);
            mapping.next_spec_number += 1;
            spec_id
        })
        .clone()
}

fn convert_to_ai_prov_format(
    req: &Requirement,
    spec_id: &str
) -> serde_json::Value {
    // Convert Requirement to ai-provenance JSON format
    // Map enums, fields, etc.
}
```

**Acceptance Criteria**:
- [ ] `export_ai_prov()` creates `.ai-prov/requirements/` directory
- [ ] Generates SPEC-{N}.json files for each requirement
- [ ] Creates `.requirements-mapping.yaml` with UUID↔SPEC-ID mappings
- [ ] Mapping is persistent across exports (reuses existing SPEC-IDs)
- [ ] Enum mappings match integration spec

**Estimated Effort**: 2 hours

---

#### Task 1.3: Implement Export Handler in main.rs

**File**: `requirements-manager/src/main.rs`

```rust
// Add export module
mod export;

// In main() match Command
Command::Export { format, output, include_mapping } => {
    let store = storage::load(&project_path)?;

    match format.as_str() {
        "json" => {
            let output_path = output.unwrap_or_else(|| PathBuf::from("requirements.json"));
            export::export_json(&store, &output_path)?;
            println!("Exported to {}", output_path.display());
        }
        "ai-prov" => {
            let output_dir = output.unwrap_or_else(|| PathBuf::from(".ai-prov/requirements"));
            export::export_ai_prov(&store, &output_dir, include_mapping)?;
            println!("Exported to {}", output_dir.display());
        }
        "csv" => {
            let output_path = output.unwrap_or_else(|| PathBuf::from("requirements.csv"));
            export::export_csv(&store, &output_path)?;
            println!("Exported to {}", output_path.display());
        }
        _ => {
            eprintln!("Unknown format: {}", format);
            std::process::exit(1);
        }
    }
}
```

**Acceptance Criteria**:
- [ ] Export command executes without errors
- [ ] Success message printed
- [ ] Error handling for invalid formats

**Estimated Effort**: 30 minutes

---

#### Task 1.4: Add Unit Tests for Export

**File**: `requirements-manager/src/export.rs` (tests module)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uuid_to_spec_id_new() {
        // Test generating new SPEC-ID
    }

    #[test]
    fn test_uuid_to_spec_id_existing() {
        // Test reusing existing SPEC-ID from mapping
    }

    #[test]
    fn test_convert_to_ai_prov_format() {
        // Test requirement conversion
    }

    #[test]
    fn test_export_ai_prov() {
        // End-to-end export test
    }

    #[test]
    fn test_mapping_persistence() {
        // Test that mappings persist across exports
    }
}
```

**Acceptance Criteria**:
- [ ] All tests pass
- [ ] Test coverage > 80%

**Estimated Effort**: 1 hour

---

### Phase 2: ID Mapping System

**Goal**: Maintain bidirectional UUID ↔ SPEC-ID mapping

#### Task 2.1: Define Mapping File Format

**File**: `.requirements-mapping.yaml`

```yaml
# UUID to SPEC-ID mappings
mappings:
  f7d250bf-5b3e-4ec3-8bd5-2bee2c4b7bb9: SPEC-001
  013cc55c-3123-42b2-81df-d930cf9eb288: SPEC-002
  819258b2-9624-4046-9f52-0b00af26c046: SPEC-003

# Next SPEC number to assign
next_spec_number: 4

# Metadata
created_at: 2025-11-22T10:00:00Z
updated_at: 2025-11-22T11:30:00Z
```

**Acceptance Criteria**:
- [ ] YAML format is valid
- [ ] Can be parsed by both Rust and Python
- [ ] Includes metadata timestamps

**Estimated Effort**: 15 minutes

---

#### Task 2.2: Implement Mapping Module (Rust)

**File**: `requirements-manager/src/mapping.rs` (new file)

```rust
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct MappingFile {
    pub mappings: HashMap<String, String>,
    pub next_spec_number: u32,
}

impl MappingFile {
    pub fn load(path: &Path) -> Result<Self> {
        // Load from YAML
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        // Save to YAML
    }

    pub fn get_or_create_spec_id(&mut self, uuid: &str) -> String {
        // Get existing or generate new SPEC-ID
    }

    pub fn get_uuid(&self, spec_id: &str) -> Option<&String> {
        // Reverse lookup: SPEC-ID → UUID
    }
}
```

**Acceptance Criteria**:
- [ ] Loads existing mapping file
- [ ] Creates new mapping if not exists
- [ ] Generates sequential SPEC-IDs
- [ ] Supports bidirectional lookup

**Estimated Effort**: 1 hour

---

#### Task 2.3: Add Mapping Commands to CLI

**File**: `requirements-manager/src/cli.rs`

```rust
#[derive(Subcommand)]
pub enum Command {
    // ... existing commands

    /// Mapping management
    Mapping {
        #[command(subcommand)]
        command: MappingCommand,
    },
}

#[derive(Subcommand)]
pub enum MappingCommand {
    /// Show UUID for SPEC-ID
    Lookup {
        spec_id: String,
    },

    /// Show SPEC-ID for UUID
    Reverse {
        uuid: String,
    },

    /// Regenerate mapping file
    Regenerate {
        /// Force regeneration
        #[arg(long)]
        force: bool,
    },
}
```

**Acceptance Criteria**:
- [ ] `mapping lookup SPEC-001` shows UUID
- [ ] `mapping reverse <uuid>` shows SPEC-ID
- [ ] `mapping regenerate` creates new mapping

**Estimated Effort**: 45 minutes

---

### Phase 3: ai-provenance Adapter

**Goal**: Enable ai-provenance to read requirements from requirements.yaml

#### Task 3.1: Create Adapter Module

**File**: `ai-provenance/src/ai_provenance/requirements/adapters.py`

```python
"""Adapters for external requirements sources."""

from pathlib import Path
from typing import List, Optional, Dict
import yaml

from ai_provenance.requirements.models import (
    Requirement,
    RequirementStatus,
    RequirementType,
    RequirementPriority,
)


class RequirementsManagerAdapter:
    """Adapter to read from requirements-manager YAML files."""

    def __init__(
        self,
        yaml_path: str = "requirements.yaml",
        mapping_path: str = ".requirements-mapping.yaml",
    ):
        self.yaml_path = Path(yaml_path)
        self.mapping_path = Path(mapping_path)
        self._mapping: Optional[Dict] = None

    def load_requirements(self) -> List[Requirement]:
        """Load requirements from requirements.yaml."""
        # Parse YAML
        # Load mapping
        # Convert each requirement to ai-provenance format
        pass

    def get_requirement(self, spec_id: str) -> Optional[Requirement]:
        """Get requirement by SPEC-ID."""
        pass

    def get_spec_id(self, uuid: str) -> Optional[str]:
        """Convert UUID to SPEC-ID."""
        pass

    def get_uuid(self, spec_id: str) -> Optional[str]:
        """Convert SPEC-ID to UUID."""
        pass

    def _load_mapping(self) -> Dict:
        """Load UUID <-> SPEC-ID mapping."""
        if self.mapping_path.exists():
            with open(self.mapping_path) as f:
                return yaml.safe_load(f)
        return {"mappings": {}, "next_spec_number": 1}

    def _convert_requirement(self, req_data: Dict, spec_id: str) -> Requirement:
        """Convert requirements-manager format to ai-provenance."""
        # Map fields
        # Convert enums
        # Return Requirement object
        pass

    @staticmethod
    def _map_status(status: str) -> RequirementStatus:
        """Map requirements-manager status to ai-provenance."""
        mapping = {
            "Draft": RequirementStatus.PLANNED,
            "Approved": RequirementStatus.PLANNED,
            "InProgress": RequirementStatus.IN_PROGRESS,
            "Completed": RequirementStatus.IMPLEMENTED,
            "Rejected": RequirementStatus.DEPRECATED,
        }
        return mapping.get(status, RequirementStatus.PLANNED)

    # Similar for _map_priority, _map_type
```

**Acceptance Criteria**:
- [ ] Loads requirements.yaml successfully
- [ ] Loads mapping file successfully
- [ ] Converts all requirement fields correctly
- [ ] Enum mappings match integration spec
- [ ] Handles missing/optional fields

**Estimated Effort**: 2 hours

---

#### Task 3.2: Update RequirementManager

**File**: `ai-provenance/src/ai_provenance/requirements/manager.py`

```python
from ai_provenance.requirements.adapters import RequirementsManagerAdapter

class RequirementManager:
    def __init__(self, repo_path: Optional[str] = None, source: str = "native"):
        """
        Initialize requirements manager.

        Args:
            repo_path: Path to repository
            source: 'native' or 'requirements-manager'
        """
        self.source = source

        if source == "requirements-manager":
            self.adapter = RequirementsManagerAdapter()
        else:
            # Use native .ai-prov/requirements/ storage
            self.adapter = None

        # ... rest of init

    def list_requirements(self, **filters) -> List[Requirement]:
        """List requirements from configured source."""
        if self.adapter:
            return self.adapter.load_requirements()
        else:
            # Use native storage
            pass
```

**Acceptance Criteria**:
- [ ] Supports both `native` and `requirements-manager` sources
- [ ] Reads from requirements.yaml when source=requirements-manager
- [ ] Backwards compatible with native storage

**Estimated Effort**: 1 hour

---

#### Task 3.3: Add Configuration Support

**File**: `.ai-prov/config.yaml`

```yaml
requirements:
  # Source: 'native' or 'requirements-manager'
  source: requirements-manager

  # Path to requirements file (for requirements-manager source)
  path: requirements.yaml

  # Path to mapping file
  mapping: .requirements-mapping.yaml
```

**File**: `ai-provenance/src/ai_provenance/core/config.py` (new)

```python
from pathlib import Path
from typing import Optional
import yaml
from pydantic import BaseModel


class RequirementsConfig(BaseModel):
    source: str = "native"
    path: str = "requirements.yaml"
    mapping: str = ".requirements-mapping.yaml"


class Config(BaseModel):
    requirements: RequirementsConfig = RequirementsConfig()


def load_config(config_path: str = ".ai-prov/config.yaml") -> Config:
    """Load configuration from YAML."""
    path = Path(config_path)

    if not path.exists():
        return Config()

    with open(path) as f:
        data = yaml.safe_load(f)

    return Config(**data)
```

**Acceptance Criteria**:
- [ ] Configuration file is loaded
- [ ] Defaults to native source if config missing
- [ ] Config is used by RequirementManager

**Estimated Effort**: 45 minutes

---

### Phase 4: CLI Integration

**Goal**: Update ai-provenance CLI to work with requirements-manager

#### Task 4.1: Update trace-matrix Command

**File**: `ai-provenance/src/ai_provenance/reporters/traceability.py`

```python
def generate_traceability_matrix(
    repo_path: str = ".",
    format: str = "md",
    config: Optional[Config] = None,
) -> str:
    """Generate traceability matrix."""

    # Load config
    if config is None:
        config = load_config()

    # Get requirements from configured source
    req_manager = RequirementManager(
        repo_path=repo_path,
        source=config.requirements.source
    )

    requirements = req_manager.list_requirements()

    # ... rest of logic
```

**Acceptance Criteria**:
- [ ] Reads requirements from configured source
- [ ] Works with both native and requirements-manager sources
- [ ] Output format unchanged

**Estimated Effort**: 30 minutes

---

#### Task 4.2: Add Export Trigger

**File**: `ai-provenance/src/ai_provenance/cli/main.py`

Add command to trigger requirements-manager export:

```python
@cli.command()
@click.option("--source", default="requirements-manager", help="Requirements source")
def sync_requirements(source: str):
    """Sync requirements from external source."""
    if source == "requirements-manager":
        import subprocess

        # Trigger requirements-manager export
        result = subprocess.run(
            ["requirements-manager", "export", "--format", "ai-prov"],
            capture_output=True,
            text=True,
        )

        if result.returncode == 0:
            click.echo("✓ Requirements synced successfully")
        else:
            click.echo(f"✗ Error: {result.stderr}", err=True)
```

**Acceptance Criteria**:
- [ ] Command triggers requirements-manager export
- [ ] Shows success/error message
- [ ] Works when requirements-manager is in PATH

**Estimated Effort**: 30 minutes

---

### Phase 5: Testing and Documentation

#### Task 5.1: Integration Tests

**File**: `requirements-manager/tests/integration_test.rs`

```rust
#[test]
fn test_export_and_import_workflow() {
    // 1. Create requirements in requirements-manager
    // 2. Export to ai-prov format
    // 3. Verify files created
    // 4. Verify mapping file created
    // 5. Verify SPEC-IDs are correct
}
```

**File**: `ai-provenance/tests/test_integration.py`

```python
def test_load_from_requirements_manager():
    """Test loading requirements from requirements.yaml."""
    # 1. Create sample requirements.yaml
    # 2. Create sample mapping file
    # 3. Load via adapter
    # 4. Verify requirements loaded correctly
    # 5. Verify SPEC-IDs match

def test_trace_matrix_with_requirements_manager():
    """Test generating trace matrix with requirements-manager source."""
    # End-to-end test
```

**Acceptance Criteria**:
- [ ] All integration tests pass
- [ ] Tests cover export → import workflow
- [ ] Tests cover trace matrix generation

**Estimated Effort**: 2 hours

---

#### Task 5.2: Update Documentation

**Files to Update**:
- `requirements-manager/README.md` - Add export section
- `requirements-manager/CLAUDE.md` - Update with export commands
- `ai-provenance/README.md` - Add integration section
- Both `INTEGRATION.md` files - Keep updated

**Acceptance Criteria**:
- [ ] Export command documented
- [ ] Integration workflow documented
- [ ] Examples provided
- [ ] Troubleshooting section added

**Estimated Effort**: 1 hour

---

#### Task 5.3: Create Example Project

**Directory**: `examples/integrated-project/`

```
examples/integrated-project/
├── requirements.yaml              # requirements-manager data
├── .requirements-mapping.yaml     # Mappings
├── .ai-prov/
│   └── config.yaml               # Points to requirements.yaml
├── src/
│   └── example.py                # Sample code with ai:tags
└── README.md                      # How to use example
```

**Acceptance Criteria**:
- [ ] Example shows full workflow
- [ ] README explains each step
- [ ] Can be run by users to test integration

**Estimated Effort**: 1 hour

---

## Summary

### Total Estimated Effort

| Phase | Tasks | Estimated Time |
|-------|-------|----------------|
| Phase 1: Export | 4 tasks | 4 hours |
| Phase 2: ID Mapping | 3 tasks | 2 hours |
| Phase 3: Adapter | 3 tasks | 3.75 hours |
| Phase 4: CLI Integration | 2 tasks | 1 hour |
| Phase 5: Testing & Docs | 3 tasks | 4 hours |
| **Total** | **15 tasks** | **~15 hours** |

### Priority Order

**High Priority** (Core functionality):
1. Task 1.1 - Export CLI command
2. Task 1.2 - Export module
3. Task 1.3 - Export handler
4. Task 2.1 - Mapping format
5. Task 2.2 - Mapping module
6. Task 3.1 - Adapter
7. Task 3.2 - Update RequirementManager

**Medium Priority** (Usability):
8. Task 3.3 - Configuration
9. Task 4.1 - Update trace-matrix
10. Task 1.4 - Export tests
11. Task 5.1 - Integration tests

**Low Priority** (Nice to have):
12. Task 2.3 - Mapping commands
13. Task 4.2 - Export trigger
14. Task 5.2 - Documentation
15. Task 5.3 - Example project

### Milestones

**Milestone 1**: Basic Export (Tasks 1.1-1.3, 2.1-2.2)
- requirements-manager can export to ai-prov format
- Mapping system works

**Milestone 2**: Import (Tasks 3.1-3.2)
- ai-provenance can read from requirements.yaml
- Adapter converts formats correctly

**Milestone 3**: Full Integration (Tasks 3.3, 4.1, 5.1)
- Configuration system works
- Trace matrix uses requirements.yaml
- Tests pass

**Milestone 4**: Polish (Tasks 1.4, 2.3, 4.2, 5.2-5.3)
- Full test coverage
- Documentation complete
- Examples available

## Next Steps

1. Review this plan with stakeholders
2. Set up project tracking (GitHub issues, etc.)
3. Begin Phase 1 implementation
4. Iterate based on feedback
