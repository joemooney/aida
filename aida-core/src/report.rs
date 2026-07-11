// trace:FR-0259 | ai:claude:high
//! AI Integration Report Generation
//!
//! Generates comprehensive reports documenting AI integration within a project:
//! - Project overview and configuration
//! - AI prompts and customizations
//! - Code traceability summary
//! - Scaffolding status and drift detection

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::models::{RequirementsStore, TraceLink};
use crate::scaffolding::{ScaffoldConfig, ScaffoldPreview, Scaffolder};

/// Report output format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    Markdown,
    Html,
}

/// Status of a scaffolded file compared to disk
#[derive(Debug, Clone)]
pub enum FileStatus {
    /// File matches expected content
    Match,
    /// File exists but has been modified
    Modified {
        expected_lines: usize,
        actual_lines: usize,
    },
    /// Expected file is missing from disk
    Missing,
    /// File exists on disk but not in scaffold (extra file)
    Extra,
}

/// Result of comparing scaffold to actual project
#[derive(Debug, Clone)]
pub struct ScaffoldStatus {
    /// Files that match exactly
    pub matching: Vec<PathBuf>,
    /// Files that have been modified
    pub modified: Vec<(PathBuf, FileStatus)>,
    /// Files that are missing
    pub missing: Vec<PathBuf>,
    /// Extra files in .claude directories not from scaffold
    pub extra: Vec<PathBuf>,
    /// Whether scaffold is up-to-date
    pub is_current: bool,
}

impl ScaffoldStatus {
    /// Create a new empty scaffold status
    pub fn new() -> Self {
        Self {
            matching: Vec::new(),
            modified: Vec::new(),
            missing: Vec::new(),
            extra: Vec::new(),
            is_current: true,
        }
    }
}

impl Default for ScaffoldStatus {
    fn default() -> Self {
        Self::new()
    }
}

/// Traceability statistics
#[derive(Debug, Clone, Default)]
pub struct TraceabilityStats {
    /// Total number of trace links
    pub total_links: usize,
    /// Links by artifact type
    pub by_type: HashMap<String, usize>,
    /// Links by confidence level
    pub by_confidence: HashMap<String, usize>,
    /// Requirements with trace links
    pub requirements_with_links: usize,
    /// Requirements without trace links
    pub requirements_without_links: usize,
    /// Unique files referenced
    pub unique_files: usize,
}

/// AI Integration Report data
#[derive(Debug, Clone)]
pub struct AiIntegrationReport {
    /// Project name
    pub project_name: String,
    /// Project description
    pub project_description: String,
    /// Database path
    pub database_path: String,
    /// Generation timestamp
    pub generated_at: chrono::DateTime<chrono::Utc>,
    /// Total requirements count
    pub total_requirements: usize,
    /// AI prompts configuration
    pub ai_prompts: AiPromptsSection,
    /// Traceability statistics
    pub traceability: TraceabilityStats,
    /// Trace links grouped by requirement
    pub trace_links_by_req: Vec<(String, String, Vec<TraceLink>)>, // (spec_id, title, links)
    /// Scaffolding status
    pub scaffold_status: Option<ScaffoldStatus>,
    /// Scaffolding configuration
    pub scaffold_config: Option<ScaffoldConfig>,
    /// Type definitions summary
    pub type_definitions: Vec<(String, String)>, // (name, description)
    /// Features summary
    pub features: Vec<(String, String)>, // (name, prefix)
}

/// AI Prompts configuration section
#[derive(Debug, Clone, Default)]
pub struct AiPromptsSection {
    /// Global context
    pub global_context: Option<String>,
    /// Evaluation prompt customization
    pub evaluation: Option<PromptCustomization>,
    /// Duplicates prompt customization
    pub duplicates: Option<PromptCustomization>,
    /// Relationships prompt customization
    pub relationships: Option<PromptCustomization>,
    /// Improve prompt customization
    pub improve: Option<PromptCustomization>,
    /// Generate children prompt customization
    pub generate_children: Option<PromptCustomization>,
    /// Type-specific customizations
    pub type_prompts: Vec<TypePromptCustomization>,
}

/// Individual prompt customization
#[derive(Debug, Clone)]
pub struct PromptCustomization {
    pub action_name: String,
    pub custom_template: Option<String>,
    pub additional_instructions: Option<String>,
}

/// Type-specific prompt customization
#[derive(Debug, Clone)]
pub struct TypePromptCustomization {
    pub type_name: String,
    pub evaluation_extra: Option<String>,
    pub improve_extra: Option<String>,
    pub generate_children_extra: Option<String>,
}

/// Report generator
pub struct ReportGenerator {
    store: RequirementsStore,
    project_root: Option<PathBuf>,
    database_path: String,
}

impl ReportGenerator {
    /// Create a new report generator
    pub fn new(store: RequirementsStore, database_path: String) -> Self {
        Self {
            store,
            project_root: None,
            database_path,
        }
    }

    /// Set project root for scaffolding status check
    pub fn with_project_root(mut self, root: PathBuf) -> Self {
        self.project_root = Some(root);
        self
    }

    /// Generate the report data
    pub fn generate(&self) -> AiIntegrationReport {
        let now = chrono::Utc::now();

        // Collect AI prompts configuration
        let ai_prompts = self.collect_ai_prompts();

        // Collect traceability stats
        let (traceability, trace_links_by_req) = self.collect_traceability();

        // Check scaffold status if project root is set
        let (scaffold_status, scaffold_config) = if let Some(ref root) = self.project_root {
            self.check_scaffold_status(root)
        } else {
            (None, None)
        };

        // Collect type definitions
        let type_definitions: Vec<_> = self
            .store
            .type_definitions
            .iter()
            .map(|td| (td.name.clone(), td.description.clone().unwrap_or_default()))
            .collect();

        // Collect features
        let features: Vec<_> = self
            .store
            .features
            .iter()
            .map(|f| (f.name.clone(), f.prefix.clone()))
            .collect();

        AiIntegrationReport {
            project_name: self.store.name.clone(),
            project_description: self.store.description.clone(),
            database_path: self.database_path.clone(),
            generated_at: now,
            total_requirements: self.store.requirements.len(),
            ai_prompts,
            traceability,
            trace_links_by_req,
            scaffold_status,
            scaffold_config,
            type_definitions,
            features,
        }
    }

    fn collect_ai_prompts(&self) -> AiPromptsSection {
        let config = &self.store.ai_prompts;

        let global_context = if config.global_context.is_empty() {
            None
        } else {
            Some(config.global_context.clone())
        };

        let evaluation = if config.evaluation.custom_template.is_some()
            || !config.evaluation.additional_instructions.is_empty()
        {
            Some(PromptCustomization {
                action_name: "Evaluation".to_string(),
                custom_template: config.evaluation.custom_template.clone(),
                additional_instructions: if config.evaluation.additional_instructions.is_empty() {
                    None
                } else {
                    Some(config.evaluation.additional_instructions.clone())
                },
            })
        } else {
            None
        };

        let duplicates = if config.duplicates.custom_template.is_some()
            || !config.duplicates.additional_instructions.is_empty()
        {
            Some(PromptCustomization {
                action_name: "Find Duplicates".to_string(),
                custom_template: config.duplicates.custom_template.clone(),
                additional_instructions: if config.duplicates.additional_instructions.is_empty() {
                    None
                } else {
                    Some(config.duplicates.additional_instructions.clone())
                },
            })
        } else {
            None
        };

        let relationships = if config.relationships.custom_template.is_some()
            || !config.relationships.additional_instructions.is_empty()
        {
            Some(PromptCustomization {
                action_name: "Suggest Relationships".to_string(),
                custom_template: config.relationships.custom_template.clone(),
                additional_instructions: if config.relationships.additional_instructions.is_empty()
                {
                    None
                } else {
                    Some(config.relationships.additional_instructions.clone())
                },
            })
        } else {
            None
        };

        let improve = if config.improve.custom_template.is_some()
            || !config.improve.additional_instructions.is_empty()
        {
            Some(PromptCustomization {
                action_name: "Improve Description".to_string(),
                custom_template: config.improve.custom_template.clone(),
                additional_instructions: if config.improve.additional_instructions.is_empty() {
                    None
                } else {
                    Some(config.improve.additional_instructions.clone())
                },
            })
        } else {
            None
        };

        let generate_children = if config.generate_children.custom_template.is_some()
            || !config.generate_children.additional_instructions.is_empty()
        {
            Some(PromptCustomization {
                action_name: "Generate Children".to_string(),
                custom_template: config.generate_children.custom_template.clone(),
                additional_instructions: if config
                    .generate_children
                    .additional_instructions
                    .is_empty()
                {
                    None
                } else {
                    Some(config.generate_children.additional_instructions.clone())
                },
            })
        } else {
            None
        };

        let type_prompts: Vec<_> = config
            .type_prompts
            .iter()
            .map(|tp| TypePromptCustomization {
                type_name: tp.type_name.clone(),
                evaluation_extra: if tp.evaluation_extra.is_empty() {
                    None
                } else {
                    Some(tp.evaluation_extra.clone())
                },
                improve_extra: if tp.improve_extra.is_empty() {
                    None
                } else {
                    Some(tp.improve_extra.clone())
                },
                generate_children_extra: if tp.generate_children_extra.is_empty() {
                    None
                } else {
                    Some(tp.generate_children_extra.clone())
                },
            })
            .collect();

        AiPromptsSection {
            global_context,
            evaluation,
            duplicates,
            relationships,
            improve,
            generate_children,
            type_prompts,
        }
    }

    fn collect_traceability(&self) -> (TraceabilityStats, Vec<(String, String, Vec<TraceLink>)>) {
        let mut stats = TraceabilityStats::default();
        let mut trace_links_by_req = Vec::new();
        let mut unique_files = std::collections::HashSet::new();

        for req in &self.store.requirements {
            if !req.trace_links.is_empty() {
                stats.requirements_with_links += 1;
                let spec_id = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());
                trace_links_by_req.push((spec_id, req.title.clone(), req.trace_links.clone()));

                for link in &req.trace_links {
                    stats.total_links += 1;

                    // Count by artifact type
                    let type_name = format!("{:?}", link.artifact_type);
                    *stats.by_type.entry(type_name).or_insert(0) += 1;

                    // Count by confidence level (if available from notes)
                    // Parse from notes like "AI tool: claude" or look for patterns
                    if let Some(notes) = &link.notes {
                        if notes.contains("high") || notes.to_lowercase().contains("ai tool") {
                            *stats.by_confidence.entry("High".to_string()).or_insert(0) += 1;
                        } else if notes.contains("med") {
                            *stats.by_confidence.entry("Medium".to_string()).or_insert(0) += 1;
                        } else if notes.contains("low") {
                            *stats.by_confidence.entry("Low".to_string()).or_insert(0) += 1;
                        }
                    }

                    // Track unique files
                    if !link.file_path.is_empty() {
                        unique_files.insert(link.file_path.clone());
                    }
                }
            } else {
                stats.requirements_without_links += 1;
            }
        }

        stats.unique_files = unique_files.len();

        (stats, trace_links_by_req)
    }

    fn check_scaffold_status(
        &self,
        project_root: &Path,
    ) -> (Option<ScaffoldStatus>, Option<ScaffoldConfig>) {
        // Use default scaffold config
        let config = ScaffoldConfig::default();
        let db_path = PathBuf::from(&self.database_path);
        let mut scaffolder =
            Scaffolder::with_database(project_root.to_path_buf(), config.clone(), db_path);

        // Generate expected artifacts
        let preview = scaffolder.preview(&self.store);

        let mut status = ScaffoldStatus::new();

        for artifact in &preview.artifacts {
            let full_path = project_root.join(&artifact.path);

            if full_path.exists() {
                // Read actual content
                if let Ok(actual_content) = fs::read_to_string(&full_path) {
                    if actual_content.trim() == artifact.content.trim() {
                        status.matching.push(artifact.path.clone());
                    } else {
                        let expected_lines = artifact.content.lines().count();
                        let actual_lines = actual_content.lines().count();
                        status.modified.push((
                            artifact.path.clone(),
                            FileStatus::Modified {
                                expected_lines,
                                actual_lines,
                            },
                        ));
                        status.is_current = false;
                    }
                } else {
                    status.modified.push((
                        artifact.path.clone(),
                        FileStatus::Modified {
                            expected_lines: artifact.content.lines().count(),
                            actual_lines: 0,
                        },
                    ));
                    status.is_current = false;
                }
            } else {
                status.missing.push(artifact.path.clone());
                status.is_current = false;
            }
        }

        // Check for extra files in .claude directory
        let claude_dir = project_root.join(".claude");
        if claude_dir.exists() {
            if let Ok(entries) = fs::read_dir(&claude_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        let rel_path = path.strip_prefix(project_root).unwrap_or(&path);
                        if !preview.artifacts.iter().any(|a| a.path == rel_path) {
                            status.extra.push(rel_path.to_path_buf());
                        }
                    } else if path.is_dir() {
                        // Check subdirectories (commands, skills)
                        if let Ok(sub_entries) = fs::read_dir(&path) {
                            for sub_entry in sub_entries.flatten() {
                                let sub_path = sub_entry.path();
                                if sub_path.is_file() {
                                    let rel_path =
                                        sub_path.strip_prefix(project_root).unwrap_or(&sub_path);
                                    if !preview.artifacts.iter().any(|a| a.path == rel_path) {
                                        status.extra.push(rel_path.to_path_buf());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        (Some(status), Some(config))
    }

    /// Render report as markdown
    pub fn render_markdown(&self, report: &AiIntegrationReport) -> String {
        let mut md = String::new();

        // Title
        md.push_str(&format!(
            "# AI Integration Report: {}\n\n",
            report.project_name
        ));
        md.push_str(&format!(
            "*Generated: {}*\n\n",
            report.generated_at.format("%Y-%m-%d %H:%M UTC")
        ));

        // Project Overview
        md.push_str("## Project Overview\n\n");
        md.push_str(&format!("- **Database**: `{}`\n", report.database_path));
        md.push_str(&format!(
            "- **Total Requirements**: {}\n",
            report.total_requirements
        ));
        if !report.project_description.is_empty() {
            md.push_str(&format!("\n{}\n", report.project_description));
        }
        md.push('\n');

        // Features
        if !report.features.is_empty() {
            md.push_str("### Features\n\n");
            md.push_str("| Feature | Prefix |\n|---------|--------|\n");
            for (name, prefix) in &report.features {
                md.push_str(&format!("| {} | {} |\n", name, prefix));
            }
            md.push('\n');
        }

        // Type Definitions
        if !report.type_definitions.is_empty() {
            md.push_str("### Requirement Types\n\n");
            for (name, desc) in &report.type_definitions {
                if desc.is_empty() {
                    md.push_str(&format!("- **{}**\n", name));
                } else {
                    md.push_str(&format!("- **{}**: {}\n", name, desc));
                }
            }
            md.push('\n');
        }

        // AI Configuration
        md.push_str("## AI Configuration\n\n");

        if let Some(ref global_ctx) = report.ai_prompts.global_context {
            md.push_str("### Global Context\n\n");
            md.push_str("```\n");
            md.push_str(global_ctx);
            md.push_str("\n```\n\n");
        }

        // Prompt Customizations
        let customizations: Vec<&PromptCustomization> = [
            report.ai_prompts.evaluation.as_ref(),
            report.ai_prompts.duplicates.as_ref(),
            report.ai_prompts.relationships.as_ref(),
            report.ai_prompts.improve.as_ref(),
            report.ai_prompts.generate_children.as_ref(),
        ]
        .into_iter()
        .flatten()
        .collect();

        if !customizations.is_empty() {
            md.push_str("### Prompt Customizations\n\n");
            for cust in &customizations {
                md.push_str(&format!("#### {}\n\n", cust.action_name));
                if let Some(ref template) = cust.custom_template {
                    md.push_str("**Custom Template:**\n```\n");
                    md.push_str(template);
                    md.push_str("\n```\n\n");
                }
                if let Some(ref instructions) = cust.additional_instructions {
                    md.push_str("**Additional Instructions:**\n```\n");
                    md.push_str(instructions);
                    md.push_str("\n```\n\n");
                }
            }
        }

        // Type-specific prompts
        if !report.ai_prompts.type_prompts.is_empty() {
            md.push_str("### Type-Specific Customizations\n\n");
            for tp in &report.ai_prompts.type_prompts {
                md.push_str(&format!("#### Type: {}\n\n", tp.type_name));
                if let Some(ref eval) = tp.evaluation_extra {
                    md.push_str(&format!("- **Evaluation Extra**: {}\n", eval));
                }
                if let Some(ref imp) = tp.improve_extra {
                    md.push_str(&format!("- **Improve Extra**: {}\n", imp));
                }
                if let Some(ref gen) = tp.generate_children_extra {
                    md.push_str(&format!("- **Generate Children Extra**: {}\n", gen));
                }
                md.push('\n');
            }
        }

        if report.ai_prompts.global_context.is_none()
            && customizations.is_empty()
            && report.ai_prompts.type_prompts.is_empty()
        {
            md.push_str("*Using default AI prompts - no customizations configured.*\n\n");
        }

        // Code Traceability
        md.push_str("## Code Traceability\n\n");

        md.push_str("### Statistics\n\n");
        md.push_str(&format!(
            "- **Total Trace Links**: {}\n",
            report.traceability.total_links
        ));
        md.push_str(&format!(
            "- **Requirements with Links**: {} ({:.1}%)\n",
            report.traceability.requirements_with_links,
            if report.total_requirements > 0 {
                (report.traceability.requirements_with_links as f64
                    / report.total_requirements as f64)
                    * 100.0
            } else {
                0.0
            }
        ));
        md.push_str(&format!(
            "- **Requirements without Links**: {}\n",
            report.traceability.requirements_without_links
        ));
        md.push_str(&format!(
            "- **Unique Files Referenced**: {}\n\n",
            report.traceability.unique_files
        ));

        if !report.traceability.by_type.is_empty() {
            md.push_str("#### By Artifact Type\n\n");
            for (type_name, count) in &report.traceability.by_type {
                md.push_str(&format!("- {}: {}\n", type_name, count));
            }
            md.push('\n');
        }

        if !report.traceability.by_confidence.is_empty() {
            md.push_str("#### By Confidence Level\n\n");
            for (level, count) in &report.traceability.by_confidence {
                md.push_str(&format!("- {}: {}\n", level, count));
            }
            md.push('\n');
        }

        // Trace Links Detail
        if !report.trace_links_by_req.is_empty() {
            md.push_str("### Trace Links by Requirement\n\n");
            for (spec_id, title, links) in &report.trace_links_by_req {
                md.push_str(&format!("#### {} - {}\n\n", spec_id, title));
                for link in links {
                    let line_info = match (link.line_start, link.line_end) {
                        (Some(start), Some(end)) => format!(":{}–{}", start, end),
                        (Some(start), None) => format!(":{}", start),
                        _ => String::new(),
                    };
                    md.push_str(&format!("- `{}{}`", link.file_path, line_info));
                    if let Some(ref symbol) = link.symbol {
                        md.push_str(&format!(" (`{}`)", symbol));
                    }
                    md.push_str(&format!(" - {:?}", link.artifact_type));
                    if let Some(ref notes) = link.notes {
                        md.push_str(&format!(" - *{}*", notes));
                    }
                    md.push('\n');
                }
                md.push('\n');
            }
        }

        // Scaffolding Status
        if let Some(ref status) = report.scaffold_status {
            md.push_str("## Scaffolding Status\n\n");

            if status.is_current {
                md.push_str("**Status: Up to date**\n\n");
            } else {
                md.push_str("**Status: Drift detected**\n\n");
            }

            md.push_str(&format!(
                "- **Matching Files**: {}\n",
                status.matching.len()
            ));
            md.push_str(&format!(
                "- **Modified Files**: {}\n",
                status.modified.len()
            ));
            md.push_str(&format!("- **Missing Files**: {}\n", status.missing.len()));
            md.push_str(&format!("- **Extra Files**: {}\n\n", status.extra.len()));

            if !status.matching.is_empty() {
                md.push_str("### Matching Files\n\n");
                for path in &status.matching {
                    md.push_str(&format!("- `{}`\n", path.display()));
                }
                md.push('\n');
            }

            if !status.modified.is_empty() {
                md.push_str("### Modified Files\n\n");
                for (path, file_status) in &status.modified {
                    match file_status {
                        FileStatus::Modified {
                            expected_lines,
                            actual_lines,
                        } => {
                            md.push_str(&format!(
                                "- `{}` (expected {} lines, found {} lines)\n",
                                path.display(),
                                expected_lines,
                                actual_lines
                            ));
                        }
                        _ => {
                            md.push_str(&format!("- `{}`\n", path.display()));
                        }
                    }
                }
                md.push('\n');
            }

            if !status.missing.is_empty() {
                md.push_str("### Missing Files\n\n");
                for path in &status.missing {
                    md.push_str(&format!("- `{}`\n", path.display()));
                }
                md.push('\n');
            }

            if !status.extra.is_empty() {
                md.push_str("### Extra Files (not from scaffold)\n\n");
                for path in &status.extra {
                    md.push_str(&format!("- `{}`\n", path.display()));
                }
                md.push('\n');
            }
        }

        // Scaffold Configuration
        if let Some(ref config) = report.scaffold_config {
            md.push_str("### Scaffold Configuration\n\n");
            md.push_str(&format!("- **Project Type**: {:?}\n", config.project_type));
            md.push_str(&format!(
                "- **Generate CLAUDE.md**: {}\n",
                config.generate_claude_md
            ));
            md.push_str(&format!(
                "- **Generate Commands**: {}\n",
                config.generate_commands
            ));
            md.push_str(&format!(
                "- **Generate Skills**: {}\n",
                config.generate_skills
            ));
            md.push_str(&format!(
                "- **Generate Git Hooks**: {}\n",
                config.generate_git_hooks
            ));
            if !config.tech_stack.is_empty() {
                md.push_str(&format!(
                    "- **Tech Stack**: {}\n",
                    config.tech_stack.join(", ")
                ));
            }
            md.push('\n');
        }

        md.push_str("---\n\n");
        md.push_str("*This report was generated by AIDA (AI Design Assistant)*\n");

        md
    }

    /// Render report as HTML
    pub fn render_html(&self, report: &AiIntegrationReport) -> String {
        let markdown = self.render_markdown(report);

        // Use pulldown-cmark for proper markdown to HTML conversion
        use pulldown_cmark::{html, Options, Parser};

        let mut options = Options::empty();
        options.insert(Options::ENABLE_STRIKETHROUGH);
        options.insert(Options::ENABLE_TABLES);

        let parser = Parser::new_ext(&markdown, options);
        let mut html_body = String::new();
        html::push_html(&mut html_body, parser);

        format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>AI Integration Report: {}</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, sans-serif;
            line-height: 1.6;
            max-width: 900px;
            margin: 0 auto;
            padding: 20px;
            color: #333;
        }}
        h1 {{ color: #2c3e50; border-bottom: 2px solid #3498db; padding-bottom: 10px; }}
        h2 {{ color: #34495e; margin-top: 30px; }}
        h3 {{ color: #7f8c8d; }}
        h4 {{ color: #95a5a6; }}
        pre {{
            background: #f4f4f4;
            padding: 15px;
            border-radius: 5px;
            overflow-x: auto;
        }}
        code {{
            background: #f4f4f4;
            padding: 2px 5px;
            border-radius: 3px;
            font-family: 'Fira Code', monospace;
        }}
        pre code {{
            background: none;
            padding: 0;
        }}
        table {{
            border-collapse: collapse;
            width: 100%;
            margin: 15px 0;
        }}
        th, td {{
            border: 1px solid #ddd;
            padding: 8px;
            text-align: left;
        }}
        th {{
            background: #f4f4f4;
        }}
        li {{
            margin: 5px 0;
        }}
        .status-current {{ color: #27ae60; }}
        .status-drift {{ color: #e74c3c; }}
    </style>
</head>
<body>
{}
</body>
</html>"#,
            report.project_name, html_body
        )
    }
}

/// Check scaffold status for a project (standalone function for CLI)
pub fn check_scaffold_status(
    store: &RequirementsStore,
    project_root: &Path,
    config: &ScaffoldConfig,
    database_path: &Path,
) -> ScaffoldStatus {
    let mut scaffolder = Scaffolder::with_database(
        project_root.to_path_buf(),
        config.clone(),
        database_path.to_path_buf(),
    );
    let preview = scaffolder.preview(store);

    let mut status = ScaffoldStatus::new();

    // BUG-917: the AIDA source repo dogfoods its own scaffolding — the .claude/
    // scaffold files (commands, skills, settings.json) are per-file SYMLINKS into
    // aida-core/templates/ (the raw masters), which intentionally differ from the
    // generated artifact (frontmatter + substitution wrapping). Comparing a
    // symlinked master against the generated preview always reports false drift.
    // A scaffold target that is a symlink resolving under aida-core/templates/ is
    // the self-hosting pattern, not drift — count it matching. Downstream projects
    // have real files here, so this only triggers in the source repo.
    // trace:BUG-917 | ai:claude
    let templates_root = fs::canonicalize(project_root.join("aida-core").join("templates")).ok();

    for artifact in &preview.artifacts {
        let full_path = project_root.join(&artifact.path);

        if let Some(root) = &templates_root {
            let symlinked_master = fs::symlink_metadata(&full_path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
                && fs::canonicalize(&full_path)
                    .map(|t| t.starts_with(root))
                    .unwrap_or(false);
            if symlinked_master {
                status.matching.push(artifact.path.clone());
                continue;
            }
        }

        if full_path.exists() {
            if let Ok(actual_content) = fs::read_to_string(&full_path) {
                let matches =
                    file_matches_for_status(&artifact.path, &actual_content, &artifact.content);

                if matches {
                    status.matching.push(artifact.path.clone());
                } else {
                    let expected_lines = artifact.content.lines().count();
                    let actual_lines = actual_content.lines().count();
                    status.modified.push((
                        artifact.path.clone(),
                        FileStatus::Modified {
                            expected_lines,
                            actual_lines,
                        },
                    ));
                    status.is_current = false;
                }
            } else {
                status.modified.push((
                    artifact.path.clone(),
                    FileStatus::Modified {
                        expected_lines: artifact.content.lines().count(),
                        actual_lines: 0,
                    },
                ));
                status.is_current = false;
            }
        } else {
            status.missing.push(artifact.path.clone());
            status.is_current = false;
        }
    }

    // Check for extra files
    let claude_dir = project_root.join(".claude");
    if claude_dir.exists() {
        scan_extra_files(&claude_dir, project_root, &preview, &mut status.extra);
    }

    status
}

/// Decide whether a file matches expectation given its category.
///
/// - **Template** — whole-content equality (AIDA owns the file)
/// - **Seed** (CLAUDE.md) — presence-only (user owns the file)
/// - **Seed** (AGENTS.md) — block-content equality if AIDA-AUTOGEN markers
///   are present; presence-only otherwise
/// - **ManagedMerge** — slot-equality: every AIDA-owned JSON Pointer slot
///   must match expected; user keys outside the slots are ignored.
///   Mirrors what `aida scaffold upgrade` will actually do (FR-1-047).
///
/// trace:FR-1-028, FR-1-047 | ai:claude
fn file_matches_for_status(path: &Path, actual: &str, expected: &str) -> bool {
    let category = crate::scaffolding::FileCategory::from_path(path);
    // .claude/AIDA.md is Template-class but carries one section
    // (`## Claude Code skills`) that an `aida init --no-skills` project
    // legitimately drops. Compare it tolerant of that section so a clean
    // --no-skills init isn't flagged as drift. trace:TASK-125 | ai:claude
    if path.file_name().and_then(|s| s.to_str()) == Some("AIDA.md") {
        return crate::scaffolding::aida_md_matches(actual, expected);
    }
    match category {
        crate::scaffolding::FileCategory::Template => actual.trim() == expected.trim(),
        crate::scaffolding::FileCategory::Seed => seed_matches(path, actual, expected),
        crate::scaffolding::FileCategory::ManagedMerge => {
            managed_merge_matches(path, actual, expected)
        }
    }
}

/// Slot-equality check for managed-merge files. Parses both sides as
/// JSON, walks the path's declared slots, and considers them matching
/// when every AIDA-owned slot's value is identical. User keys outside
/// the slots are not compared. Falls back to whole-content equality if
/// either side is unparseable JSON (so malformed files still report
/// drift through this path).
/// trace:FR-1-047 | ai:claude
fn managed_merge_matches(path: &Path, actual: &str, expected: &str) -> bool {
    use serde_json::Value;
    let actual_v: Value = match serde_json::from_str(actual) {
        Ok(v) => v,
        Err(_) => return actual.trim() == expected.trim(),
    };
    let expected_v: Value = match serde_json::from_str(expected) {
        Ok(v) => v,
        Err(_) => return actual.trim() == expected.trim(),
    };
    let slots = crate::scaffolding::slots_for_file(path);
    if slots.is_empty() {
        return actual.trim() == expected.trim();
    }
    slots
        .iter()
        .all(|slot| actual_v.pointer(slot) == expected_v.pointer(slot))
}

fn seed_matches(path: &Path, actual: &str, expected: &str) -> bool {
    use crate::scaffolding::{claude_md_has_import, extract_aida_block};
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    match name {
        // CLAUDE.md is mostly user-owned, but AIDA does manage the
        // `@.claude/AIDA.md` import line. Drift only when that's missing.
        // trace:BUG-1-065 | ai:claude
        "CLAUDE.md" => claude_md_has_import(actual),
        "AGENTS.md" => {
            // If the user kept the AIDA-AUTOGEN markers, AIDA owns the
            // block content and compares it. If markers are absent, the
            // user opted out — we treat the file as fully theirs.
            match extract_aida_block(actual) {
                Some(actual_block) => match extract_aida_block(expected) {
                    Some(expected_block) => actual_block.trim() == expected_block.trim(),
                    // No expected block but actual has markers — shouldn't
                    // normally happen, but lean towards "matching" rather
                    // than flagging drift on a file that's no longer
                    // marker-coupled in the embedded template.
                    None => true,
                },
                None => true, // user opted out
            }
        }
        _ => actual.trim() == expected.trim(),
    }
}

fn scan_extra_files(
    dir: &Path,
    project_root: &Path,
    preview: &ScaffoldPreview,
    extra: &mut Vec<PathBuf>,
) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let rel_path = path.strip_prefix(project_root).unwrap_or(&path);
                if !preview.artifacts.iter().any(|a| a.path == rel_path) {
                    extra.push(rel_path.to_path_buf());
                }
            } else if path.is_dir() {
                // BUG-917: only descend into scaffold-MANAGED directories — a dir
                // that is an ancestor of some scaffold artifact. This stops the
                // walk counting runtime data (.claude/projects/ session
                // transcripts, statsig, todos, shell-snapshots, ...) as "extra
                // scaffold": in the AIDA source repo that was 40k+ false extras.
                // trace:BUG-917 | ai:claude
                let rel_dir = path.strip_prefix(project_root).unwrap_or(&path);
                if preview
                    .artifacts
                    .iter()
                    .any(|a| a.path.starts_with(rel_dir))
                {
                    scan_extra_files(&path, project_root, preview, extra);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scaffold_status_new() {
        let status = ScaffoldStatus::new();
        assert!(status.is_current);
        assert!(status.matching.is_empty());
        assert!(status.modified.is_empty());
        assert!(status.missing.is_empty());
        assert!(status.extra.is_empty());
    }

    #[test]
    fn test_traceability_stats_default() {
        let stats = TraceabilityStats::default();
        assert_eq!(stats.total_links, 0);
        assert_eq!(stats.requirements_with_links, 0);
    }
}
