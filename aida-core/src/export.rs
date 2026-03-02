// trace:META-EXPORT | ai:claude:high
//! Export/Import functionality for requirements.
//!
//! This module provides:
//! - Mapping file generation (UUID to SPEC-ID)
//! - JSON export of full store
//! - Requirements specification export (markdown)
//! - Implementation records export (markdown)
//! - Tree export/import for portability between databases

use crate::models::{
    Comment, MetaSubtype, RelationshipType, Requirement, RequirementPriority, RequirementStatus,
    RequirementType, RequirementsStore,
};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct MappingFile {
    pub mappings: HashMap<String, String>, // UUID -> SPEC-ID
    pub next_spec_number: u32,
}

impl MappingFile {
    /// Load existing mapping file or create new
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            let content = fs::read_to_string(path)?;
            let mapping: MappingFile = serde_yaml::from_str(&content)?;
            Ok(mapping)
        } else {
            Ok(MappingFile {
                mappings: HashMap::new(),
                next_spec_number: 1,
            })
        }
    }

    /// Save mapping file to disk
    pub fn save(&self, path: &Path) -> Result<()> {
        let yaml = serde_yaml::to_string(self)?;
        fs::write(path, yaml)?;
        Ok(())
    }

    /// Get or create SPEC-ID for a UUID
    pub fn get_or_create_spec_id(&mut self, uuid: &str) -> String {
        if let Some(spec_id) = self.mappings.get(uuid) {
            spec_id.clone()
        } else {
            let spec_id = format!("SPEC-{:03}", self.next_spec_number);
            self.mappings.insert(uuid.to_string(), spec_id.clone());
            self.next_spec_number += 1;
            spec_id
        }
    }

    /// Get UUID for SPEC-ID (reverse lookup)
    pub fn get_uuid(&self, spec_id: &str) -> Option<String> {
        for (uuid, sid) in &self.mappings {
            if sid == spec_id {
                return Some(uuid.clone());
            }
        }
        None
    }
}

/// Generate mapping file (UUID -> SPEC-ID)
pub fn generate_mapping_file(store: &RequirementsStore, output_path: &Path) -> Result<()> {
    // Load existing mapping or create new
    let mut mapping = MappingFile::load_or_create(output_path)?;

    // Generate SPEC-IDs for all requirements
    for req in &store.requirements {
        let uuid = req.id.to_string();
        mapping.get_or_create_spec_id(&uuid);
    }

    // Save mapping
    mapping.save(output_path)?;

    println!("Generated mapping file: {}", output_path.display());
    println!("  Total mappings: {}", mapping.mappings.len());
    println!("  Next SPEC number: {}", mapping.next_spec_number);

    Ok(())
}

/// Export requirements to JSON format
pub fn export_json(store: &RequirementsStore, output_path: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(store)?;
    fs::write(output_path, json)?;

    println!("Exported to JSON: {}", output_path.display());
    println!("  Total requirements: {}", store.requirements.len());

    Ok(())
}

/// Export requirements specification (excludes IMPL tasks and implementation details)
pub fn export_requirements_spec(store: &RequirementsStore, output_path: &Path) -> Result<()> {
    let mut output = String::new();

    // Title
    let title = if !store.title.is_empty() {
        &store.title
    } else if !store.name.is_empty() {
        &store.name
    } else {
        "Requirements Specification"
    };
    output.push_str(&format!("# {}\n\n", title));

    if !store.description.is_empty() {
        output.push_str(&format!("{}\n\n", store.description));
    }

    // Group requirements by type, excluding IMPL
    let mut by_type: HashMap<String, Vec<&crate::models::Requirement>> = HashMap::new();

    for req in &store.requirements {
        // Skip IMPL tasks
        let spec_id = req.spec_id.as_deref().unwrap_or("");
        if spec_id.starts_with("IMPL-") {
            continue;
        }

        let type_name = format!("{:?}", req.req_type);
        by_type.entry(type_name).or_default().push(req);
    }

    // Sort types for consistent output
    let mut type_names: Vec<_> = by_type.keys().cloned().collect();
    type_names.sort();

    for type_name in type_names {
        if let Some(reqs) = by_type.get(&type_name) {
            output.push_str(&format!("## {} Requirements\n\n", type_name));

            let mut sorted_reqs = reqs.clone();
            sorted_reqs.sort_by(|a, b| a.spec_id.cmp(&b.spec_id));

            for req in sorted_reqs {
                let spec_id = req.spec_id.as_deref().unwrap_or("N/A");
                output.push_str(&format!("### {} - {}\n\n", spec_id, req.title));
                output.push_str(&format!(
                    "**Status:** {:?} | **Priority:** {:?}\n\n",
                    req.status, req.priority
                ));

                if !req.description.is_empty() {
                    output.push_str(&format!("{}\n\n", req.description));
                }

                // Show parent relationship if exists
                for rel in &req.relationships {
                    if rel.rel_type == crate::models::RelationshipType::Parent {
                        if let Some(parent) =
                            store.requirements.iter().find(|r| r.id == rel.target_id)
                        {
                            let parent_spec_id = parent.spec_id.as_deref().unwrap_or("N/A");
                            output.push_str(&format!(
                                "**Parent:** {} - {}\n\n",
                                parent_spec_id, parent.title
                            ));
                        }
                    }
                }
            }
        }
    }

    fs::write(output_path, output)?;

    let req_count = store
        .requirements
        .iter()
        .filter(|r| !r.spec_id.as_deref().unwrap_or("").starts_with("IMPL-"))
        .count();

    println!(
        "Exported requirements specification: {}",
        output_path.display()
    );
    println!("  Total requirements: {} (excluding IMPL tasks)", req_count);

    Ok(())
}

/// Export implementation records (IMPL tasks only)
pub fn export_implementation_records(store: &RequirementsStore, output_path: &Path) -> Result<()> {
    let mut output = String::new();

    // Title
    let title = if !store.title.is_empty() {
        &store.title
    } else if !store.name.is_empty() {
        &store.name
    } else {
        "Project"
    };
    output.push_str(&format!("# {} - Implementation Records\n\n", title));
    output.push_str("This document contains implementation details and design records.\n\n");

    // Get all IMPL tasks, sorted
    let mut impl_tasks: Vec<_> = store
        .requirements
        .iter()
        .filter(|r| r.spec_id.as_deref().unwrap_or("").starts_with("IMPL-"))
        .collect();

    impl_tasks.sort_by(|a, b| a.spec_id.cmp(&b.spec_id));

    for req in &impl_tasks {
        let spec_id = req.spec_id.as_deref().unwrap_or("N/A");
        output.push_str(&format!("## {} - {}\n\n", spec_id, req.title));
        output.push_str(&format!(
            "**Status:** {:?} | **Date:** {}\n\n",
            req.status,
            req.created_at.format("%Y-%m-%d")
        ));

        // Show parent requirement
        for rel in &req.relationships {
            if rel.rel_type == crate::models::RelationshipType::Parent {
                if let Some(parent) = store.requirements.iter().find(|r| r.id == rel.target_id) {
                    let parent_spec_id = parent.spec_id.as_deref().unwrap_or("N/A");
                    output.push_str(&format!(
                        "**Implements:** {} - {}\n\n",
                        parent_spec_id, parent.title
                    ));
                }
            }
        }

        if !req.description.is_empty() {
            output.push_str(&format!("{}\n\n", req.description));
        }

        // Include custom fields (implementation_summary, files_changed, etc.)
        if !req.custom_fields.is_empty() {
            for (field_name, value) in &req.custom_fields {
                if !value.is_empty() {
                    let label = match field_name.as_str() {
                        "implementation_summary" => "Implementation Summary",
                        "files_changed" => "Files Changed",
                        "session_date" => "Session Date",
                        _ => field_name,
                    };
                    output.push_str(&format!("### {}\n\n{}\n\n", label, value));
                }
            }
        }

        output.push_str("---\n\n");
    }

    fs::write(output_path, output)?;

    println!("Exported implementation records: {}", output_path.display());
    println!("  Total IMPL tasks: {}", impl_tasks.len());

    Ok(())
}

// ============================================================================
// Tree Export/Import
// ============================================================================

/// Version of the tree export format
pub const TREE_EXPORT_VERSION: &str = "1.0";

/// A complete exported requirement tree
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedTree {
    /// Export format version
    pub version: String,
    /// When the export was created
    pub exported_at: DateTime<Utc>,
    /// Source database name (if available)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_database: Option<String>,
    /// The root requirement and all its descendants
    pub root: ExportedRequirement,
}

/// An exported requirement with its children embedded
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedRequirement {
    /// Original UUID from source database
    pub original_uuid: Uuid,
    /// Original SPEC-ID from source database
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_spec_id: Option<String>,
    /// Requirement title
    pub title: String,
    /// Requirement description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Requirement type
    pub req_type: RequirementType,
    /// Meta subtype (for Meta requirements)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta_subtype: Option<MetaSubtype>,
    /// Status
    pub status: RequirementStatus,
    /// Priority
    pub priority: RequirementPriority,
    /// Owner
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
    /// Feature category
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub feature: String,
    /// Tags
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub tags: HashSet<String>,
    /// Comments
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,
    /// Custom fields
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom_fields: HashMap<String, String>,
    /// Custom status (for types with custom statuses)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_status: Option<String>,
    /// Custom priority (for types with custom priorities)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_priority: Option<String>,
    /// Archived flag
    #[serde(default)]
    pub archived: bool,
    /// Child requirements (embedded tree structure)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ExportedRequirement>,
    /// Relationships to requirements outside this tree
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_relationships: Vec<ExternalRelRef>,
}

/// Reference to a relationship target outside the exported tree
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalRelRef {
    /// Original UUID of the target
    pub original_target_uuid: Uuid,
    /// Original SPEC-ID of the target (if available)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_target_spec_id: Option<String>,
    /// Relationship type
    pub rel_type: RelationshipType,
}

/// Options for importing a tree
#[derive(Debug, Clone, Default)]
pub struct TreeImportOptions {
    /// Parent requirement to attach the imported tree under
    pub parent_id: Option<String>,
    /// How to handle conflicts (by title)
    pub conflict_strategy: ConflictStrategy,
    /// Created by field for imported requirements
    pub created_by: Option<String>,
}

/// Strategy for handling conflicts during import
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConflictStrategy {
    /// Skip if a requirement with the same title exists
    #[default]
    Skip,
    /// Rename imported requirements (add suffix)
    Rename,
    /// Replace existing requirements
    Replace,
}

/// Result of an import operation
#[derive(Debug, Clone, Default)]
pub struct TreeImportResult {
    /// Number of requirements successfully imported
    pub imported_count: usize,
    /// Number of requirements skipped (due to conflicts)
    pub skipped_count: usize,
    /// Mapping from old UUIDs to new UUIDs
    pub uuid_mapping: HashMap<Uuid, Uuid>,
    /// Mapping from old SPEC-IDs to new SPEC-IDs
    pub spec_id_mapping: HashMap<String, String>,
    /// External relationships that couldn't be resolved
    pub unresolved_refs: Vec<ExternalRelRef>,
}

/// Export a requirement and all its descendants to an ExportedTree
pub fn export_tree(store: &RequirementsStore, root_id: &str) -> Result<ExportedTree> {
    // Find the root requirement - try SPEC-ID first, then UUID
    let root = store
        .get_requirement_by_spec_id(root_id)
        .or_else(|| {
            root_id
                .parse::<Uuid>()
                .ok()
                .and_then(|uuid| store.get_requirement_by_id(&uuid))
        })
        .context("Root requirement not found")?;

    // Build set of UUIDs in this tree (for external relationship detection)
    let mut tree_uuids = HashSet::new();
    collect_descendant_uuids(store, root.id, &mut tree_uuids);

    // Export recursively
    let exported_root = export_requirement_tree(store, root, &tree_uuids)?;

    Ok(ExportedTree {
        version: TREE_EXPORT_VERSION.to_string(),
        exported_at: Utc::now(),
        source_database: if store.name.is_empty() {
            None
        } else {
            Some(store.name.clone())
        },
        root: exported_root,
    })
}

/// Collect UUIDs of a requirement and all its descendants
fn collect_descendant_uuids(store: &RequirementsStore, id: Uuid, uuids: &mut HashSet<Uuid>) {
    uuids.insert(id);
    // Get children via Child relationship
    let child_ids = store.get_relationships_by_type(&id, &RelationshipType::Child);
    for child_id in child_ids {
        collect_descendant_uuids(store, child_id, uuids);
    }
}

/// Export a single requirement (recursive)
fn export_requirement_tree(
    store: &RequirementsStore,
    req: &Requirement,
    tree_uuids: &HashSet<Uuid>,
) -> Result<ExportedRequirement> {
    // Find external relationships (target not in tree)
    let external_rels: Vec<ExternalRelRef> = req
        .relationships
        .iter()
        .filter(|rel| !tree_uuids.contains(&rel.target_id))
        .filter(|rel| {
            // Exclude Parent/Child relationships (structural)
            !matches!(
                rel.rel_type,
                RelationshipType::Parent | RelationshipType::Child
            )
        })
        .map(|rel| {
            let target_spec_id = store
                .get_requirement_by_id(&rel.target_id)
                .and_then(|r| r.spec_id.clone());
            ExternalRelRef {
                original_target_uuid: rel.target_id,
                original_target_spec_id: target_spec_id,
                rel_type: rel.rel_type.clone(),
            }
        })
        .collect();

    // Export children recursively - get child IDs via Child relationship
    let child_ids = store.get_relationships_by_type(&req.id, &RelationshipType::Child);
    let children: Vec<ExportedRequirement> = child_ids
        .iter()
        .filter_map(|child_id| store.get_requirement_by_id(child_id))
        .filter_map(|child| export_requirement_tree(store, child, tree_uuids).ok())
        .collect();

    Ok(ExportedRequirement {
        original_uuid: req.id,
        original_spec_id: req.spec_id.clone(),
        title: req.title.clone(),
        description: if req.description.is_empty() {
            None
        } else {
            Some(req.description.clone())
        },
        req_type: req.req_type.clone(),
        meta_subtype: req.meta_subtype.clone(),
        status: req.status.clone(),
        priority: req.priority.clone(),
        owner: req.owner.clone(),
        feature: req.feature.clone(),
        tags: req.tags.clone(),
        comments: req.comments.clone(),
        custom_fields: req.custom_fields.clone(),
        custom_status: req.custom_status.clone(),
        custom_priority: req.custom_priority.clone(),
        archived: req.archived,
        children,
        external_relationships: external_rels,
    })
}

/// Import a tree into a store, returning the result
pub fn import_tree(
    store: &mut RequirementsStore,
    tree: ExportedTree,
    options: TreeImportOptions,
) -> Result<TreeImportResult> {
    let mut result = TreeImportResult::default();

    // Collect all external relationship references
    collect_external_refs(&tree.root, &mut result.unresolved_refs);

    // Resolve parent_id to UUID if specified
    let parent_uuid = if let Some(ref parent_id) = options.parent_id {
        // Try SPEC-ID first, then UUID
        let uuid = store
            .get_requirement_by_spec_id(parent_id)
            .map(|r| r.id)
            .or_else(|| parent_id.parse::<Uuid>().ok());
        if uuid.is_none() {
            bail!("Parent requirement not found: {}", parent_id);
        }
        uuid
    } else {
        None
    };

    // Import recursively, starting from root
    import_requirement_recursive(store, &tree.root, parent_uuid, &options, &mut result)?;

    Ok(result)
}

/// Collect external relationship references from the tree
fn collect_external_refs(req: &ExportedRequirement, refs: &mut Vec<ExternalRelRef>) {
    refs.extend(req.external_relationships.clone());
    for child in &req.children {
        collect_external_refs(child, refs);
    }
}

/// Import a requirement and its children recursively
fn import_requirement_recursive(
    store: &mut RequirementsStore,
    exported: &ExportedRequirement,
    parent_uuid: Option<Uuid>,
    options: &TreeImportOptions,
    result: &mut TreeImportResult,
) -> Result<Option<Uuid>> {
    // Check for conflicts by title
    let existing_uuid = store
        .requirements
        .iter()
        .find(|r| r.title == exported.title)
        .map(|r| r.id);

    if let Some(existing_id) = existing_uuid {
        match options.conflict_strategy {
            ConflictStrategy::Skip => {
                result.skipped_count += 1;
                // Still process children with the existing requirement as parent
                for child in &exported.children {
                    import_requirement_recursive(store, child, Some(existing_id), options, result)?;
                }
                return Ok(None);
            }
            ConflictStrategy::Rename => {
                // Will create with modified title below
            }
            ConflictStrategy::Replace => {
                // Remove existing requirement by retaining all others
                store.requirements.retain(|r| r.id != existing_id);
            }
        }
    }

    // Create the new requirement
    let title = if existing_uuid.is_some() && options.conflict_strategy == ConflictStrategy::Rename
    {
        format!("{} (imported)", exported.title)
    } else {
        exported.title.clone()
    };

    let description = exported.description.clone().unwrap_or_default();
    let mut new_req = Requirement::new(title.clone(), description);
    new_req.req_type = exported.req_type.clone();
    new_req.meta_subtype = exported.meta_subtype.clone();
    new_req.status = exported.status.clone();
    new_req.priority = exported.priority.clone();
    new_req.owner = exported.owner.clone();
    new_req.feature = exported.feature.clone();
    new_req.tags = exported.tags.clone();
    new_req.comments = exported.comments.clone();
    new_req.custom_fields = exported.custom_fields.clone();
    new_req.custom_status = exported.custom_status.clone();
    new_req.custom_priority = exported.custom_priority.clone();
    new_req.archived = exported.archived;
    new_req.created_by = options.created_by.clone();

    // Capture the new UUID before adding
    let new_uuid = new_req.id;

    // Get the type prefix for ID generation
    let type_prefix = store.get_type_prefix(&new_req.req_type);

    // Add the requirement (this assigns a spec_id)
    store.add_requirement_with_id(new_req, None, type_prefix.as_deref());

    // Get the assigned spec_id
    let new_spec_id = store
        .get_requirement_by_id(&new_uuid)
        .and_then(|r| r.spec_id.clone())
        .unwrap_or_default();

    // Record the mapping
    result.uuid_mapping.insert(exported.original_uuid, new_uuid);
    if let Some(ref old_spec_id) = exported.original_spec_id {
        result
            .spec_id_mapping
            .insert(old_spec_id.clone(), new_spec_id.clone());
    }
    result.imported_count += 1;

    // Add parent relationship if specified
    if let Some(parent_id) = parent_uuid {
        // Use set_relationship to ensure only one parent
        store.set_relationship(&new_uuid, RelationshipType::Parent, &parent_id, true)?;
    }

    // Import children
    for child in &exported.children {
        import_requirement_recursive(store, child, Some(new_uuid), options, result)?;
    }

    Ok(Some(new_uuid))
}

/// Export a tree to a JSON file
pub fn export_tree_to_file<P: AsRef<Path>>(
    store: &RequirementsStore,
    root_id: &str,
    output_path: P,
) -> Result<()> {
    let tree = export_tree(store, root_id)?;
    let json = serde_json::to_string_pretty(&tree)?;
    fs::write(output_path, json)?;
    Ok(())
}

/// Import a tree from a JSON file
pub fn import_tree_from_file<P: AsRef<Path>>(
    store: &mut RequirementsStore,
    input_path: P,
    options: TreeImportOptions,
) -> Result<TreeImportResult> {
    let json = fs::read_to_string(input_path)?;
    let tree: ExportedTree = serde_json::from_str(&json)?;

    // Validate version compatibility
    if tree.version != TREE_EXPORT_VERSION {
        bail!(
            "Unsupported export version: {}. Expected: {}",
            tree.version,
            TREE_EXPORT_VERSION
        );
    }

    import_tree(store, tree, options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_mapping_file_new() {
        let mapping = MappingFile {
            mappings: HashMap::new(),
            next_spec_number: 1,
        };

        assert_eq!(mapping.mappings.len(), 0);
        assert_eq!(mapping.next_spec_number, 1);
    }

    #[test]
    fn test_get_or_create_spec_id_new() {
        let mut mapping = MappingFile {
            mappings: HashMap::new(),
            next_spec_number: 1,
        };

        let uuid = "f7d250bf-5b3e-4ec3-8bd5-2bee2c4b7bb9";
        let spec_id = mapping.get_or_create_spec_id(uuid);

        assert_eq!(spec_id, "SPEC-001");
        assert_eq!(mapping.next_spec_number, 2);
        assert_eq!(mapping.mappings.get(uuid), Some(&"SPEC-001".to_string()));
    }

    #[test]
    fn test_get_or_create_spec_id_existing() {
        let mut mappings = HashMap::new();
        mappings.insert(
            "f7d250bf-5b3e-4ec3-8bd5-2bee2c4b7bb9".to_string(),
            "SPEC-001".to_string(),
        );

        let mut mapping = MappingFile {
            mappings,
            next_spec_number: 2,
        };

        let uuid = "f7d250bf-5b3e-4ec3-8bd5-2bee2c4b7bb9";
        let spec_id = mapping.get_or_create_spec_id(uuid);

        assert_eq!(spec_id, "SPEC-001");
        assert_eq!(mapping.next_spec_number, 2); // Should not increment
    }

    #[test]
    fn test_get_uuid() {
        let mut mappings = HashMap::new();
        mappings.insert(
            "f7d250bf-5b3e-4ec3-8bd5-2bee2c4b7bb9".to_string(),
            "SPEC-001".to_string(),
        );

        let mapping = MappingFile {
            mappings,
            next_spec_number: 2,
        };

        let uuid = mapping.get_uuid("SPEC-001");
        assert_eq!(
            uuid,
            Some("f7d250bf-5b3e-4ec3-8bd5-2bee2c4b7bb9".to_string())
        );

        let uuid = mapping.get_uuid("SPEC-999");
        assert_eq!(uuid, None);
    }

    #[test]
    fn test_save_and_load() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("test-mapping.yaml");

        // Create and save
        let mut mapping = MappingFile {
            mappings: HashMap::new(),
            next_spec_number: 1,
        };
        mapping.get_or_create_spec_id("uuid-1");
        mapping.get_or_create_spec_id("uuid-2");
        mapping.save(&path)?;

        // Load and verify
        let loaded = MappingFile::load_or_create(&path)?;
        assert_eq!(loaded.mappings.len(), 2);
        assert_eq!(loaded.next_spec_number, 3);
        assert_eq!(loaded.mappings.get("uuid-1"), Some(&"SPEC-001".to_string()));
        assert_eq!(loaded.mappings.get("uuid-2"), Some(&"SPEC-002".to_string()));

        Ok(())
    }
}
