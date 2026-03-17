// trace:FR-0152,FR-0226 | ai:claude:high
//! AI Project Scaffolding Module
//!
//! Provides functionality to generate AI coding agent integration artifacts:
//! - CLAUDE.md project instructions
//! - AGENTS.md project instructions
//! - .claude/commands/ directory with project-specific slash commands
//! - .claude/skills/ directory with requirements-driven development skills
//! - .codex/skills/ directory with requirements-driven development skills
//! - .git/hooks/ directory with traceability validation hooks
//! - Code traceability configuration

mod claude_md;
mod codex_md;
mod hooks;
mod settings;

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::models::RequirementsStore;
use crate::templates::TemplateLoader;

/// Current scaffolding version - increment when templates change significantly
pub const SCAFFOLD_VERSION: &str = "2.0.0";

/// Compute a simple checksum for content (first 8 chars of hex-encoded hash)
fn compute_checksum(content: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())[..8].to_string()
}

/// Generate the AIDA header for a markdown file
fn generate_aida_header(content: &str) -> String {
    let checksum = compute_checksum(content);
    format!(
        "<!-- AIDA Generated: v{} | checksum:{} | DO NOT EDIT DIRECTLY -->\n\
         <!-- To customize: copy this file and modify the copy -->\n\n",
        SCAFFOLD_VERSION, checksum
    )
}

/// Generate the AIDA header for a shell script file
fn generate_aida_header_shell(content: &str) -> String {
    let checksum = compute_checksum(content);
    format!(
        "# AIDA Generated: v{} | checksum:{}\n\
         # To customize: copy this file and modify the copy\n",
        SCAFFOLD_VERSION, checksum
    )
}

/// Find the AIDA header line in file content, skipping YAML frontmatter if present
fn find_aida_header_line(content: &str) -> Option<&str> {
    // If content starts with YAML frontmatter, skip past it
    if content.starts_with("---\n") || content.starts_with("---\r\n") {
        // Find the closing --- after the opening one
        let after_open = if content.starts_with("---\r\n") { 5 } else { 4 };
        if let Some(close_pos) = content[after_open..].find("\n---") {
            let after_close = after_open + close_pos + 4; // past "\n---"
                                                          // Skip the newline after closing ---
            let rest = content[after_close..]
                .trim_start_matches('\r')
                .trim_start_matches('\n');
            return rest.lines().next();
        }
    }

    // No frontmatter - header should be on the first line
    content.lines().next()
}

/// Parse an existing file to determine its status relative to expected content
fn check_file_status(file_path: &PathBuf, expected_content: &str) -> FileStatus {
    let content = match fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(_) => return FileStatus::New,
    };

    // Try to parse AIDA header (markdown format)
    // Format: <!-- AIDA Generated: v{version} | checksum:{hash} | DO NOT EDIT DIRECTLY -->
    let md_header_pattern = regex::Regex::new(
        r"^<!-- AIDA Generated: v([0-9.]+) \| checksum:([a-f0-9]+) \| DO NOT EDIT DIRECTLY -->",
    )
    .unwrap();

    // Try to parse AIDA header (shell format)
    // Format: # AIDA Generated: v{version} | checksum:{hash}
    let shell_header_pattern =
        regex::Regex::new(r"^# AIDA Generated: v([0-9.]+) \| checksum:([a-f0-9]+)").unwrap();

    // Find the header line, skipping frontmatter if present
    let header_line = find_aida_header_line(&content).unwrap_or("");

    // Check markdown header
    if let Some(caps) = md_header_pattern.captures(header_line) {
        let file_version = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let stored_checksum = caps.get(2).map(|m| m.as_str()).unwrap_or("");

        // Check version first
        if file_version != SCAFFOLD_VERSION {
            return FileStatus::OlderVersion {
                file_version: file_version.to_string(),
            };
        }

        // Compute checksum of the expected content (without header)
        let expected_checksum = compute_checksum(expected_content);

        if stored_checksum == expected_checksum {
            return FileStatus::Unmodified;
        } else {
            return FileStatus::Modified {
                expected_checksum,
                actual_checksum: stored_checksum.to_string(),
            };
        }
    }

    // Check shell header
    if let Some(caps) = shell_header_pattern.captures(header_line) {
        let file_version = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let stored_checksum = caps.get(2).map(|m| m.as_str()).unwrap_or("");

        // Check version first
        if file_version != SCAFFOLD_VERSION {
            return FileStatus::OlderVersion {
                file_version: file_version.to_string(),
            };
        }

        // Compute checksum of the expected content (without header)
        let expected_checksum = compute_checksum(expected_content);

        if stored_checksum == expected_checksum {
            return FileStatus::Unmodified;
        } else {
            return FileStatus::Modified {
                expected_checksum,
                actual_checksum: stored_checksum.to_string(),
            };
        }
    }

    // No AIDA header found - file exists but wasn't generated by AIDA
    FileStatus::NoHeader
}

/// Status of an existing scaffolded file
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    /// File doesn't exist yet
    New,
    /// File exists and matches expected checksum (safe to overwrite)
    Unmodified,
    /// File exists but checksum differs (user modified)
    Modified {
        expected_checksum: String,
        actual_checksum: String,
    },
    /// File exists but has no AIDA header (unknown origin)
    NoHeader,
    /// File exists with older version (can be upgraded)
    OlderVersion { file_version: String },
}

/// Configuration for what scaffolding artifacts to generate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaffoldConfig {
    /// Generate CLAUDE.md project instructions
    pub generate_claude_md: bool,
    /// Generate AGENTS.md project instructions for Codex-compatible agents
    pub generate_agents_md: bool,
    /// Generate .claude/commands/ directory with slash commands
    pub generate_commands: bool,
    /// Generate .claude/skills/ directory with skills
    pub generate_skills: bool,
    /// Generate .codex/skills/ directory with Codex-compatible skills
    pub generate_codex_skills: bool,
    /// Include aida-req skill for requirement creation
    pub include_aida_req_skill: bool,
    /// Include aida-plan skill for implementation planning
    pub include_aida_plan_skill: bool,
    /// Include aida-implement skill for requirement implementation
    pub include_aida_implement_skill: bool,
    /// Include aida-capture skill for session review
    pub include_aida_capture_skill: bool,
    /// Include aida-docs skill for documentation management
    pub include_aida_docs_skill: bool,
    /// Include aida-docs-review skill for exhaustive documentation quality review
    pub include_aida_docs_review_skill: bool,
    /// Include aida-release skill for release management
    pub include_aida_release_skill: bool,
    /// Include aida-evaluate skill for requirement quality evaluation
    pub include_aida_evaluate_skill: bool,
    /// Include aida-commit skill for commit with requirement linking
    pub include_aida_commit_skill: bool,
    /// Include aida-sync skill for template synchronization
    pub include_aida_sync_skill: bool,
    /// Include aida-test skill for test generation linked to requirements
    pub include_aida_test_skill: bool,
    /// Include aida-review skill for code review against specs
    pub include_aida_review_skill: bool,
    /// Include aida-onboard skill for project onboarding
    pub include_aida_onboard_skill: bool,
    /// Include aida-sprint skill for sprint planning
    pub include_aida_sprint_skill: bool,
    /// Include aida-search skill for unified search
    pub include_aida_search_skill: bool,
    /// Include aida-standup skill for daily standup generation
    pub include_aida_standup_skill: bool,
    /// Generate git hooks for traceability validation
    pub generate_git_hooks: bool,
    /// Include commit-msg hook for AI attribution validation
    pub include_commit_msg_hook: bool,
    /// Include pre-commit hook for trace comment validation
    pub include_pre_commit_hook: bool,
    /// Generate Claude Code hooks for AIDA integration
    pub generate_claude_code_hooks: bool,
    /// Include commit validation hook (PreToolUse)
    pub include_validate_commit_hook: bool,
    /// Include commit tracking hook (PostToolUse)
    pub include_track_commits_hook: bool,
    /// Custom project type for specialized scaffolding
    pub project_type: ProjectType,
    /// Tech stack hints for context generation
    pub tech_stack: Vec<String>,
}

impl Default for ScaffoldConfig {
    fn default() -> Self {
        Self {
            generate_claude_md: true,
            generate_agents_md: true,
            generate_commands: true,
            generate_skills: true,
            generate_codex_skills: true,
            include_aida_req_skill: true,
            include_aida_plan_skill: true,
            include_aida_implement_skill: true,
            include_aida_capture_skill: true,
            include_aida_docs_skill: true,
            include_aida_docs_review_skill: true,
            include_aida_release_skill: true,
            include_aida_evaluate_skill: true,
            include_aida_commit_skill: true,
            include_aida_sync_skill: true,
            include_aida_test_skill: true,
            include_aida_review_skill: true,
            include_aida_onboard_skill: true,
            include_aida_sprint_skill: true,
            include_aida_search_skill: true,
            include_aida_standup_skill: true,
            generate_git_hooks: true,
            include_commit_msg_hook: true,
            include_pre_commit_hook: false, // Optional, disabled by default
            generate_claude_code_hooks: true,
            include_validate_commit_hook: true,
            include_track_commits_hook: true,
            project_type: ProjectType::Generic,
            tech_stack: Vec::new(),
        }
    }
}

/// Project type for specialized scaffolding
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProjectType {
    #[default]
    Generic,
    Rust,
    Python,
    TypeScript,
    Web,
    Api,
    Cli,
}

impl ProjectType {
    /// Get all project types for UI selection
    pub fn all() -> &'static [ProjectType] {
        &[
            ProjectType::Generic,
            ProjectType::Rust,
            ProjectType::Python,
            ProjectType::TypeScript,
            ProjectType::Web,
            ProjectType::Api,
            ProjectType::Cli,
        ]
    }

    /// Get display label for the project type
    pub fn label(&self) -> &'static str {
        match self {
            ProjectType::Generic => "Generic",
            ProjectType::Rust => "Rust",
            ProjectType::Python => "Python",
            ProjectType::TypeScript => "TypeScript",
            ProjectType::Web => "Web Application",
            ProjectType::Api => "API/Backend",
            ProjectType::Cli => "CLI Tool",
        }
    }
}

/// Represents a scaffolding artifact to be generated
#[derive(Debug, Clone)]
pub struct ScaffoldArtifact {
    /// Relative path from project root
    pub path: PathBuf,
    /// Content of the artifact (with AIDA header)
    pub content: String,
    /// Description of what this artifact does
    pub description: String,
    /// Whether the file already exists
    pub exists: bool,
    /// Status of existing file (if any)
    pub file_status: FileStatus,
}

/// Result of scaffolding preview
#[derive(Debug, Clone)]
pub struct ScaffoldPreview {
    /// Artifacts to be generated
    pub artifacts: Vec<ScaffoldArtifact>,
    /// Files that would be overwritten
    pub overwrites: Vec<PathBuf>,
    /// New files that would be created
    pub new_files: Vec<PathBuf>,
    /// Directories that would be created
    pub new_dirs: Vec<PathBuf>,
    /// Files that have been modified by user (need confirmation to overwrite)
    pub modified_files: Vec<PathBuf>,
    /// Files with older AIDA versions (safe to upgrade)
    pub upgradeable_files: Vec<PathBuf>,
}

/// Options for applying scaffolding
#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    /// Force overwrite of modified files (ignores user modifications)
    pub force: bool,
}

/// Scaffolding generator
pub struct Scaffolder {
    /// Project root directory
    project_root: PathBuf,
    /// Scaffolding configuration
    config: ScaffoldConfig,
    /// Database path (to determine backend type)
    database_path: Option<PathBuf>,
    /// Template loader for external/embedded templates (used for customization fallback chain)
    #[allow(dead_code)]
    template_loader: TemplateLoader,
}

impl Scaffolder {
    /// Create a new scaffolder for the given project directory
    pub fn new(project_root: PathBuf, config: ScaffoldConfig) -> Self {
        let template_loader = TemplateLoader::with_project_root(&project_root);
        Self {
            project_root,
            config,
            database_path: None,
            template_loader,
        }
    }

    /// Create a new scaffolder with database path for backend-aware scaffolding
    pub fn with_database(
        project_root: PathBuf,
        config: ScaffoldConfig,
        database_path: PathBuf,
    ) -> Self {
        let template_loader = TemplateLoader::with_project_root(&project_root);
        Self {
            project_root,
            config,
            database_path: Some(database_path),
            template_loader,
        }
    }

    /// Check if the database is SQLite based on path extension
    fn is_sqlite_database(&self) -> bool {
        self.database_path
            .as_ref()
            .map(|p| p.extension().map(|e| e == "db").unwrap_or(false))
            .unwrap_or(false)
    }

    /// Get the database filename for display
    fn database_filename(&self) -> String {
        self.database_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "requirements.yaml".to_string())
    }

    /// Load a template from external sources or embedded, with fallback chain
    #[allow(dead_code)]
    fn load_template(&mut self, key: &str) -> Option<String> {
        self.template_loader.load(key)
    }

    /// Helper to create an artifact with version/checksum header and file status checking
    fn create_artifact(
        &self,
        path: PathBuf,
        raw_content: String,
        description: String,
        is_shell: bool,
    ) -> ScaffoldArtifact {
        let full_path = self.project_root.join(&path);
        let exists = full_path.exists();

        // Check file status against the raw content (what we're comparing against)
        let file_status = if exists {
            check_file_status(&full_path, &raw_content)
        } else {
            FileStatus::New
        };

        // Generate content with appropriate header
        // For files with YAML frontmatter (---), insert header AFTER the closing ---
        let content = if is_shell {
            format!(
                "{}{}",
                generate_aida_header_shell(&raw_content),
                raw_content
            )
        } else if raw_content.starts_with("---\n") {
            // Split at the closing --- and insert header after frontmatter
            let after_open = 4; // past "---\n"
            if let Some(close_pos) = raw_content[after_open..].find("\n---\n") {
                let fm_end = after_open + close_pos + 5; // past "\n---\n"
                let (frontmatter, body) = raw_content.split_at(fm_end);
                format!("{}{}{}", frontmatter, generate_aida_header(body), body)
            } else {
                format!("{}{}", generate_aida_header(&raw_content), raw_content)
            }
        } else {
            format!("{}{}", generate_aida_header(&raw_content), raw_content)
        };

        ScaffoldArtifact {
            path,
            content,
            description,
            exists,
            file_status,
        }
    }

    /// Generate a preview of what would be scaffolded
    pub fn preview(&mut self, store: &RequirementsStore) -> ScaffoldPreview {
        let mut artifacts = Vec::new();
        let mut overwrites = Vec::new();
        let mut new_files = Vec::new();
        let mut new_dirs = HashSet::new();
        let mut modified_files = Vec::new();
        let mut upgradeable_files = Vec::new();

        // CLAUDE.md - Note: CLAUDE.md is user-edited, so no AIDA header
        if self.config.generate_claude_md {
            let path = PathBuf::from("CLAUDE.md");
            let full_path = self.project_root.join(&path);
            let exists = full_path.exists();
            let content = self.generate_claude_md(store);

            if exists {
                overwrites.push(path.clone());
            } else {
                new_files.push(path.clone());
            }

            artifacts.push(ScaffoldArtifact {
                path,
                content,
                description: "Project instructions for Claude Code".to_string(),
                exists,
                file_status: if exists {
                    FileStatus::NoHeader
                } else {
                    FileStatus::New
                },
            });
        }

        // AGENTS.md - Note: AGENTS.md is user-edited, so no AIDA header
        if self.config.generate_agents_md {
            let path = PathBuf::from("AGENTS.md");
            let full_path = self.project_root.join(&path);
            let exists = full_path.exists();
            let content = self.generate_agents_md(store);

            if exists {
                overwrites.push(path.clone());
            } else {
                new_files.push(path.clone());
            }

            artifacts.push(ScaffoldArtifact {
                path,
                content,
                description: "Project instructions for Codex-compatible agents".to_string(),
                exists,
                file_status: if exists {
                    FileStatus::NoHeader
                } else {
                    FileStatus::New
                },
            });
        }

        // .claude/commands/ directory
        if self.config.generate_commands {
            new_dirs.insert(PathBuf::from(".claude/commands"));

            // Add default commands
            let commands = self.generate_commands(store);
            for (name, content, desc) in commands {
                let path = PathBuf::from(format!(".claude/commands/{}.md", name));
                let artifact = self.create_artifact(path.clone(), content, desc, false);

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }
        }

        // .claude/skills/ directory
        if self.config.generate_skills {
            new_dirs.insert(PathBuf::from(".claude/skills"));

            // Add aida-req skill
            if self.config.include_aida_req_skill {
                let path = PathBuf::from(".claude/skills/aida-req.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_req_skill(),
                    "Skill for adding requirements with AI evaluation".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-plan skill
            if self.config.include_aida_plan_skill {
                let path = PathBuf::from(".claude/skills/aida-plan.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_plan_skill(),
                    "Skill for planning requirement implementation".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-implement skill
            if self.config.include_aida_implement_skill {
                let path = PathBuf::from(".claude/skills/aida-implement.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_implement_skill(),
                    "Skill for implementing requirements with traceability".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-capture skill
            if self.config.include_aida_capture_skill {
                let path = PathBuf::from(".claude/skills/aida-capture.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_capture_skill(),
                    "Skill for capturing missed requirements from session".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-docs skill
            if self.config.include_aida_docs_skill {
                let path = PathBuf::from(".claude/skills/aida-docs.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_docs_skill(),
                    "Skill for documentation management and generation".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-docs-review skill
            if self.config.include_aida_docs_review_skill {
                let path = PathBuf::from(".claude/skills/aida-docs-review.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_docs_review_skill(),
                    "Skill for exhaustive documentation quality review".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-release skill
            if self.config.include_aida_release_skill {
                let path = PathBuf::from(".claude/skills/aida-release.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_release_skill(),
                    "Skill for release management and version bumping".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-evaluate skill
            if self.config.include_aida_evaluate_skill {
                let path = PathBuf::from(".claude/skills/aida-evaluate.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_evaluate_skill(),
                    "Skill for evaluating requirement quality".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-commit skill
            if self.config.include_aida_commit_skill {
                let path = PathBuf::from(".claude/skills/aida-commit.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_commit_skill(),
                    "Skill for committing with requirement linking".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-sync skill
            if self.config.include_aida_sync_skill {
                let path = PathBuf::from(".claude/skills/aida-sync.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_sync_skill(),
                    "Skill for template synchronization".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-test skill
            if self.config.include_aida_test_skill {
                let path = PathBuf::from(".claude/skills/aida-test.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_test_skill(),
                    "Skill for generating tests linked to requirements".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-review skill
            if self.config.include_aida_review_skill {
                let path = PathBuf::from(".claude/skills/aida-review.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_review_skill(),
                    "Skill for reviewing code changes against specs".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-onboard skill
            if self.config.include_aida_onboard_skill {
                let path = PathBuf::from(".claude/skills/aida-onboard.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_onboard_skill(),
                    "Skill for project onboarding".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-sprint skill
            if self.config.include_aida_sprint_skill {
                let path = PathBuf::from(".claude/skills/aida-sprint.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_sprint_skill(),
                    "Skill for sprint planning".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-search skill
            if self.config.include_aida_search_skill {
                let path = PathBuf::from(".claude/skills/aida-search.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_search_skill(),
                    "Skill for unified search across requirements and code".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-standup skill
            if self.config.include_aida_standup_skill {
                let path = PathBuf::from(".claude/skills/aida-standup.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_standup_skill(),
                    "Skill for daily standup generation".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }
        }

        // .codex/skills/ directory
        if self.config.generate_codex_skills {
            new_dirs.insert(PathBuf::from(".codex/skills"));

            let codex_skill_defs = [
                ("aida-req", self.config.include_aida_req_skill),
                ("aida-plan", self.config.include_aida_plan_skill),
                ("aida-implement", self.config.include_aida_implement_skill),
                ("aida-capture", self.config.include_aida_capture_skill),
                ("aida-docs", self.config.include_aida_docs_skill),
                ("aida-docs-review", self.config.include_aida_docs_review_skill),
                ("aida-release", self.config.include_aida_release_skill),
                ("aida-evaluate", self.config.include_aida_evaluate_skill),
                ("aida-commit", self.config.include_aida_commit_skill),
                ("aida-sync", self.config.include_aida_sync_skill),
                ("aida-test", self.config.include_aida_test_skill),
                ("aida-review", self.config.include_aida_review_skill),
                ("aida-onboard", self.config.include_aida_onboard_skill),
                ("aida-sprint", self.config.include_aida_sprint_skill),
                ("aida-search", self.config.include_aida_search_skill),
                ("aida-standup", self.config.include_aida_standup_skill),
            ];

            for (name, enabled) in codex_skill_defs {
                if !enabled {
                    continue;
                }
                let path = PathBuf::from(format!(".codex/skills/{}/SKILL.md", name));
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_codex_skill(name),
                    format!("Codex-compatible skill {}", name),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }
        }

        // .mcp.json — MCP server configuration for Claude Code
        {
            let mcp_content = r#"{
  "mcpServers": {
    "aida": {
      "type": "stdio",
      "command": "aida",
      "args": ["mcp-serve"]
    }
  }
}"#
            .to_string();
            let path = PathBuf::from(".mcp.json");
            let artifact = self.create_artifact(
                path.clone(),
                mcp_content,
                "MCP server configuration for Claude Code".to_string(),
                false,
            );

            match &artifact.file_status {
                FileStatus::New => new_files.push(path),
                FileStatus::Modified { .. } | FileStatus::NoHeader => {
                    modified_files.push(artifact.path.clone())
                }
                FileStatus::OlderVersion { .. } => upgradeable_files.push(artifact.path.clone()),
                FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
            }

            artifacts.push(artifact);
        }

        // .git/hooks/ directory (only if .git exists)
        if self.config.generate_git_hooks && self.project_root.join(".git").exists() {
            new_dirs.insert(PathBuf::from(".git/hooks"));

            // commit-msg hook
            if self.config.include_commit_msg_hook {
                let path = PathBuf::from(".git/hooks/commit-msg");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_commit_msg_hook(),
                    "Git hook for validating AI attribution in commit messages".to_string(),
                    true, // shell script
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // pre-commit hook
            if self.config.include_pre_commit_hook {
                let path = PathBuf::from(".git/hooks/pre-commit");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_pre_commit_hook(),
                    "Git hook for validating trace comments before commit".to_string(),
                    true, // shell script
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }
        }

        // Claude Code hooks (in .claude/hooks/)
        if self.config.generate_claude_code_hooks {
            new_dirs.insert(PathBuf::from(".claude/hooks"));

            // Validate commit hook (PreToolUse)
            if self.config.include_validate_commit_hook {
                let path = PathBuf::from(".claude/hooks/aida-validate-commit.sh");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_validate_commit_hook(),
                    "Claude Code hook for validating commit messages reference requirements"
                        .to_string(),
                    true, // shell script
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Track commits hook (PostToolUse)
            if self.config.include_track_commits_hook {
                let path = PathBuf::from(".claude/hooks/aida-track-commits.sh");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_track_commits_hook(),
                    "Claude Code hook for updating requirement status after commits".to_string(),
                    true, // shell script
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Generate settings.json with hook configuration
            let path = PathBuf::from(".claude/settings.json");
            let artifact = self.create_artifact(
                path.clone(),
                self.generate_claude_settings_json(),
                "Claude Code settings with AIDA hook configuration".to_string(),
                false, // JSON file
            );

            match &artifact.file_status {
                FileStatus::New => new_files.push(path),
                FileStatus::Modified { .. } | FileStatus::NoHeader => {
                    modified_files.push(artifact.path.clone())
                }
                FileStatus::OlderVersion { .. } => upgradeable_files.push(artifact.path.clone()),
                FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
            }

            artifacts.push(artifact);
        }

        // Filter new_dirs to only include those that don't exist
        let new_dirs: Vec<PathBuf> = new_dirs
            .into_iter()
            .filter(|d| !self.project_root.join(d).exists())
            .collect();

        ScaffoldPreview {
            artifacts,
            overwrites,
            new_files,
            new_dirs,
            modified_files,
            upgradeable_files,
        }
    }

    /// Apply the scaffolding (write files) - writes all files regardless of status
    /// For more control, use `apply_with_options`
    pub fn apply(&self, preview: &ScaffoldPreview) -> Result<Vec<PathBuf>, ScaffoldError> {
        self.apply_with_options(preview, &ApplyOptions::default())
    }

    /// Apply the scaffolding with options to control behavior for modified files
    pub fn apply_with_options(
        &self,
        preview: &ScaffoldPreview,
        options: &ApplyOptions,
    ) -> Result<Vec<PathBuf>, ScaffoldError> {
        let mut written_files = Vec::new();
        let mut skipped_files = Vec::new();

        // Create directories first
        for dir in &preview.new_dirs {
            let full_path = self.project_root.join(dir);
            fs::create_dir_all(&full_path).map_err(|e| ScaffoldError::IoError {
                path: full_path.clone(),
                message: e.to_string(),
            })?;
        }

        // Also ensure parent directories exist for all artifacts
        for artifact in &preview.artifacts {
            if let Some(parent) = artifact.path.parent() {
                let full_parent = self.project_root.join(parent);
                if !full_parent.exists() {
                    fs::create_dir_all(&full_parent).map_err(|e| ScaffoldError::IoError {
                        path: full_parent.clone(),
                        message: e.to_string(),
                    })?;
                }
            }
        }

        // Write artifacts based on their status and options
        for artifact in &preview.artifacts {
            let should_write = match &artifact.file_status {
                FileStatus::New => true,
                FileStatus::Unmodified => true,
                FileStatus::OlderVersion { .. } => true, // Always upgrade
                FileStatus::Modified { .. } => {
                    // User modified - only write if --force
                    options.force
                }
                FileStatus::NoHeader => {
                    // No header means unknown origin - only write if --force
                    options.force
                }
            };

            if !should_write {
                skipped_files.push(artifact.path.clone());
                continue;
            }

            let full_path = self.project_root.join(&artifact.path);
            fs::write(&full_path, &artifact.content).map_err(|e| ScaffoldError::IoError {
                path: full_path.clone(),
                message: e.to_string(),
            })?;

            // Make git hooks and Claude Code hooks executable on Unix
            #[cfg(unix)]
            if artifact.path.starts_with(".git/hooks/")
                || artifact.path.starts_with(".claude/hooks/")
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&full_path)
                    .map_err(|e| ScaffoldError::IoError {
                        path: full_path.clone(),
                        message: e.to_string(),
                    })?
                    .permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&full_path, perms).map_err(|e| ScaffoldError::IoError {
                    path: full_path.clone(),
                    message: e.to_string(),
                })?;
            }

            written_files.push(artifact.path.clone());
        }

        Ok(written_files)
    }

    /// Generate slash commands
    fn generate_commands(&self, _store: &RequirementsStore) -> Vec<(String, String, String)> {
        use crate::templates::EMBEDDED_TEMPLATES;

        // Command definitions: (template_key, output_name, description)
        let command_defs = [
            (
                "commands/aida-status.md",
                "aida-status",
                "Show project requirements status",
            ),
            (
                "commands/aida-review.md",
                "aida-review",
                "Review a requirement for quality",
            ),
            (
                "commands/aida-req.md",
                "aida-req",
                "Add a new requirement with AI evaluation",
            ),
            (
                "commands/aida-implement.md",
                "aida-implement",
                "Implement a requirement with traceability",
            ),
            (
                "commands/aida-capture.md",
                "aida-capture",
                "Capture missed requirements from session",
            ),
            (
                "commands/aida-evaluate.md",
                "aida-evaluate",
                "Evaluate requirement quality with AI",
            ),
            (
                "commands/aida-commit.md",
                "aida-commit",
                "Commit with requirement linking",
            ),
            (
                "commands/aida-sync.md",
                "aida-sync",
                "Sync templates and scaffolding",
            ),
            (
                "commands/aida-test.md",
                "aida-test",
                "Generate tests linked to requirements",
            ),
            (
                "commands/aida-onboard.md",
                "aida-onboard",
                "Project onboarding for new team members",
            ),
            (
                "commands/aida-sprint.md",
                "aida-sprint",
                "Sprint planning from approved requirements",
            ),
            (
                "commands/aida-search.md",
                "aida-search",
                "Unified search across requirements and code",
            ),
            (
                "commands/aida-standup.md",
                "aida-standup",
                "Daily standup summary from recent activity",
            ),
            (
                "commands/aida-docs-review.md",
                "aida-docs-review",
                "Exhaustive documentation quality review",
            ),
        ];

        command_defs
            .iter()
            .filter_map(|(key, name, desc)| {
                EMBEDDED_TEMPLATES
                    .get(key)
                    .map(|content| (name.to_string(), content.to_string(), desc.to_string()))
            })
            .collect()
    }

    /// Generate aida-req skill content (loads from embedded template)
    fn generate_aida_req_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-req.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                "# AIDA Requirement Creation Skill\n\n(template not found)".to_string()
            })
    }

    /// Generate aida-implement skill content (loads from embedded template)
    fn generate_aida_implement_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-implement.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Implementation Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-plan skill content (loads from embedded template)
    fn generate_aida_plan_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-plan.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Planning Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-capture skill content (loads from embedded template)
    fn generate_aida_capture_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-capture.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Session Capture Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-docs skill content (loads from embedded template)
    fn generate_aida_docs_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-docs.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Documentation Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-docs-review skill content (loads from embedded template)
    fn generate_aida_docs_review_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-docs-review.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                "# AIDA Documentation Review Skill\n\n(template not found)".to_string()
            })
    }

    /// Generate aida-release skill content (loads from embedded template)
    fn generate_aida_release_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-release.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                "# AIDA Release Management Skill\n\n(template not found)".to_string()
            })
    }

    /// Generate aida-evaluate skill content (loads from embedded template)
    fn generate_aida_evaluate_skill(&self) -> String {
        // Load from embedded templates at compile time
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-evaluate.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                r#"# AIDA Requirement Evaluation Skill

## Purpose

Evaluate a requirement's quality using AI analysis.

## When to Use

Use this skill when:
- User wants to evaluate a specific requirement's quality
- User asks to "evaluate", "assess", or "review" a requirement

## Workflow

1. Load the requirement from database: `aida show <SPEC-ID>`
2. Run AI evaluation for clarity, testability, completeness, consistency
3. Display quality score and issues found
4. Offer follow-up actions: improve, split, or accept
"#
                .to_string()
            })
    }

    /// Generate aida-commit skill content (loads from embedded template)
    fn generate_aida_commit_skill(&self) -> String {
        // Load from embedded templates at compile time
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-commit.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                r#"# AIDA Commit Skill

## Purpose

Create git commits with automatic requirement linkage.

## When to Use

Use this skill when:
- User wants to commit changes with requirement traceability
- User says "commit" after implementing features

## Workflow

1. Analyze staged changes and extract requirement traces
2. Check for untraced implementation code
3. Offer to create requirements for untraced work
4. Create commit with requirement links
5. Update linked requirement statuses
"#
                .to_string()
            })
    }

    /// Generate aida-sync skill content (loads from embedded template)
    fn generate_aida_sync_skill(&self) -> String {
        // Load from embedded templates at compile time
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-sync.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                r#"# AIDA Sync Skill

## Purpose

Maintain consistency between AIDA templates and scaffolded projects.

## When to Use

Use this skill when:
- You've modified templates in `aida-core/templates/`
- You want to check scaffold status
- At the end of an AIDA development session

## Workflow

1. Detect environment (AIDA repo vs scaffolded project)
2. For AIDA repo: Check template integrity
3. For other projects: Check scaffold status
4. Ensure templates and skills are consistent
"#
                .to_string()
            })
    }

    /// Generate aida-test skill content (loads from embedded template)
    fn generate_aida_test_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-test.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Test Generation Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-review skill content (loads from embedded template)
    fn generate_aida_review_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-review.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Code Review Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-onboard skill content (loads from embedded template)
    fn generate_aida_onboard_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-onboard.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                "# AIDA Project Onboarding Skill\n\n(template not found)".to_string()
            })
    }

    /// Generate aida-sprint skill content (loads from embedded template)
    fn generate_aida_sprint_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-sprint.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Sprint Planning Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-search skill content (loads from embedded template)
    fn generate_aida_search_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-search.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Unified Search Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-standup skill content (loads from embedded template)
    fn generate_aida_standup_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-standup.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Standup Skill\n\n(template not found)".to_string())
    }

    /// Generate Codex skill content from an embedded Claude skill template.
    /// Converts frontmatter-style skill files into plain SKILL.md content.
    fn generate_codex_skill(&self, skill_name: &str) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;

        let key = format!("skills/{}.md", skill_name);
        let raw = EMBEDDED_TEMPLATES
            .get(key.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("# {}\n\n(template not found)", skill_name));

        strip_yaml_frontmatter(&raw)
    }
}

fn strip_yaml_frontmatter(content: &str) -> String {
    if !content.starts_with("---\n") && !content.starts_with("---\r\n") {
        return content.to_string();
    }

    let after_open = if content.starts_with("---\r\n") { 5 } else { 4 };
    let rest = &content[after_open..];
    if let Some(close_pos) = rest.find("\n---\n") {
        let body_start = after_open + close_pos + 5;
        return content[body_start..].to_string();
    }
    if let Some(close_pos) = rest.find("\n---\r\n") {
        let body_start = after_open + close_pos + 6;
        return content[body_start..].to_string();
    }

    content.to_string()
}

/// Errors that can occur during scaffolding
#[derive(Debug)]
pub enum ScaffoldError {
    /// IO error while reading/writing files
    IoError { path: PathBuf, message: String },
}

impl std::fmt::Display for ScaffoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScaffoldError::IoError { path, message } => {
                write!(f, "IO error at {}: {}", path.display(), message)
            }
        }
    }
}

impl std::error::Error for ScaffoldError {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_store() -> RequirementsStore {
        RequirementsStore {
            name: "test-project".to_string(),
            title: "Test Project".to_string(),
            description: "A test project for scaffolding".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_default_config() {
        let config = ScaffoldConfig::default();
        assert!(config.generate_claude_md);
        assert!(config.generate_agents_md);
        assert!(config.generate_commands);
        assert!(config.generate_skills);
        assert!(config.generate_codex_skills);
        assert!(config.include_aida_req_skill);
        assert!(config.include_aida_implement_skill);
        assert!(config.include_aida_capture_skill);
        assert_eq!(config.project_type, ProjectType::Generic);
    }

    #[test]
    fn test_preview_generates_expected_artifacts() {
        let temp_dir = TempDir::new().unwrap();
        let config = ScaffoldConfig::default();
        let mut scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config);
        let store = create_test_store();

        let preview = scaffolder.preview(&store);

        // Should have CLAUDE.md, 2 commands, and 2 skills
        assert!(!preview.artifacts.is_empty());

        // Check that CLAUDE.md is generated
        let claude_md = preview
            .artifacts
            .iter()
            .find(|a| a.path == PathBuf::from("CLAUDE.md"));
        assert!(claude_md.is_some());
        assert!(claude_md.unwrap().content.contains("Test Project"));
    }

    #[test]
    fn test_apply_creates_files() {
        let temp_dir = TempDir::new().unwrap();
        let config = ScaffoldConfig::default();
        let mut scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config);
        let store = create_test_store();

        let preview = scaffolder.preview(&store);
        let result = scaffolder.apply(&preview);

        assert!(result.is_ok());

        // Check that CLAUDE.md was created
        assert!(temp_dir.path().join("CLAUDE.md").exists());
        assert!(temp_dir.path().join("AGENTS.md").exists());

        // Check that .claude directories were created
        assert!(temp_dir.path().join(".claude/commands").exists());
        assert!(temp_dir.path().join(".claude/skills").exists());
        assert!(temp_dir.path().join(".codex/skills").exists());
    }

    #[test]
    fn test_project_type_labels() {
        assert_eq!(ProjectType::Rust.label(), "Rust");
        assert_eq!(ProjectType::Python.label(), "Python");
        assert_eq!(ProjectType::Generic.label(), "Generic");
    }
}
