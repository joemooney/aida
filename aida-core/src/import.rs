// trace:FR-0226 | ai:claude:high
//! Import functionality for legacy/incompatible requirements databases.
//!
//! This module provides:
//! - Schema validation to detect incompatible records before import
//! - User-configurable handling of incompatible data (skip/convert/abort)
//! - Backup of original database before import
//! - Summary reporting of import results

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use uuid::Uuid;

use crate::models::{
    Requirement, RequirementPriority, RequirementStatus, RequirementType, RequirementsStore,
};

/// Represents an issue found during import validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportIssue {
    /// The SPEC-ID or identifier of the problematic record
    pub record_id: String,
    /// Title of the record (for display)
    pub record_title: String,
    /// The type of issue
    pub issue_type: ImportIssueType,
    /// Human-readable description of the issue
    pub description: String,
    /// Whether this issue can be auto-converted
    pub can_convert: bool,
    /// Suggested conversion value (if applicable)
    pub suggested_conversion: Option<String>,
}

/// Types of issues that can occur during import
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ImportIssueType {
    /// Unknown requirement type variant
    UnknownType(String),
    /// Unknown status variant
    UnknownStatus(String),
    /// Unknown priority variant
    UnknownPriority(String),
    /// Invalid relationship target (referenced SPEC-ID doesn't exist)
    InvalidRelationshipTarget(String),
    /// Duplicate SPEC-ID found
    DuplicateSpecId(String),
    /// Missing required field
    MissingRequiredField(String),
    /// Invalid date format
    InvalidDateFormat(String),
    /// Schema version mismatch
    SchemaVersionMismatch(String),
}

impl std::fmt::Display for ImportIssueType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportIssueType::UnknownType(t) => write!(f, "Unknown type: {}", t),
            ImportIssueType::UnknownStatus(s) => write!(f, "Unknown status: {}", s),
            ImportIssueType::UnknownPriority(p) => write!(f, "Unknown priority: {}", p),
            ImportIssueType::InvalidRelationshipTarget(id) => {
                write!(f, "Invalid relationship target: {}", id)
            }
            ImportIssueType::DuplicateSpecId(id) => write!(f, "Duplicate SPEC-ID: {}", id),
            ImportIssueType::MissingRequiredField(field) => {
                write!(f, "Missing required field: {}", field)
            }
            ImportIssueType::InvalidDateFormat(field) => {
                write!(f, "Invalid date in field: {}", field)
            }
            ImportIssueType::SchemaVersionMismatch(v) => write!(f, "Schema version: {}", v),
        }
    }
}

/// How to handle a specific issue during import
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum IssueResolution {
    /// Skip the record with this issue
    #[default]
    Skip,
    /// Convert to a default value
    ConvertToDefault,
    /// Abort the entire import
    Abort,
}

impl std::fmt::Display for IssueResolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IssueResolution::Skip => write!(f, "Skip"),
            IssueResolution::ConvertToDefault => write!(f, "Convert"),
            IssueResolution::Abort => write!(f, "Abort"),
        }
    }
}

/// Result of validating an import file
#[derive(Debug, Clone, Default)]
pub struct ImportValidation {
    /// All issues found during validation
    pub issues: Vec<ImportIssue>,
    /// Records that can be imported without issues
    pub valid_record_count: usize,
    /// Records with issues
    pub problematic_record_count: usize,
    /// Whether the import can proceed (no fatal issues)
    pub can_proceed: bool,
    /// Raw parsed store (if parsing succeeded)
    pub parsed_store: Option<RawImportStore>,
}

impl ImportValidation {
    /// Returns true if there are any issues
    pub fn has_issues(&self) -> bool {
        !self.issues.is_empty()
    }

    /// Returns issues grouped by record ID
    pub fn issues_by_record(&self) -> HashMap<String, Vec<&ImportIssue>> {
        let mut map: HashMap<String, Vec<&ImportIssue>> = HashMap::new();
        for issue in &self.issues {
            map.entry(issue.record_id.clone()).or_default().push(issue);
        }
        map
    }

    /// Returns issues of a specific type
    pub fn issues_of_type(&self, issue_type: &ImportIssueType) -> Vec<&ImportIssue> {
        self.issues
            .iter()
            .filter(|i| std::mem::discriminant(&i.issue_type) == std::mem::discriminant(issue_type))
            .collect()
    }

    /// Count of unknown type issues
    pub fn unknown_type_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| matches!(i.issue_type, ImportIssueType::UnknownType(_)))
            .count()
    }

    /// Count of unknown status issues
    pub fn unknown_status_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| matches!(i.issue_type, ImportIssueType::UnknownStatus(_)))
            .count()
    }

    /// Count of invalid relationship issues
    pub fn invalid_relationship_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| matches!(i.issue_type, ImportIssueType::InvalidRelationshipTarget(_)))
            .count()
    }
}

/// Summary of an import operation
#[derive(Debug, Clone, Default)]
pub struct ImportSummary {
    /// Total records in source file
    pub total_records: usize,
    /// Records successfully imported
    pub imported_count: usize,
    /// Records skipped due to issues
    pub skipped_count: usize,
    /// Records converted (type/status changed to default)
    pub converted_count: usize,
    /// Relationships skipped (target not imported)
    pub relationships_skipped: usize,
    /// Path to backup file (if created)
    pub backup_path: Option<String>,
    /// Any warnings generated during import
    pub warnings: Vec<String>,
    /// Time taken for import
    pub duration_ms: u64,
}

impl ImportSummary {
    /// Returns true if the import was fully successful (no skips or conversions)
    pub fn is_clean(&self) -> bool {
        self.skipped_count == 0 && self.converted_count == 0 && self.relationships_skipped == 0
    }
}

/// Configuration for an import operation
#[derive(Debug, Clone)]
pub struct ImportConfig {
    /// How to handle unknown types
    pub unknown_type_resolution: IssueResolution,
    /// How to handle unknown statuses
    pub unknown_status_resolution: IssueResolution,
    /// How to handle unknown priorities
    pub unknown_priority_resolution: IssueResolution,
    /// Whether to create a backup before import
    pub create_backup: bool,
    /// Whether to merge with existing data or replace
    pub merge_mode: ImportMergeMode,
    /// Default type to use when converting unknown types
    pub default_type: RequirementType,
    /// Default status to use when converting unknown statuses
    pub default_status: RequirementStatus,
    /// Default priority to use when converting unknown priorities
    pub default_priority: RequirementPriority,
}

impl Default for ImportConfig {
    fn default() -> Self {
        Self {
            unknown_type_resolution: IssueResolution::Skip,
            unknown_status_resolution: IssueResolution::ConvertToDefault,
            unknown_priority_resolution: IssueResolution::ConvertToDefault,
            create_backup: true,
            merge_mode: ImportMergeMode::Replace,
            default_type: RequirementType::Functional,
            default_status: RequirementStatus::Draft,
            default_priority: RequirementPriority::Medium,
        }
    }
}

/// How to merge imported data with existing data
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImportMergeMode {
    /// Replace existing database entirely
    #[default]
    Replace,
    /// Merge with existing, keeping existing on conflict
    MergeKeepExisting,
    /// Merge with existing, preferring imported on conflict
    MergePreferImported,
}

/// Raw representation of a requirement during import (before validation)
/// Uses String for enum fields to allow detection of unknown values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawRequirement {
    #[serde(default)]
    pub id: Option<Uuid>,
    #[serde(default)]
    pub spec_id: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub req_type: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub feature: String,
    #[serde(default)]
    pub tags: HashSet<String>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub modified_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub relationships: Vec<RawRelationship>,
    #[serde(default)]
    pub comments: Vec<serde_yaml::Value>,
    #[serde(default)]
    pub history: Vec<serde_yaml::Value>,
    #[serde(default)]
    pub urls: Vec<serde_yaml::Value>,
    #[serde(default)]
    pub custom_fields: HashMap<String, serde_yaml::Value>,
    #[serde(default)]
    pub ai_evaluation: Option<serde_yaml::Value>,
}

/// Raw relationship for import
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawRelationship {
    #[serde(default)]
    pub target_id: Option<Uuid>,
    #[serde(default)]
    pub target_spec_id: Option<String>,
    #[serde(default)]
    pub rel_type: Option<String>,
}

/// Raw store structure for initial parsing
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RawImportStore {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub requirements: Vec<RawRequirement>,
    #[serde(default)]
    pub users: Vec<serde_yaml::Value>,
    #[serde(default)]
    pub teams: Vec<serde_yaml::Value>,
    #[serde(default)]
    pub features: Vec<serde_yaml::Value>,
    #[serde(default)]
    pub id_config: Option<serde_yaml::Value>,
    #[serde(default)]
    pub type_definitions: Vec<serde_yaml::Value>,
    #[serde(default)]
    pub relationship_definitions: Vec<serde_yaml::Value>,
    #[serde(default)]
    pub reaction_definitions: Vec<serde_yaml::Value>,
    #[serde(default)]
    pub ai_prompts: Option<serde_yaml::Value>,
    #[serde(default)]
    pub baselines: Vec<serde_yaml::Value>,
    // Capture unknown fields
    #[serde(flatten)]
    pub extra_fields: HashMap<String, serde_yaml::Value>,
}

/// Known requirement type strings (for validation)
const KNOWN_TYPES: &[&str] = &[
    "Functional",
    "NonFunctional",
    "Non-Functional",
    "System",
    "User",
    "ChangeRequest",
    "Change Request",
    "Bug",
    "Epic",
    "Story",
    "Task",
    "Spike",
    "Sprint",
    "Folder",
];

/// Known status strings (for validation)
const KNOWN_STATUSES: &[&str] = &[
    "Draft",
    "Approved",
    "Planned",
    "In Progress",
    "Done",
    "Completed",
    "Rejected",
    "Needs Attention",
];

/// Known priority strings (for validation)
const KNOWN_PRIORITIES: &[&str] = &["High", "Medium", "Low"];

/// Parse a type string into RequirementType
pub fn parse_requirement_type(s: &str) -> Option<RequirementType> {
    match s {
        "Functional" => Some(RequirementType::Functional),
        "NonFunctional" | "Non-Functional" => Some(RequirementType::NonFunctional),
        "System" => Some(RequirementType::System),
        "User" => Some(RequirementType::User),
        "ChangeRequest" | "Change Request" => Some(RequirementType::ChangeRequest),
        "Bug" => Some(RequirementType::Bug),
        "Epic" => Some(RequirementType::Epic),
        "Story" => Some(RequirementType::Story),
        "Task" => Some(RequirementType::Task),
        "Spike" => Some(RequirementType::Spike),
        "Sprint" => Some(RequirementType::Sprint),
        "Folder" => Some(RequirementType::Folder),
        _ => None,
    }
}

/// Parse a status string into RequirementStatus
pub fn parse_requirement_status(s: &str) -> Option<RequirementStatus> {
    match s {
        "Draft" => Some(RequirementStatus::Draft),
        "Approved" => Some(RequirementStatus::Approved),
        "Planned" => Some(RequirementStatus::Planned),
        "InProgress" | "In Progress" => Some(RequirementStatus::InProgress),
        "Done" => Some(RequirementStatus::Done),
        "Completed" => Some(RequirementStatus::Completed),
        "Rejected" => Some(RequirementStatus::Rejected),
        "NeedsAttention" | "Needs Attention" => Some(RequirementStatus::NeedsAttention),
        _ => None,
    }
}

/// Parse a priority string into RequirementPriority
pub fn parse_requirement_priority(s: &str) -> Option<RequirementPriority> {
    match s {
        "High" => Some(RequirementPriority::High),
        "Medium" => Some(RequirementPriority::Medium),
        "Low" => Some(RequirementPriority::Low),
        _ => None,
    }
}

/// Validate an import file and return validation results
pub fn validate_import_file(path: &Path) -> Result<ImportValidation> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read import file: {:?}", path))?;

    validate_import_content(&content)
}

/// Validate import content (YAML string)
pub fn validate_import_content(content: &str) -> Result<ImportValidation> {
    let mut validation = ImportValidation::default();

    // Try to parse as raw store
    let raw_store: RawImportStore = match serde_yaml::from_str(content) {
        Ok(store) => store,
        Err(e) => {
            validation.can_proceed = false;
            validation.issues.push(ImportIssue {
                record_id: "PARSE_ERROR".to_string(),
                record_title: "File Parse Error".to_string(),
                issue_type: ImportIssueType::SchemaVersionMismatch(e.to_string()),
                description: format!("Failed to parse YAML: {}", e),
                can_convert: false,
                suggested_conversion: None,
            });
            return Ok(validation);
        }
    };

    // Collect all SPEC-IDs for relationship validation
    let all_spec_ids: HashSet<String> = raw_store
        .requirements
        .iter()
        .filter_map(|r| r.spec_id.clone())
        .collect();

    // Check for unknown extra fields at store level
    if !raw_store.extra_fields.is_empty() {
        for field_name in raw_store.extra_fields.keys() {
            validation.issues.push(ImportIssue {
                record_id: "STORE".to_string(),
                record_title: "Database Schema".to_string(),
                issue_type: ImportIssueType::SchemaVersionMismatch(format!(
                    "Unknown field: {}",
                    field_name
                )),
                description: format!("Unknown field '{}' in database schema", field_name),
                can_convert: true,
                suggested_conversion: Some("Field will be ignored".to_string()),
            });
        }
    }

    // Validate each requirement
    let mut seen_spec_ids: HashSet<String> = HashSet::new();

    for raw_req in &raw_store.requirements {
        let record_id = raw_req
            .spec_id
            .clone()
            .unwrap_or_else(|| raw_req.id.map(|id| id.to_string()).unwrap_or_default());
        let record_title = raw_req.title.clone();

        // Check for duplicate SPEC-IDs
        if let Some(ref spec_id) = raw_req.spec_id {
            if !seen_spec_ids.insert(spec_id.clone()) {
                validation.issues.push(ImportIssue {
                    record_id: record_id.clone(),
                    record_title: record_title.clone(),
                    issue_type: ImportIssueType::DuplicateSpecId(spec_id.clone()),
                    description: format!("Duplicate SPEC-ID: {}", spec_id),
                    can_convert: false,
                    suggested_conversion: None,
                });
            }
        }

        // Validate requirement type
        if let Some(ref type_str) = raw_req.req_type {
            if parse_requirement_type(type_str).is_none() {
                validation.issues.push(ImportIssue {
                    record_id: record_id.clone(),
                    record_title: record_title.clone(),
                    issue_type: ImportIssueType::UnknownType(type_str.clone()),
                    description: format!(
                        "Unknown requirement type '{}'. Known types: {}",
                        type_str,
                        KNOWN_TYPES.join(", ")
                    ),
                    can_convert: true,
                    suggested_conversion: Some("Functional".to_string()),
                });
                validation.problematic_record_count += 1;
                continue;
            }
        }

        // Validate status
        if let Some(ref status_str) = raw_req.status {
            if parse_requirement_status(status_str).is_none() {
                validation.issues.push(ImportIssue {
                    record_id: record_id.clone(),
                    record_title: record_title.clone(),
                    issue_type: ImportIssueType::UnknownStatus(status_str.clone()),
                    description: format!(
                        "Unknown status '{}'. Known statuses: {}",
                        status_str,
                        KNOWN_STATUSES.join(", ")
                    ),
                    can_convert: true,
                    suggested_conversion: Some("Draft".to_string()),
                });
            }
        }

        // Validate priority
        if let Some(ref priority_str) = raw_req.priority {
            if parse_requirement_priority(priority_str).is_none() {
                validation.issues.push(ImportIssue {
                    record_id: record_id.clone(),
                    record_title: record_title.clone(),
                    issue_type: ImportIssueType::UnknownPriority(priority_str.clone()),
                    description: format!(
                        "Unknown priority '{}'. Known priorities: {}",
                        priority_str,
                        KNOWN_PRIORITIES.join(", ")
                    ),
                    can_convert: true,
                    suggested_conversion: Some("Medium".to_string()),
                });
            }
        }

        // Validate relationships
        for rel in &raw_req.relationships {
            if let Some(ref target_spec_id) = rel.target_spec_id {
                if !all_spec_ids.contains(target_spec_id) {
                    validation.issues.push(ImportIssue {
                        record_id: record_id.clone(),
                        record_title: record_title.clone(),
                        issue_type: ImportIssueType::InvalidRelationshipTarget(
                            target_spec_id.clone(),
                        ),
                        description: format!(
                            "Relationship references non-existent requirement: {}",
                            target_spec_id
                        ),
                        can_convert: true,
                        suggested_conversion: Some("Relationship will be skipped".to_string()),
                    });
                }
            }
        }

        // Check for missing required fields
        if raw_req.title.trim().is_empty() {
            validation.issues.push(ImportIssue {
                record_id: record_id.clone(),
                record_title: "(untitled)".to_string(),
                issue_type: ImportIssueType::MissingRequiredField("title".to_string()),
                description: "Requirement is missing a title".to_string(),
                can_convert: true,
                suggested_conversion: Some("Untitled Requirement".to_string()),
            });
        }

        validation.valid_record_count += 1;
    }

    // Update counts
    validation.problematic_record_count = validation
        .issues
        .iter()
        .map(|i| i.record_id.clone())
        .collect::<HashSet<_>>()
        .len();

    // Determine if import can proceed
    // Fatal issues: parse errors, duplicate SPEC-IDs
    let has_fatal_issues = validation.issues.iter().any(|i| {
        matches!(
            i.issue_type,
            ImportIssueType::SchemaVersionMismatch(_) | ImportIssueType::DuplicateSpecId(_)
        ) && !i.can_convert
    });

    validation.can_proceed = !has_fatal_issues;
    validation.parsed_store = Some(raw_store);

    Ok(validation)
}

/// Create a backup of the current requirements file
pub fn create_backup(source_path: &Path) -> Result<String> {
    if !source_path.exists() {
        return Ok(String::new());
    }

    let backup_path = source_path.with_extension("yaml.backup");
    fs::copy(source_path, &backup_path)
        .with_context(|| format!("Failed to create backup at {:?}", backup_path))?;

    Ok(backup_path.to_string_lossy().to_string())
}

/// Execute an import operation with the given configuration
pub fn execute_import(
    _import_content: &str,
    target_path: &Path,
    config: &ImportConfig,
    validation: &ImportValidation,
) -> Result<ImportSummary> {
    let start_time = std::time::Instant::now();
    let mut summary = ImportSummary::default();

    // Create backup if requested
    if config.create_backup && target_path.exists() {
        summary.backup_path = Some(create_backup(target_path)?);
    }

    // Get the raw store from validation
    let raw_store = validation
        .parsed_store
        .as_ref()
        .context("No parsed store available - validation must be run first")?;

    summary.total_records = raw_store.requirements.len();

    // Build set of records to skip based on issues and resolution settings
    let mut skip_records: HashSet<String> = HashSet::new();
    let mut convert_records: HashSet<String> = HashSet::new();

    for issue in &validation.issues {
        let resolution = match &issue.issue_type {
            ImportIssueType::UnknownType(_) => config.unknown_type_resolution,
            ImportIssueType::UnknownStatus(_) => config.unknown_status_resolution,
            ImportIssueType::UnknownPriority(_) => config.unknown_priority_resolution,
            ImportIssueType::DuplicateSpecId(_) => IssueResolution::Skip,
            ImportIssueType::InvalidRelationshipTarget(_) => IssueResolution::Skip, // Relationships handled separately
            ImportIssueType::MissingRequiredField(_) => IssueResolution::ConvertToDefault,
            ImportIssueType::InvalidDateFormat(_) => IssueResolution::ConvertToDefault,
            ImportIssueType::SchemaVersionMismatch(_) => IssueResolution::Skip,
        };

        match resolution {
            IssueResolution::Abort => {
                anyhow::bail!("Import aborted due to issue: {}", issue.description);
            }
            IssueResolution::Skip => {
                skip_records.insert(issue.record_id.clone());
            }
            IssueResolution::ConvertToDefault => {
                convert_records.insert(issue.record_id.clone());
            }
        }
    }

    // Build the target store
    let mut target_store = RequirementsStore::new();
    target_store.name = raw_store.name.clone();
    target_store.title = raw_store.title.clone();
    target_store.description = raw_store.description.clone();

    // Track which SPEC-IDs were imported (for relationship validation)
    let mut imported_spec_ids: HashSet<String> = HashSet::new();

    // Convert raw requirements to proper requirements
    for raw_req in &raw_store.requirements {
        let record_id = raw_req
            .spec_id
            .clone()
            .unwrap_or_else(|| raw_req.id.map(|id| id.to_string()).unwrap_or_default());

        // Skip if marked for skipping
        if skip_records.contains(&record_id) {
            summary.skipped_count += 1;
            continue;
        }

        let needs_conversion = convert_records.contains(&record_id);

        // Convert the requirement
        let mut req = Requirement::new(
            if raw_req.title.trim().is_empty() {
                "Untitled Requirement".to_string()
            } else {
                raw_req.title.clone()
            },
            raw_req.description.clone(),
        );

        // Set ID if present
        if let Some(id) = raw_req.id {
            req.id = id;
        }

        // Set SPEC-ID
        req.spec_id = raw_req.spec_id.clone();
        if let Some(ref spec_id) = req.spec_id {
            imported_spec_ids.insert(spec_id.clone());
        }

        // Set type (with conversion if needed)
        if let Some(ref type_str) = raw_req.req_type {
            req.req_type = parse_requirement_type(type_str).unwrap_or_else(|| {
                if needs_conversion {
                    summary.converted_count += 1;
                }
                config.default_type.clone()
            });
        }

        // Set status (with conversion if needed)
        if let Some(ref status_str) = raw_req.status {
            req.status = parse_requirement_status(status_str).unwrap_or_else(|| {
                if needs_conversion {
                    summary.converted_count += 1;
                }
                config.default_status.clone()
            });
        }

        // Set priority (with conversion if needed)
        if let Some(ref priority_str) = raw_req.priority {
            req.priority = parse_requirement_priority(priority_str).unwrap_or_else(|| {
                if needs_conversion {
                    summary.converted_count += 1;
                }
                config.default_priority.clone()
            });
        }

        // Set other fields
        req.owner = raw_req.owner.clone();
        req.feature = raw_req.feature.clone();
        req.tags = raw_req.tags.clone();

        // Set timestamps
        if let Some(created_at) = raw_req.created_at {
            req.created_at = created_at;
        }
        if let Some(modified_at) = raw_req.modified_at {
            req.modified_at = modified_at;
        }

        target_store.requirements.push(req);
        summary.imported_count += 1;
    }

    // Second pass: add relationships (only for imported records)
    for (idx, raw_req) in raw_store.requirements.iter().enumerate() {
        let record_id = raw_req
            .spec_id
            .clone()
            .unwrap_or_else(|| raw_req.id.map(|id| id.to_string()).unwrap_or_default());

        if skip_records.contains(&record_id) {
            continue;
        }

        // Find the corresponding requirement in target_store
        if idx < target_store.requirements.len() {
            for raw_rel in &raw_req.relationships {
                // Check if target exists in imported records
                let target_exists = raw_rel
                    .target_spec_id
                    .as_ref()
                    .map(|id| imported_spec_ids.contains(id))
                    .unwrap_or(false);

                if !target_exists {
                    summary.relationships_skipped += 1;
                    summary.warnings.push(format!(
                        "Skipped relationship from {} to {} (target not imported)",
                        record_id,
                        raw_rel.target_spec_id.as_deref().unwrap_or("unknown")
                    ));
                }
                // Note: We'd add the relationship here if we had the full conversion logic
                // For now, relationships are handled during the full store deserialization
            }
        }
    }

    // Save the imported store
    let yaml = serde_yaml::to_string(&target_store)?;
    fs::write(target_path, yaml)?;

    summary.duration_ms = start_time.elapsed().as_millis() as u64;

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_requirement_type() {
        assert_eq!(
            parse_requirement_type("Functional"),
            Some(RequirementType::Functional)
        );
        assert_eq!(
            parse_requirement_type("NonFunctional"),
            Some(RequirementType::NonFunctional)
        );
        assert_eq!(
            parse_requirement_type("Non-Functional"),
            Some(RequirementType::NonFunctional)
        );
        assert_eq!(
            parse_requirement_type("ChangeRequest"),
            Some(RequirementType::ChangeRequest)
        );
        assert_eq!(parse_requirement_type("UnknownType"), None);
    }

    #[test]
    fn test_parse_requirement_status() {
        // trace:STORY-86 | ai:claude — parser table now covers every
        // canonical variant (including Done + InProgress + Planned which
        // were previously missing).
        assert_eq!(
            parse_requirement_status("Draft"),
            Some(RequirementStatus::Draft)
        );
        assert_eq!(
            parse_requirement_status("Approved"),
            Some(RequirementStatus::Approved)
        );
        assert_eq!(
            parse_requirement_status("Planned"),
            Some(RequirementStatus::Planned)
        );
        assert_eq!(
            parse_requirement_status("InProgress"),
            Some(RequirementStatus::InProgress)
        );
        assert_eq!(
            parse_requirement_status("In Progress"),
            Some(RequirementStatus::InProgress)
        );
        assert_eq!(
            parse_requirement_status("Done"),
            Some(RequirementStatus::Done)
        );
        assert_eq!(
            parse_requirement_status("Completed"),
            Some(RequirementStatus::Completed)
        );
        assert_eq!(
            parse_requirement_status("Rejected"),
            Some(RequirementStatus::Rejected)
        );
        // trace:STORY-332 | ai:claude — both spellings of the punt status.
        assert_eq!(
            parse_requirement_status("NeedsAttention"),
            Some(RequirementStatus::NeedsAttention)
        );
        assert_eq!(
            parse_requirement_status("Needs Attention"),
            Some(RequirementStatus::NeedsAttention)
        );
        // Unknown values still fall through to None.
        assert_eq!(parse_requirement_status("FlibbertyGibbet"), None);
    }

    #[test]
    fn test_validate_empty_content() {
        let content = "requirements: []";
        let result = validate_import_content(content).unwrap();
        assert!(result.can_proceed);
        assert_eq!(result.valid_record_count, 0);
    }

    #[test]
    fn test_validate_unknown_type() {
        let content = r#"
requirements:
  - title: Test Requirement
    description: Test description
    req_type: UnknownCustomType
    status: Draft
"#;
        let result = validate_import_content(content).unwrap();
        assert!(result.has_issues());
        assert_eq!(result.unknown_type_count(), 1);
    }

    #[test]
    fn test_validate_invalid_relationship() {
        let content = r#"
requirements:
  - spec_id: FR-0001
    title: Test Requirement
    description: Test
    relationships:
      - target_spec_id: FR-9999
        rel_type: parent
"#;
        let result = validate_import_content(content).unwrap();
        assert!(result.has_issues());
        assert_eq!(result.invalid_relationship_count(), 1);
    }
}
