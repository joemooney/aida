// trace:ARCH-review-config | ai:claude
//! Review configuration loaded from `.aida/review-config.yaml`.
//!
//! Allows projects to customize review behavior:
//! - Suppress specific rules by ID
//! - Override severity levels
//! - Set complexity thresholds
//! - Exclude file patterns from review

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Review configuration loaded from .aida/review-config.yaml
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReviewConfig {
    /// Rules to suppress (by rule_id)
    #[serde(default)]
    pub suppress: Vec<String>,

    /// Severity overrides (rule_id -> severity)
    #[serde(default)]
    pub severity_overrides: HashMap<String, String>,

    /// Complexity thresholds
    #[serde(default)]
    pub complexity: ComplexityConfig,

    /// File/path patterns to exclude from review
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
}

/// Complexity threshold configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityConfig {
    /// Max lines per file before flagging
    #[serde(default = "default_max_file_lines")]
    pub max_file_lines: usize,

    /// Max lines per function before flagging
    #[serde(default = "default_max_function_lines")]
    pub max_function_lines: usize,

    /// Max nesting depth before flagging
    #[serde(default = "default_max_nesting")]
    pub max_nesting_depth: usize,
}

fn default_max_file_lines() -> usize {
    500
}

fn default_max_function_lines() -> usize {
    50
}

fn default_max_nesting() -> usize {
    4
}

impl Default for ComplexityConfig {
    fn default() -> Self {
        Self {
            max_file_lines: default_max_file_lines(),
            max_function_lines: default_max_function_lines(),
            max_nesting_depth: default_max_nesting(),
        }
    }
}

impl ReviewConfig {
    /// Load configuration from a YAML file. Returns defaults if file not found.
    pub fn load(path: &std::path::Path) -> Self {
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(path) {
            Ok(content) => match serde_yaml::from_str(&content) {
                Ok(config) => config,
                Err(e) => {
                    eprintln!(
                        "Warning: failed to parse review config {}: {}",
                        path.display(),
                        e
                    );
                    Self::default()
                }
            },
            Err(e) => {
                eprintln!(
                    "Warning: failed to read review config {}: {}",
                    path.display(),
                    e
                );
                Self::default()
            }
        }
    }

    /// Check if a rule is suppressed by its rule_id.
    pub fn is_suppressed(&self, rule_id: &str) -> bool {
        self.suppress.iter().any(|s| s == rule_id)
    }

    /// Return the effective severity for a rule, applying any override.
    /// Falls back to `default_severity` if no override is configured.
    pub fn effective_severity(&self, rule_id: &str, default_severity: &str) -> String {
        self.severity_overrides
            .get(rule_id)
            .cloned()
            .unwrap_or_else(|| default_severity.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ReviewConfig::default();
        assert!(config.suppress.is_empty());
        assert!(config.severity_overrides.is_empty());
        assert_eq!(config.complexity.max_file_lines, 500);
        assert_eq!(config.complexity.max_function_lines, 50);
        assert_eq!(config.complexity.max_nesting_depth, 4);
        assert!(config.exclude_patterns.is_empty());
    }

    #[test]
    fn test_is_suppressed() {
        let config = ReviewConfig {
            suppress: vec!["DOCS-004".into(), "DEAD-001".into()],
            ..Default::default()
        };
        assert!(config.is_suppressed("DOCS-004"));
        assert!(config.is_suppressed("DEAD-001"));
        assert!(!config.is_suppressed("TRACE-001"));
    }

    #[test]
    fn test_effective_severity_with_override() {
        let mut overrides = HashMap::new();
        overrides.insert("DOCS-001".into(), "CRITICAL".into());
        let config = ReviewConfig {
            severity_overrides: overrides,
            ..Default::default()
        };
        assert_eq!(config.effective_severity("DOCS-001", "MINOR"), "CRITICAL");
        assert_eq!(
            config.effective_severity("DOCS-002", "IMPORTANT"),
            "IMPORTANT"
        );
    }

    #[test]
    fn test_load_missing_file() {
        let config = ReviewConfig::load(std::path::Path::new("/nonexistent/review-config.yaml"));
        assert_eq!(config.complexity.max_file_lines, 500);
    }

    #[test]
    fn test_load_from_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review-config.yaml");
        std::fs::write(
            &path,
            r#"
suppress:
  - DOCS-004
  - DEAD-001

severity_overrides:
  DOCS-001: CRITICAL
  ERROR-002: IMPORTANT

complexity:
  max_file_lines: 1000
  max_function_lines: 100
  max_nesting_depth: 6

exclude_patterns:
  - "vendor/**"
  - "*.generated.rs"
"#,
        )
        .unwrap();

        let config = ReviewConfig::load(&path);
        assert_eq!(config.suppress.len(), 2);
        assert!(config.is_suppressed("DOCS-004"));
        assert_eq!(config.effective_severity("DOCS-001", "MINOR"), "CRITICAL");
        assert_eq!(config.complexity.max_file_lines, 1000);
        assert_eq!(config.complexity.max_function_lines, 100);
        assert_eq!(config.complexity.max_nesting_depth, 6);
        assert_eq!(config.exclude_patterns.len(), 2);
    }

    #[test]
    fn test_load_invalid_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review-config.yaml");
        std::fs::write(&path, "{{{{invalid yaml!!!!").unwrap();

        let config = ReviewConfig::load(&path);
        // Should fall back to defaults
        assert_eq!(config.complexity.max_file_lines, 500);
    }

    #[test]
    fn test_partial_yaml_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review-config.yaml");
        std::fs::write(
            &path,
            r#"
suppress:
  - TRACE-001
"#,
        )
        .unwrap();

        let config = ReviewConfig::load(&path);
        assert!(config.is_suppressed("TRACE-001"));
        // Other fields should have defaults
        assert_eq!(config.complexity.max_file_lines, 500);
        assert_eq!(config.complexity.max_function_lines, 50);
        assert!(config.severity_overrides.is_empty());
    }
}
