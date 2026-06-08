// trace:ARCH-docs-review | ai:claude
//! Review report generator for documentation and code quality reviews.
//!
//! Produces reports in multiple formats:
//! - Markdown with before/after diffs
//! - HTML with dark-theme side-by-side diff viewer
//! - SARIF for GitHub Code Scanning integration
//!
//! Used by both /aida-docs-review and /aida-code-review skills.

use serde::{Deserialize, Serialize};

/// Severity of a documentation issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Critical,
    Important,
    Minor,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Critical => write!(f, "CRITICAL"),
            Severity::Important => write!(f, "IMPORTANT"),
            Severity::Minor => write!(f, "MINOR"),
        }
    }
}

/// A single documentation issue found during review.
#[derive(Debug, Clone, Serialize)]
pub struct DocIssue {
    pub rule_id: String,
    pub file: String,
    pub line: Option<usize>,
    pub severity: Severity,
    pub category: String,
    pub description: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// A review rule definition from the catalog.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewRule {
    pub id: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    pub default_severity: Severity,
}

/// Catalog of all review rules.
pub const REVIEW_RULES: &[ReviewRule] = &[
    // Traceability
    ReviewRule {
        id: "TRACE-001",
        category: "Traceability",
        description: "Source file has no trace comments",
        default_severity: Severity::Important,
    },
    ReviewRule {
        id: "TRACE-002",
        category: "Traceability",
        description: "Trace references non-existent requirement",
        default_severity: Severity::Critical,
    },
    ReviewRule {
        id: "TRACE-003",
        category: "Traceability",
        description: "Requirement has no implementing code",
        default_severity: Severity::Important,
    },
    // Complexity
    ReviewRule {
        id: "COMPLEXITY-001",
        category: "Complexity",
        description: "File exceeds line limit",
        default_severity: Severity::Important,
    },
    ReviewRule {
        id: "COMPLEXITY-002",
        category: "Complexity",
        description: "Function exceeds line limit",
        default_severity: Severity::Important,
    },
    ReviewRule {
        id: "COMPLEXITY-003",
        category: "Complexity",
        description: "Excessive nesting depth",
        default_severity: Severity::Minor,
    },
    // Dead Code
    ReviewRule {
        id: "DEAD-001",
        category: "Dead Code",
        description: "TODO/FIXME/HACK comment found",
        default_severity: Severity::Minor,
    },
    ReviewRule {
        id: "DEAD-002",
        category: "Dead Code",
        description: "Unused dependency detected",
        default_severity: Severity::Important,
    },
    // Error Handling
    ReviewRule {
        id: "ERROR-001",
        category: "Error Handling",
        description: "unwrap() in non-test code",
        default_severity: Severity::Important,
    },
    ReviewRule {
        id: "ERROR-002",
        category: "Error Handling",
        description: "Error swallowed (let _ = ...)",
        default_severity: Severity::Minor,
    },
    // Security
    ReviewRule {
        id: "SECURITY-001",
        category: "Security",
        description: "Potential hardcoded secret",
        default_severity: Severity::Critical,
    },
    ReviewRule {
        id: "SECURITY-002",
        category: "Security",
        description: "Unsafe code block without justification",
        default_severity: Severity::Important,
    },
    ReviewRule {
        id: "SECURITY-003",
        category: "Security",
        description: "Known vulnerability in dependency",
        default_severity: Severity::Critical,
    },
    // Consistency
    ReviewRule {
        id: "CONSISTENCY-001",
        category: "Consistency",
        description: "Inconsistent naming convention",
        default_severity: Severity::Minor,
    },
    // Tests
    ReviewRule {
        id: "TEST-001",
        category: "Tests",
        description: "Module has no tests",
        default_severity: Severity::Important,
    },
    ReviewRule {
        id: "TEST-002",
        category: "Tests",
        description: "Test only covers happy path",
        default_severity: Severity::Minor,
    },
    // Documentation
    ReviewRule {
        id: "DOCS-001",
        category: "Documentation",
        description: "Public API missing doc comment",
        default_severity: Severity::Minor,
    },
    ReviewRule {
        id: "DOCS-002",
        category: "Documentation",
        description: "Doc comment contradicts implementation",
        default_severity: Severity::Important,
    },
    ReviewRule {
        id: "DOCS-003",
        category: "Documentation",
        description: "Stale content (referenced item no longer exists)",
        default_severity: Severity::Critical,
    },
    ReviewRule {
        id: "DOCS-004",
        category: "Documentation",
        description: "Hype/marketing language in technical docs",
        default_severity: Severity::Minor,
    },
    ReviewRule {
        id: "DOCS-005",
        category: "Documentation",
        description: "Inconsistency between documents",
        default_severity: Severity::Important,
    },
    // Dependencies
    ReviewRule {
        id: "DEPS-001",
        category: "Dependencies",
        description: "Outdated dependency",
        default_severity: Severity::Minor,
    },
    ReviewRule {
        id: "DEPS-002",
        category: "Dependencies",
        description: "Dependency with known vulnerability",
        default_severity: Severity::Critical,
    },
];

/// Complete documentation review report.
#[derive(Debug, Clone, Serialize)]
pub struct DocReviewReport {
    pub generated_at: String,
    pub files_reviewed: usize,
    pub issues: Vec<DocIssue>,
}

impl Default for DocReviewReport {
    fn default() -> Self {
        Self::new()
    }
}

impl DocReviewReport {
    pub fn new() -> Self {
        Self {
            generated_at: chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string(),
            files_reviewed: 0,
            issues: Vec::new(),
        }
    }

    pub fn add_issue(&mut self, issue: DocIssue) {
        self.issues.push(issue);
    }

    pub fn critical_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Critical)
            .count()
    }

    pub fn important_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Important)
            .count()
    }

    pub fn minor_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Minor)
            .count()
    }

    /// Generate a markdown report.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str("# Documentation Review Report\n\n");
        md.push_str(&format!("Generated: {}\n", self.generated_at));
        md.push_str(&format!("Files reviewed: {}\n", self.files_reviewed));
        md.push_str(&format!(
            "Issues found: {} ({} critical, {} important, {} minor)\n\n",
            self.issues.len(),
            self.critical_count(),
            self.important_count(),
            self.minor_count(),
        ));

        // Summary table
        let mut file_counts: std::collections::BTreeMap<String, (usize, usize, usize)> =
            std::collections::BTreeMap::new();
        for issue in &self.issues {
            let entry = file_counts.entry(issue.file.clone()).or_insert((0, 0, 0));
            match issue.severity {
                Severity::Critical => entry.0 += 1,
                Severity::Important => entry.1 += 1,
                Severity::Minor => entry.2 += 1,
            }
        }

        md.push_str("## Summary\n\n");
        md.push_str("| File | Critical | Important | Minor |\n");
        md.push_str("|------|----------|-----------|-------|\n");
        for (file, (c, i, m)) in &file_counts {
            md.push_str(&format!("| {} | {} | {} | {} |\n", file, c, i, m));
        }
        md.push('\n');

        // Issues by file
        md.push_str("## Issues\n\n");
        let mut current_file = String::new();
        for issue in &self.issues {
            if issue.file != current_file {
                current_file = issue.file.clone();
                md.push_str(&format!("### {}\n\n", current_file));
            }

            let line_info = issue
                .line
                .map(|l| format!(" (line {})", l))
                .unwrap_or_default();
            md.push_str(&format!(
                "**[{}]** **{}**: {}{}\n",
                issue.rule_id, issue.severity, issue.category, line_info
            ));
            md.push_str(&format!("- {}\n", issue.description));

            if let (Some(before), Some(after)) = (&issue.before, &issue.after) {
                md.push_str(&format!("- **Before**: {}\n", truncate(before, 100)));
                md.push_str(&format!("- **After**: {}\n", truncate(after, 100)));
            }
            md.push('\n');
        }

        md
    }

    /// Generate an HTML report with side-by-side diffs.
    pub fn to_html(&self) -> String {
        let mut html = String::new();
        html.push_str(HTML_HEADER);

        // Summary
        html.push_str(&format!(
            r#"<div class="summary">
<h1>Documentation Review Report</h1>
<p>Generated: {} | Files: {} | Issues: {}
(<span class="critical">{} critical</span>,
<span class="important">{} important</span>,
<span class="minor">{} minor</span>)</p>
</div>"#,
            self.generated_at,
            self.files_reviewed,
            self.issues.len(),
            self.critical_count(),
            self.important_count(),
            self.minor_count(),
        ));

        // Issues
        let mut current_file = String::new();
        for issue in &self.issues {
            if issue.file != current_file {
                if !current_file.is_empty() {
                    html.push_str("</div>"); // close previous file section
                }
                current_file = issue.file.clone();
                html.push_str(&format!(
                    r#"<div class="file-section">
<h2 class="file-header" onclick="this.parentElement.classList.toggle('collapsed')">{}</h2>"#,
                    escape_html(&current_file)
                ));
            }

            let severity_class = match issue.severity {
                Severity::Critical => "critical",
                Severity::Important => "important",
                Severity::Minor => "minor",
            };

            html.push_str(&format!(
                r#"<div class="issue {}">
<div class="issue-header"><span class="badge {}">{}</span> <code>{}</code> {}</div>
<p>{}</p>"#,
                severity_class,
                severity_class,
                issue.severity,
                escape_html(&issue.rule_id),
                escape_html(&issue.category),
                escape_html(&issue.description),
            ));

            if let (Some(before), Some(after)) = (&issue.before, &issue.after) {
                html.push_str(r#"<div class="diff">"#);
                html.push_str(r#"<div class="diff-before"><strong>Before:</strong><pre>"#);
                html.push_str(&escape_html(before));
                html.push_str("</pre></div>");
                html.push_str(r#"<div class="diff-after"><strong>After:</strong><pre>"#);
                html.push_str(&escape_html(after));
                html.push_str("</pre></div></div>");
            }

            html.push_str("</div>\n");
        }

        if !current_file.is_empty() {
            html.push_str("</div>"); // close last file section
        }

        html.push_str(HTML_FOOTER);
        html
    }

    /// Generate a SARIF report for GitHub Code Scanning integration.
    pub fn to_sarif(&self, tool_name: &str) -> String {
        let results: Vec<serde_json::Value> = self
            .issues
            .iter()
            .map(|issue| {
                let level = match issue.severity {
                    Severity::Critical => "error",
                    Severity::Important => "warning",
                    Severity::Minor => "note",
                };

                let rule_id = if issue.rule_id.is_empty() {
                    format!("aida/{}", issue.category.to_lowercase().replace(' ', "-"))
                } else {
                    issue.rule_id.clone()
                };

                let mut result = serde_json::json!({
                    "ruleId": rule_id,
                    "level": level,
                    "message": { "text": issue.description },
                    "locations": [{
                        "physicalLocation": {
                            "artifactLocation": { "uri": issue.file },
                            "region": { "startLine": issue.line.unwrap_or(1) }
                        }
                    }]
                });

                if let (Some(before), Some(after)) = (&issue.before, &issue.after) {
                    result["fixes"] = serde_json::json!([{
                        "description": { "text": format!("Change: {} → {}", before, after) }
                    }]);
                }

                result
            })
            .collect();

        serde_json::json!({
            "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json",
            "version": "2.1.0",
            "runs": [{
                "tool": {
                    "driver": {
                        "name": tool_name,
                        "version": env!("CARGO_PKG_VERSION"),
                        "informationUri": "https://github.com/joemooney/aida"
                    }
                },
                "results": results
            }]
        })
        .to_string()
    }

    /// Record review results in the telemetry store.
    pub fn record_in_telemetry(&self, telemetry: &mut crate::telemetry::TelemetryStore) {
        telemetry.record(
            "system",
            crate::telemetry::EventKind::ReviewCompleted {
                total_issues: self.issues.len(),
                critical: self.critical_count(),
                important: self.important_count(),
                minor: self.minor_count(),
            },
            None,
        );
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const HTML_HEADER: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>AIDA Documentation Review</title>
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; background: #0d1117; color: #c9d1d9; line-height: 1.6; padding: 2rem; }
  .summary { background: #161b22; border: 1px solid #30363d; border-radius: 8px; padding: 1.5rem; margin-bottom: 2rem; }
  h1 { color: #f0f6fc; margin-bottom: 0.5rem; }
  h2 { color: #f0f6fc; cursor: pointer; padding: 0.5rem 0; }
  h2:hover { color: #58a6ff; }
  .file-section { background: #161b22; border: 1px solid #30363d; border-radius: 8px; margin-bottom: 1rem; padding: 1rem 1.5rem; }
  .file-section.collapsed .issue { display: none; }
  .issue { border-left: 3px solid #30363d; padding: 0.75rem 1rem; margin: 0.75rem 0; }
  .issue.critical { border-left-color: #f85149; background: rgba(248,81,73,0.05); }
  .issue.important { border-left-color: #d29922; background: rgba(210,153,34,0.05); }
  .issue.minor { border-left-color: #8b949e; background: rgba(139,148,158,0.05); }
  .issue-header { font-weight: 600; margin-bottom: 0.25rem; }
  .badge { padding: 2px 8px; border-radius: 4px; font-size: 0.75rem; font-weight: 700; text-transform: uppercase; }
  .badge.critical { background: #f85149; color: #fff; }
  .badge.important { background: #d29922; color: #fff; }
  .badge.minor { background: #8b949e; color: #fff; }
  .critical { color: #f85149; }
  .important { color: #d29922; }
  .minor { color: #8b949e; }
  .diff { display: grid; grid-template-columns: 1fr 1fr; gap: 1rem; margin-top: 0.5rem; }
  .diff-before, .diff-after { background: #0d1117; border: 1px solid #30363d; border-radius: 4px; padding: 0.5rem; overflow-x: auto; }
  .diff-before { border-left: 3px solid #f85149; }
  .diff-after { border-left: 3px solid #3fb950; }
  pre { font-family: 'SF Mono', 'Fira Code', monospace; font-size: 0.85rem; white-space: pre-wrap; word-break: break-word; }
  strong { color: #f0f6fc; }
  @media (max-width: 768px) { .diff { grid-template-columns: 1fr; } }
</style>
</head>
<body>
"#;

const HTML_FOOTER: &str = r#"
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_report_generation() {
        let mut report = DocReviewReport::new();
        report.files_reviewed = 5;
        report.add_issue(DocIssue {
            rule_id: "DOCS-003".into(),
            file: "README.md".into(),
            line: Some(42),
            severity: Severity::Critical,
            category: "Accuracy".into(),
            description: "Skill count wrong".into(),
            before: Some("15 skills".into()),
            after: Some("21 skills".into()),
        });
        report.add_issue(DocIssue {
            rule_id: "DOCS-004".into(),
            file: "README.md".into(),
            line: Some(58),
            severity: Severity::Minor,
            category: "Tone".into(),
            description: "Hype language".into(),
            before: Some("blazing fast".into()),
            after: Some("sub-millisecond queries".into()),
        });

        assert_eq!(report.critical_count(), 1);
        assert_eq!(report.minor_count(), 1);

        let md = report.to_markdown();
        assert!(md.contains("Documentation Review Report"));
        assert!(md.contains("[DOCS-003]"));
        assert!(md.contains("CRITICAL"));
        assert!(md.contains("21 skills"));

        let html = report.to_html();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("badge critical"));
        assert!(html.contains("DOCS-003"));
        assert!(html.contains("21 skills"));

        let sarif = report.to_sarif("aida-review");
        assert!(sarif.contains("DOCS-003"));
        assert!(sarif.contains("DOCS-004"));
    }

    #[test]
    fn test_rule_catalog() {
        // Verify all rules have unique IDs
        let mut ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for rule in REVIEW_RULES {
            assert!(!rule.id.is_empty(), "Rule ID should not be empty");
            assert!(
                !rule.category.is_empty(),
                "Category should not be empty for rule {}",
                rule.id
            );
            assert!(
                !rule.description.is_empty(),
                "Description should not be empty for rule {}",
                rule.id
            );
            assert!(ids.insert(rule.id), "Duplicate rule ID: {}", rule.id);
        }
        // Verify expected count
        assert_eq!(REVIEW_RULES.len(), 23);
    }

    #[test]
    fn test_record_in_telemetry() {
        let mut report = DocReviewReport::new();
        report.files_reviewed = 2;
        report.add_issue(DocIssue {
            rule_id: "TRACE-001".into(),
            file: "lib.rs".into(),
            line: None,
            severity: Severity::Important,
            category: "Traceability".into(),
            description: "No trace comments".into(),
            before: None,
            after: None,
        });
        report.add_issue(DocIssue {
            rule_id: "SECURITY-001".into(),
            file: "config.rs".into(),
            line: Some(10),
            severity: Severity::Critical,
            category: "Security".into(),
            description: "Hardcoded secret".into(),
            before: None,
            after: None,
        });

        let mut telemetry = crate::telemetry::TelemetryStore::default();
        report.record_in_telemetry(&mut telemetry);

        assert_eq!(telemetry.events.len(), 1);
        match &telemetry.events[0].kind {
            crate::telemetry::EventKind::ReviewCompleted {
                total_issues,
                critical,
                important,
                minor,
            } => {
                assert_eq!(*total_issues, 2);
                assert_eq!(*critical, 1);
                assert_eq!(*important, 1);
                assert_eq!(*minor, 0);
            }
            _ => panic!("Expected ReviewCompleted event"),
        }
    }
}
