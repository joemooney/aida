// trace:ARCH-docs-review | ai:claude
//! Documentation review report generator.
//!
//! Produces HTML reports with side-by-side before/after diffs,
//! color-coded by severity, with collapsible sections per file.

use serde::Serialize;

/// Severity of a documentation issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
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
    pub file: String,
    pub line: Option<usize>,
    pub severity: Severity,
    pub category: String,
    pub description: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// Complete documentation review report.
#[derive(Debug, Clone, Serialize)]
pub struct DocReviewReport {
    pub generated_at: String,
    pub files_reviewed: usize,
    pub issues: Vec<DocIssue>,
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
        self.issues.iter().filter(|i| i.severity == Severity::Critical).count()
    }

    pub fn important_count(&self) -> usize {
        self.issues.iter().filter(|i| i.severity == Severity::Important).count()
    }

    pub fn minor_count(&self) -> usize {
        self.issues.iter().filter(|i| i.severity == Severity::Minor).count()
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
        md.push_str("\n");

        // Issues by file
        md.push_str("## Issues\n\n");
        let mut current_file = String::new();
        for issue in &self.issues {
            if issue.file != current_file {
                current_file = issue.file.clone();
                md.push_str(&format!("### {}\n\n", current_file));
            }

            let line_info = issue.line.map(|l| format!(" (line {})", l)).unwrap_or_default();
            md.push_str(&format!(
                "**{}**: {}{}\n",
                issue.severity, issue.category, line_info
            ));
            md.push_str(&format!("- {}\n", issue.description));

            if let (Some(before), Some(after)) = (&issue.before, &issue.after) {
                md.push_str(&format!("- **Before**: {}\n", truncate(before, 100)));
                md.push_str(&format!("- **After**: {}\n", truncate(after, 100)));
            }
            md.push_str("\n");
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
<div class="issue-header"><span class="badge {}">{}</span> {}</div>
<p>{}</p>"#,
                severity_class,
                severity_class,
                issue.severity,
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
            file: "README.md".into(),
            line: Some(42),
            severity: Severity::Critical,
            category: "Accuracy".into(),
            description: "Skill count wrong".into(),
            before: Some("15 skills".into()),
            after: Some("21 skills".into()),
        });
        report.add_issue(DocIssue {
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
        assert!(md.contains("CRITICAL"));
        assert!(md.contains("21 skills"));

        let html = report.to_html();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("badge critical"));
        assert!(html.contains("21 skills"));
    }
}
