// trace:FR-0152,FR-0226 | ai:claude:high
//! AI Project Scaffolding Module
//!
//! Provides functionality to generate Claude Code integration artifacts:
//! - CLAUDE.md project instructions
//! - .claude/commands/ directory with project-specific slash commands
//! - .claude/skills/ directory with requirements-driven development skills
//! - .git/hooks/ directory with traceability validation hooks
//! - Code traceability configuration

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::models::RequirementsStore;

/// Configuration for what scaffolding artifacts to generate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaffoldConfig {
    /// Generate CLAUDE.md project instructions
    pub generate_claude_md: bool,
    /// Generate .claude/commands/ directory with slash commands
    pub generate_commands: bool,
    /// Generate .claude/skills/ directory with skills
    pub generate_skills: bool,
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
    /// Include aida-release skill for release management
    pub include_aida_release_skill: bool,
    /// Generate git hooks for traceability validation
    pub generate_git_hooks: bool,
    /// Include commit-msg hook for AI attribution validation
    pub include_commit_msg_hook: bool,
    /// Include pre-commit hook for trace comment validation
    pub include_pre_commit_hook: bool,
    /// Custom project type for specialized scaffolding
    pub project_type: ProjectType,
    /// Tech stack hints for context generation
    pub tech_stack: Vec<String>,
}

impl Default for ScaffoldConfig {
    fn default() -> Self {
        Self {
            generate_claude_md: true,
            generate_commands: true,
            generate_skills: true,
            include_aida_req_skill: true,
            include_aida_plan_skill: true,
            include_aida_implement_skill: true,
            include_aida_capture_skill: true,
            include_aida_docs_skill: true,
            include_aida_release_skill: true,
            generate_git_hooks: true,
            include_commit_msg_hook: true,
            include_pre_commit_hook: false, // Optional, disabled by default
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
    /// Content of the artifact
    pub content: String,
    /// Description of what this artifact does
    pub description: String,
    /// Whether the file already exists
    pub exists: bool,
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
}

/// Scaffolding generator
pub struct Scaffolder {
    /// Project root directory
    project_root: PathBuf,
    /// Scaffolding configuration
    config: ScaffoldConfig,
    /// Database path (to determine backend type)
    database_path: Option<PathBuf>,
}

impl Scaffolder {
    /// Create a new scaffolder for the given project directory
    pub fn new(project_root: PathBuf, config: ScaffoldConfig) -> Self {
        Self {
            project_root,
            config,
            database_path: None,
        }
    }

    /// Create a new scaffolder with database path for backend-aware scaffolding
    pub fn with_database(project_root: PathBuf, config: ScaffoldConfig, database_path: PathBuf) -> Self {
        Self {
            project_root,
            config,
            database_path: Some(database_path),
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

    /// Generate a preview of what would be scaffolded
    pub fn preview(&self, store: &RequirementsStore) -> ScaffoldPreview {
        let mut artifacts = Vec::new();
        let mut overwrites = Vec::new();
        let mut new_files = Vec::new();
        let mut new_dirs = HashSet::new();

        // CLAUDE.md
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
            });
        }

        // .claude/commands/ directory
        if self.config.generate_commands {
            new_dirs.insert(PathBuf::from(".claude/commands"));

            // Add default commands
            let commands = self.generate_commands(store);
            for (name, content, desc) in commands {
                let path = PathBuf::from(format!(".claude/commands/{}.md", name));
                let full_path = self.project_root.join(&path);
                let exists = full_path.exists();

                if exists {
                    overwrites.push(path.clone());
                } else {
                    new_files.push(path.clone());
                }

                artifacts.push(ScaffoldArtifact {
                    path,
                    content,
                    description: desc,
                    exists,
                });
            }
        }

        // .claude/skills/ directory
        if self.config.generate_skills {
            new_dirs.insert(PathBuf::from(".claude/skills"));

            // Add aida-req skill
            if self.config.include_aida_req_skill {
                let path = PathBuf::from(".claude/skills/aida-req.md");
                let full_path = self.project_root.join(&path);
                let exists = full_path.exists();

                if exists {
                    overwrites.push(path.clone());
                } else {
                    new_files.push(path.clone());
                }

                artifacts.push(ScaffoldArtifact {
                    path,
                    content: self.generate_aida_req_skill(store),
                    description: "Skill for adding requirements with AI evaluation".to_string(),
                    exists,
                });
            }

            // Add aida-plan skill
            if self.config.include_aida_plan_skill {
                let path = PathBuf::from(".claude/skills/aida-plan.md");
                let full_path = self.project_root.join(&path);
                let exists = full_path.exists();

                if exists {
                    overwrites.push(path.clone());
                } else {
                    new_files.push(path.clone());
                }

                artifacts.push(ScaffoldArtifact {
                    path,
                    content: self.generate_aida_plan_skill(),
                    description: "Skill for planning requirement implementation".to_string(),
                    exists,
                });
            }

            // Add aida-implement skill
            if self.config.include_aida_implement_skill {
                let path = PathBuf::from(".claude/skills/aida-implement.md");
                let full_path = self.project_root.join(&path);
                let exists = full_path.exists();

                if exists {
                    overwrites.push(path.clone());
                } else {
                    new_files.push(path.clone());
                }

                artifacts.push(ScaffoldArtifact {
                    path,
                    content: self.generate_aida_implement_skill(store),
                    description: "Skill for implementing requirements with traceability".to_string(),
                    exists,
                });
            }

            // Add aida-capture skill
            if self.config.include_aida_capture_skill {
                let path = PathBuf::from(".claude/skills/aida-capture.md");
                let full_path = self.project_root.join(&path);
                let exists = full_path.exists();

                if exists {
                    overwrites.push(path.clone());
                } else {
                    new_files.push(path.clone());
                }

                artifacts.push(ScaffoldArtifact {
                    path,
                    content: self.generate_aida_capture_skill(),
                    description: "Skill for capturing missed requirements from session".to_string(),
                    exists,
                });
            }

            // Add aida-docs skill
            if self.config.include_aida_docs_skill {
                let path = PathBuf::from(".claude/skills/aida-docs.md");
                let full_path = self.project_root.join(&path);
                let exists = full_path.exists();

                if exists {
                    overwrites.push(path.clone());
                } else {
                    new_files.push(path.clone());
                }

                artifacts.push(ScaffoldArtifact {
                    path,
                    content: self.generate_aida_docs_skill(),
                    description: "Skill for documentation management and generation".to_string(),
                    exists,
                });
            }

            // Add aida-release skill
            if self.config.include_aida_release_skill {
                let path = PathBuf::from(".claude/skills/aida-release.md");
                let full_path = self.project_root.join(&path);
                let exists = full_path.exists();

                if exists {
                    overwrites.push(path.clone());
                } else {
                    new_files.push(path.clone());
                }

                artifacts.push(ScaffoldArtifact {
                    path,
                    content: self.generate_aida_release_skill(),
                    description: "Skill for release management and version bumping".to_string(),
                    exists,
                });
            }
        }

        // .git/hooks/ directory (only if .git exists)
        if self.config.generate_git_hooks && self.project_root.join(".git").exists() {
            new_dirs.insert(PathBuf::from(".git/hooks"));

            // commit-msg hook
            if self.config.include_commit_msg_hook {
                let path = PathBuf::from(".git/hooks/commit-msg");
                let full_path = self.project_root.join(&path);
                let exists = full_path.exists();

                if exists {
                    overwrites.push(path.clone());
                } else {
                    new_files.push(path.clone());
                }

                artifacts.push(ScaffoldArtifact {
                    path,
                    content: self.generate_commit_msg_hook(),
                    description: "Git hook for validating AI attribution in commit messages".to_string(),
                    exists,
                });
            }

            // pre-commit hook
            if self.config.include_pre_commit_hook {
                let path = PathBuf::from(".git/hooks/pre-commit");
                let full_path = self.project_root.join(&path);
                let exists = full_path.exists();

                if exists {
                    overwrites.push(path.clone());
                } else {
                    new_files.push(path.clone());
                }

                artifacts.push(ScaffoldArtifact {
                    path,
                    content: self.generate_pre_commit_hook(),
                    description: "Git hook for validating trace comments before commit".to_string(),
                    exists,
                });
            }
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
        }
    }

    /// Apply the scaffolding (write files)
    pub fn apply(&self, preview: &ScaffoldPreview) -> Result<Vec<PathBuf>, ScaffoldError> {
        let mut written_files = Vec::new();

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

        // Write artifacts
        for artifact in &preview.artifacts {
            let full_path = self.project_root.join(&artifact.path);
            fs::write(&full_path, &artifact.content).map_err(|e| ScaffoldError::IoError {
                path: full_path.clone(),
                message: e.to_string(),
            })?;

            // Make git hooks executable on Unix
            #[cfg(unix)]
            if artifact.path.starts_with(".git/hooks/") {
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

    /// Generate CLAUDE.md content
    fn generate_claude_md(&self, store: &RequirementsStore) -> String {
        let project_name = if !store.title.is_empty() {
            &store.title
        } else if !store.name.is_empty() {
            &store.name
        } else {
            "Project"
        };

        let description = if !store.description.is_empty() {
            format!("\n\n{}", store.description)
        } else {
            String::new()
        };

        let tech_stack_section = if !self.config.tech_stack.is_empty() {
            format!(
                "\n\n## Tech Stack\n\n{}",
                self.config.tech_stack
                    .iter()
                    .map(|t| format!("- {}", t))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        } else {
            String::new()
        };

        let features_section = if !store.features.is_empty() {
            let features_list = store
                .features
                .iter()
                .map(|f| {
                    let prefix = if f.prefix.is_empty() { "N/A" } else { &f.prefix };
                    format!("- **{}** ({})", f.name, prefix)
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("\n\n## Features\n\n{}", features_list)
        } else {
            String::new()
        };

        let type_section = self.generate_type_specific_section();

        let traceability_section = r#"
## Code Traceability

When implementing requirements, add inline trace comments:

```rust
// trace:FR-0042 | ai:claude:high
fn implement_feature() {
    // Implementation
}
```

Format: `// trace:<SPEC-ID> | ai:<tool>:<confidence>`

Confidence levels:
- `high`: >80% AI-generated
- `med`: 40-80% AI with modifications
- `low`: <40% AI, mostly human
"#;

        let db_filename = self.database_filename();
        let db_storage_section = if self.is_sqlite_database() {
            format!(r#"Requirements database: `{}`

### Database Storage
AIDA supports both YAML and SQLite backends:
- **YAML**: Human-readable, git-friendly, good for single-user scenarios
- **SQLite**: Better for concurrent access (GUI + CLI), optimistic locking

To migrate: `aida db migrate --from yaml --to sqlite`"#, db_filename)
        } else {
            format!("Requirements database: `{}`", db_filename)
        };

        let requirements_section = format!(r#"
## Requirements Management

This project uses AIDA for requirements tracking. **Do NOT maintain a separate REQUIREMENTS.md file.**

{}

### CLI Commands
```bash
aida list                              # List all requirements
aida list --status draft               # Filter by status
aida show <ID>                         # Show requirement details (e.g., FR-0042)
aida add --title "..." --description "..." --status draft  # Add new requirement
aida edit <ID> --status completed      # Update status
aida comment add <ID> "..."            # Add implementation note
```

### During Development
- When implementing a feature, update its requirement status
- Add comments to requirements with implementation decisions
- Create child requirements for sub-tasks discovered during implementation
- Link related requirements with: `aida rel add --from <FROM> --to <TO> --type <Parent|Verifies|References>`

### Session Workflow
If you work conversationally without explicit /aida-req calls, use `/aida-capture` at session end to review and capture any requirements that were discussed but not yet added to the database.
"#, db_storage_section);

        let skills_section = r#"
## Claude Code Skills

This project uses AIDA requirements-driven development:

### /aida-req
Add new requirements with AI evaluation:
- Interactive requirement gathering
- Immediate database storage with draft status
- Background AI evaluation for quality feedback
- Follow-up actions: improve, split, link, accept

### /aida-implement
Implement requirements with traceability:
- Load and display requirement context
- Break down into child requirements as needed
- Update requirements during implementation
- Add inline traceability comments to code

### /aida-capture
Review session and capture missed requirements:
- Scan conversation for discussed features/bugs/ideas
- Identify implemented work not yet in requirements database
- Prompt to add missing requirements or update statuses
- Use at end of conversational sessions as a safety net
"#;

        format!(
            r#"# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

{}{}{}{}{}{}{}{}
"#,
            project_name,
            description,
            tech_stack_section,
            features_section,
            type_section,
            requirements_section,
            traceability_section,
            if self.config.generate_skills {
                skills_section
            } else {
                ""
            }
        )
    }

    /// Generate type-specific sections based on project type
    fn generate_type_specific_section(&self) -> String {
        match self.config.project_type {
            ProjectType::Rust => r#"
## Common Commands

```bash
cargo build --workspace --release   # Build all crates
cargo test --workspace              # Run all tests
cargo check --workspace             # Quick syntax check
cargo clippy --workspace            # Linting
```
"#
            .to_string(),

            ProjectType::Python => r#"
## Common Commands

```bash
python -m venv venv                 # Create virtual environment
source venv/bin/activate            # Activate venv (Unix)
pip install -e ".[dev]"             # Install with dev dependencies
pytest                              # Run tests
black src tests                     # Format code
ruff check src tests                # Lint code
```
"#
            .to_string(),

            ProjectType::TypeScript => r#"
## Common Commands

```bash
npm install                         # Install dependencies
npm run build                       # Build project
npm test                            # Run tests
npm run lint                        # Lint code
npm run format                      # Format code
```
"#
            .to_string(),

            ProjectType::Web => r#"
## Common Commands

```bash
npm install                         # Install dependencies
npm run dev                         # Start development server
npm run build                       # Build for production
npm test                            # Run tests
```
"#
            .to_string(),

            ProjectType::Api => r#"
## Common Commands

```bash
# Start the API server
npm run dev                         # Development mode
npm start                           # Production mode

# Testing
npm test                            # Run tests
npm run test:integration            # Integration tests
```
"#
            .to_string(),

            ProjectType::Cli => r#"
## Common Commands

```bash
cargo build --release               # Build release binary
cargo run -- --help                 # Show help
cargo test                          # Run tests
```
"#
            .to_string(),

            ProjectType::Generic => String::new(),
        }
    }

    /// Generate slash commands
    fn generate_commands(&self, store: &RequirementsStore) -> Vec<(String, String, String)> {
        let mut commands = Vec::new();

        // Add a project-specific status command
        let project_name = if !store.name.is_empty() {
            &store.name
        } else {
            "project"
        };

        let status_cmd = format!(
            r#"# Project Status

Show current project status and requirements summary.

## Instructions

1. Run `aida list --status approved` to show approved requirements
2. Run `aida list --status draft` to show draft requirements needing review
3. Summarize the current state of the project

## Output Format

```
## {} Status

### Approved Requirements (Ready for Implementation)
- [SPEC-ID] Title

### Draft Requirements (Needing Review)
- [SPEC-ID] Title

### Recently Completed
- [SPEC-ID] Title
```
"#,
            project_name
        );

        commands.push((
            "status".to_string(),
            status_cmd,
            "Show project requirements status".to_string(),
        ));

        // Add a review command
        let review_cmd = r#"# Review Requirement

Review a specific requirement for quality and completeness.

## Usage

Invoke with: `/review <SPEC-ID>`

## Instructions

1. Load the requirement: `aida show $ARGUMENTS`
2. Evaluate the requirement for:
   - Clarity: Is it unambiguous?
   - Testability: Can it be verified?
   - Completeness: Does it have all necessary information?
3. Suggest improvements if needed
4. Offer to update the requirement with suggested changes

## Output Format

```
## Review: [SPEC-ID] - [Title]

### Quality Assessment
- Clarity: X/10
- Testability: X/10
- Completeness: X/10

### Issues Found
- Issue 1
- Issue 2

### Suggested Improvements
[Improved description text]

### Actions
- [ ] Update description
- [ ] Add acceptance criteria
- [ ] Approve requirement
```
"#
        .to_string();

        commands.push((
            "review".to_string(),
            review_cmd,
            "Review a requirement for quality".to_string(),
        ));

        // Add aida-req command
        let aida_req_cmd = r#"# Add AIDA Requirement

Add a new requirement to the database with AI evaluation.

## Instructions

Follow the workflow in `.claude/skills/aida-req.md`:

1. Ask user for requirement description (required) and optional: type, priority, feature, tags
2. Generate a concise title from the description
3. Add to database with `aida add --title "..." --description "..." --status draft`
4. Run AI evaluation (clarity, testability, completeness, consistency)
5. Offer follow-up actions: improve, split, link, or accept
"#
        .to_string();

        commands.push((
            "aida-req".to_string(),
            aida_req_cmd,
            "Add a new requirement with AI evaluation".to_string(),
        ));

        // Add aida-implement command
        let aida_implement_cmd = r#"# Implement AIDA Requirement

Implement a requirement with full traceability.

## Usage

Invoke with: `/aida-implement <SPEC-ID>`

## Instructions

Follow the workflow in `.claude/skills/aida-implement.md`:

1. Load requirement: `aida show $ARGUMENTS`
2. Analyze scope and identify files to modify
3. Implement with traceability comments: `// trace:<SPEC-ID> | ai:claude:high`
4. Update requirement during implementation with `aida edit` and `aida comment add`
5. Create child requirements if needed with `aida add` and `aida rel add`
6. Mark complete: `aida edit <SPEC-ID> --status completed`
"#
        .to_string();

        commands.push((
            "aida-implement".to_string(),
            aida_implement_cmd,
            "Implement a requirement with traceability".to_string(),
        ));

        // Add aida-capture command
        let aida_capture_cmd = r#"# Capture Session Requirements

Review conversation and capture any missed requirements.

## Instructions

Follow the workflow in `.claude/skills/aida-capture.md`:

1. Scan the conversation for discussed features, bugs, or ideas
2. Identify any implemented work not yet in requirements database
3. For each finding, offer to:
   - Add as new requirement with `aida add`
   - Update existing requirement status
   - Link related requirements
4. Summarize what was captured

Use at end of conversational sessions as a safety net.
"#
        .to_string();

        commands.push((
            "aida-capture".to_string(),
            aida_capture_cmd,
            "Capture missed requirements from session".to_string(),
        ));

        commands
    }

    /// Generate aida-req skill content
    fn generate_aida_req_skill(&self, _store: &RequirementsStore) -> String {
        r#"# AIDA Requirement Creation Skill

## Purpose

Add a new requirement to the AIDA requirements database with AI-powered evaluation feedback.

## When to Use

Use this skill when:
- User wants to add a new requirement or feature request
- User describes something they want the system to do
- User has an idea that should be captured as a requirement
- User asks to "add a requirement" or "create a spec"

## Workflow

### Step 1: Gather Requirement Information

Ask the user for the following information (in conversational style):

1. **Description** (required): What should the system do? This can be:
   - A formal requirement: "The system shall..."
   - A question or idea to be formalized
   - A rough note that needs refinement

2. **Type** (optional, default: Functional):
   - Functional (FR) - System behaviors
   - NonFunctional (NFR) - Quality attributes (performance, security)
   - User (UR) - User needs/goals
   - System (SR) - Technical constraints
   - ChangeRequest (CR) - Modifications to existing features

3. **Priority** (optional, default: Medium):
   - High, Medium, Low

4. **Feature** (optional): Which feature area does this belong to?

5. **Tags** (optional): Comma-separated keywords

### Step 2: Add Requirement to Database

Use the `aida` CLI to add the requirement immediately:

```bash
aida add \
  --title "<generated-title>" \
  --description "<user-description>" \
  --type <type> \
  --priority <priority> \
  --status draft \
  --feature "<feature>" \
  --tags "<tags>"
```

**Title Generation**: Generate a concise title (5-10 words) from the description that captures the essence of the requirement.

### Step 3: Show Confirmation

After adding, display:
```
Requirement added: <SPEC-ID>
Title: <title>
Status: Draft (evaluation pending...)
```

### Step 4: Run AI Evaluation

Evaluate the requirement quality using the AI evaluation prompt. The evaluation should assess:

1. **Clarity** (1-10): Is the requirement clear and unambiguous?
2. **Testability** (1-10): Can this requirement be verified?
3. **Completeness** (1-10): Does it include all necessary information?
4. **Consistency** (1-10): Does it conflict with other requirements?

Provide:
- Overall quality score
- Issues found (if any)
- Suggestions for improvement
- Whether this should be split into multiple requirements

### Step 5: Offer Follow-up Actions

Based on the evaluation, offer:
- **Improve**: Let AI suggest improved description text
- **Split**: Generate child requirements if too broad
- **Link**: Suggest relationships to existing requirements
- **Accept**: Keep as-is and approve

## CLI Reference

```bash
# Add requirement
aida add --title "..." --description "..." --type functional --priority high --status draft

# Show requirement details
aida show <SPEC-ID>

# Edit requirement
aida edit <SPEC-ID> --description "..."

# List features
aida feature list
```

## Integration Notes

- Requirements are stored in `requirements.yaml` or the configured project database
- SPEC-IDs are auto-generated based on type prefix configuration
- The GUI (aida-gui) can be used to view and manage requirements with full AI features
"#
        .to_string()
    }

    /// Generate aida-implement skill content
    fn generate_aida_implement_skill(&self, _store: &RequirementsStore) -> String {
        let comment_examples = match self.config.project_type {
            ProjectType::Rust | ProjectType::Cli => {
                r#"**Rust:**
```rust
// trace:FR-0042 - Keyboard navigation | ai:claude:high | impl:2025-12-10 | by:joe
fn implement_feature() {
    // Implementation here
}
```"#
            }
            ProjectType::Python => {
                r#"**Python:**
```python
# trace:FR-0042 - Keyboard navigation | ai:claude:high | impl:2025-12-10 | by:joe
def implement_feature():
    """Implementation of FR-0042."""
    pass
```"#
            }
            ProjectType::TypeScript | ProjectType::Web | ProjectType::Api => {
                r#"**TypeScript/JavaScript:**
```typescript
// trace:FR-0042 - Keyboard navigation | ai:claude:high | impl:2025-12-10 | by:joe
function implementFeature() {
    // Implementation here
}
```"#
            }
            ProjectType::Generic => {
                r#"**Generic (use language-appropriate comment syntax):**
```
// trace:FR-0042 - Feature title | ai:claude:high | impl:2025-12-10 | by:joe
// Your implementation here
```"#
            }
        };

        format!(
            r#"# AIDA Implementation Skill

## Purpose

Implement an approved requirement with full traceability, evolving the requirement database to capture implementation details and creating child requirements as needed.

## When to Use

Use this skill when:
- User says "implement <SPEC-ID>" or "work on <requirement>"
- User triggers "Copy for Claude Code" from the aida-gui AI menu
- An approved requirement is ready to be implemented
- Continuing implementation of a requirement from a previous session

## Core Principles

### Living Documentation
The requirements database should evolve during implementation to accurately reflect:
- What was actually built (vs. what was initially specified)
- Implementation decisions and trade-offs
- Child requirements discovered during development
- Technical constraints encountered

### Traceability
All AI-generated code must include inline traceability comments linking back to requirement IDs.

## Workflow

### Step 1: Load Requirement Context

Fetch the requirement details:

```bash
aida show <SPEC-ID>
```

Display to user:
- SPEC-ID and title
- Current description
- Status, priority, type
- Related requirements (parent/child, links)
- Any existing implementation notes

### Step 2: Analyze Implementation Scope

Before writing code:
1. Identify files that will be created or modified
2. Identify any sub-tasks or child requirements
3. Confirm approach with user if there are significant decisions

If the requirement is too broad, suggest splitting:
```bash
# Create child requirements
aida add --title "..." --description "..." --type functional --status draft

# Link as child
aida rel add --from <PARENT-ID> --to <CHILD-ID> --type Parent
```

### Step 3: Implement with Traceability

When writing or modifying code, add inline traceability comments:

{}

**Comment Format:**
```
// trace:<SPEC-ID> - <title> | ai:<tool>:<confidence> | impl:<date> | by:<user>
```

Where:
- `<SPEC-ID>`: The requirement being implemented (e.g., FR-0042)
- `<title>`: Brief requirement title (truncate if >40 chars)
- `<tool>`: AI tool used (e.g., `claude`)
- `<confidence>`: `high` (>80% AI), `med` (40-80%), `low` (<40%)
- `<date>`: Implementation date (YYYY-MM-DD)
- `<user>`: Who implemented it

### Step 4: Update Requirement During Implementation

As you implement, update the requirement to reflect reality:

```bash
# Update description with implementation details
aida edit <SPEC-ID> --description "Updated description with implementation notes..."

# Add implementation notes to history
aida comment add <SPEC-ID> "Implementation note: Used async/await pattern for..."

# Update status as appropriate
aida edit <SPEC-ID> --status completed
```

### Step 5: Create Child Requirements

When implementation reveals sub-tasks:

```bash
# Add child requirement
aida add \
  --title "Handle edge case: empty input" \
  --description "The system shall handle empty input gracefully..." \
  --type functional \
  --status draft

# Link to parent
aida rel add --from <PARENT-ID> --to <NEW-CHILD-ID> --type Parent
```

### Step 6: Document Completion

When implementation is complete:

1. Update requirement status:
```bash
aida edit <SPEC-ID> --status completed
```

2. Add completion comment:
```bash
aida comment add <SPEC-ID> "Implementation complete. Files modified: src/foo.rs, src/bar.rs"
```

3. Create "Verifies" relationship if tests were added:
```bash
aida rel add --from <TEST-SPEC-ID> --to <SPEC-ID> --type Verifies
```

## State Transitions

During implementation, requirements should transition through:

1. **Approved** -> **In Progress** (when starting implementation)
2. **In Progress** -> **Completed** (when implementation is verified)
3. **In Progress** -> **Draft** (if significant changes needed)

Update via:
```bash
aida edit <SPEC-ID> --status <new-status>
```

## CLI Reference

```bash
# Show requirement
aida show <SPEC-ID>

# Edit requirement
aida edit <SPEC-ID> --description "..." --status <status>

# Add comment
aida comment add <SPEC-ID> "Comment text"

# Add relationship
aida rel add --from <FROM-ID> --to <TO-ID> --type <Parent|Verifies|References|Duplicate>

# Create new requirement
aida add --title "..." --description "..." --type <type> --status draft

# List requirements by feature
aida list --feature <feature-name>
```
"#,
            comment_examples
        )
    }

    /// Generate aida-plan skill content
    fn generate_aida_plan_skill(&self) -> String {
        r#"# AIDA Planning Skill

## Purpose

Plan the implementation of an approved requirement before coding begins. This ensures implementation is well-thought-out, decomposed into manageable pieces, and the plan is recorded in the requirements database.

## When to Use

Use this skill when:
- User says "plan <SPEC-ID>" or "plan implementation of <requirement>"
- A requirement is in `Approved` status and needs planning before implementation
- `/aida-implement` is invoked on a requirement that hasn't been planned yet
- User wants to decompose a large requirement into child requirements

## Core Principles

### Think Before Code
Planning separates design decisions from implementation. This allows for:
- Reviewing approach before committing effort
- Identifying risks and unknowns early
- Creating a clear implementation roadmap
- Breaking down complex work into manageable pieces

### Documented Plans
All planning decisions should be captured in the requirements database as:
- Child requirements for sub-tasks
- Comments for design decisions and trade-offs
- Status transition to `Planned` when complete

## Workflow

### Step 1: Load Requirement Context

Fetch the requirement details:

```bash
aida show <SPEC-ID>
```

Display to user:
- SPEC-ID and title
- Current description
- Status, priority, type
- Related requirements (parent/child, links)
- Any existing comments

Verify the requirement is in `Approved` status. If not, inform the user:
- `Draft`: Needs approval first
- `Planned`: Already planned, proceed to `/aida-implement`
- `In Progress` or `Completed`: Already being/been implemented

### Step 2: Analyze Scope

Examine the requirement to understand:
1. What files will need to be created or modified?
2. What external dependencies are involved?
3. Are there any architectural decisions to make?
4. What are the edge cases and error scenarios?
5. Are there any unknowns or risks?

For each significant unknown, note it as a question to resolve during planning.

### Step 3: Decompose into Child Requirements

If the requirement is complex, break it into child requirements:

```bash
# Create child requirement for each logical unit of work
aida add \
  --title "Component: User input validation" \
  --description "Validate user input for..." \
  --type task \
  --status draft

# Link as child
aida rel add --from <PARENT-ID> --to <CHILD-ID> --type Parent
```

Guidelines for decomposition:
- Each child should be implementable in a focused session
- Children should have clear boundaries
- Avoid too many children (3-7 is usually good)
- Order children by implementation dependency

### Step 4: Document Design Decisions

Record any significant design decisions:

```bash
aida comment add <SPEC-ID> "Design: Using async/await pattern because..."
aida comment add <SPEC-ID> "Decision: Chose HashMap over BTreeMap for O(1) lookup"
aida comment add <SPEC-ID> "Risk: External API rate limiting may need handling"
```

### Step 5: Identify File Changes

List the files that will be modified or created:

```bash
aida comment add <SPEC-ID> "Files to modify:
- src/models.rs: Add new struct
- src/handlers.rs: Add endpoint
- src/tests/mod.rs: Add unit tests"
```

### Step 6: Mark as Planned

When planning is complete:

```bash
aida edit <SPEC-ID> --status planned
aida comment add <SPEC-ID> "Planning complete. Ready for implementation."
```

If child requirements were created, approve them:

```bash
aida edit <CHILD-ID> --status approved
```

### Step 7: Present Plan to User

Summarize for the user:
1. Overview of implementation approach
2. List of child requirements created
3. Key design decisions made
4. Files that will be affected
5. Any risks or unknowns identified

Ask if they want to proceed to implementation with `/aida-implement`.

## Status Transitions

During planning, requirements transition:

1. **Approved** -> **Planned** (when planning is complete)

Child requirements created during planning start as:
- **Draft** -> **Approved** (when ready for implementation)

## Integration with /aida-implement

When `/aida-implement` is invoked on a requirement:
1. Check the status
2. If `Approved` (not `Planned`), suggest running `/aida-plan` first
3. If `Planned`, proceed with implementation

## CLI Reference

```bash
# Show requirement
aida show <SPEC-ID>

# Check status
aida show <SPEC-ID> | grep Status

# Create child requirement
aida add --title "..." --description "..." --type task --status draft

# Link child to parent
aida rel add --from <PARENT-ID> --to <CHILD-ID> --type Parent

# Add design comment
aida comment add <SPEC-ID> "Design: ..."

# Mark as planned
aida edit <SPEC-ID> --status planned

# Approve child requirements
aida edit <CHILD-ID> --status approved

# List children of a requirement
aida show <SPEC-ID>  # Shows relationships section
```
"#
        .to_string()
    }

    /// Generate aida-capture skill content
    fn generate_aida_capture_skill(&self) -> String {
        r#"# AIDA Session Capture Skill

## Purpose

Review the current conversation and capture any requirements, features, or implementation details that were discussed but not yet added to the requirements database.

## When to Use

Use this skill when:
- User says "capture requirements" or "review session"
- At the end of a conversational coding session
- User asks to update requirements based on what was discussed
- After implementing features without explicitly creating requirements

## Workflow

### Step 1: Scan Conversation

Review the conversation history for:
- Features that were discussed or requested
- Bugs that were identified or fixed
- Implementation decisions that were made
- Ideas or future enhancements mentioned
- Any work that was completed

### Step 2: Check Against Database

For each finding, check if it already exists:

```bash
aida list --search "<keyword>"
```

### Step 3: Present Findings

Present a summary to the user:
```
## Session Review

### Implemented (not in database)
- [Description of implemented work]

### Discussed (not captured)
- [Description of discussed feature/idea]

### Existing Requirements Updated
- [SPEC-ID] - Status changed / notes added
```

### Step 4: Offer Actions

For each finding, offer to:
1. **Add as new requirement**: Create with appropriate type and status
2. **Update existing**: Add comments or change status
3. **Skip**: Don't capture this item

### Step 5: Execute Updates

For new requirements:
```bash
aida add --title "..." --description "..." --type functional --status completed
```

For existing requirements:
```bash
aida comment add <SPEC-ID> "Session note: ..."
aida edit <SPEC-ID> --status completed
```

## CLI Reference

```bash
# Search for existing requirements
aida list --search "<keyword>"

# Add new requirement
aida add --title "..." --description "..." --status <status>

# Update requirement
aida edit <SPEC-ID> --status <status>

# Add comment
aida comment add <SPEC-ID> "Comment text"
```

## Best Practices

- Use status `completed` for work that was already implemented
- Use status `draft` for ideas that need refinement
- Link related requirements that were discovered during the session
- Add implementation comments with file paths that were modified
"#
        .to_string()
    }

    /// Generate aida-docs skill content
    fn generate_aida_docs_skill(&self) -> String {
        r#"# AIDA Documentation Skill

## Purpose

Manage project documentation including markdown guides, HTML generation, slideshow updates, and report generation. Keep documentation in sync with the codebase and requirements.

## When to Use

Use this skill when:
- User says "update docs", "regenerate documentation", or "sync docs"
- User asks to update the slideshow or add screenshots
- User requests a requirements report or status report
- Documentation needs updating after significant feature changes
- User wants to generate HTML versions of guides

## Documentation Structure

Typical project documentation:

```
docs/
├── user-guide.md          # End-user documentation
├── user-guide.html        # Generated HTML version
├── admin-guide.md         # Administration and configuration
├── admin-guide.html       # Generated HTML version
├── DEVELOPER_GUIDE.md     # Developer/contributor guide
├── DEVELOPER_GUIDE.html   # Generated HTML version
├── slideshow.html         # Feature showcase presentation
└── images/                # Screenshots and diagrams
```

## Workflows

### 1. Update Markdown Documentation

When features change, update the relevant guide:
- **user-guide.md**: UI features, keyboard shortcuts, views
- **admin-guide.md**: Configuration, settings, multi-project setup
- **DEVELOPER_GUIDE.md**: Architecture, code patterns, contributing

**Guidelines:**
- Keep sections numbered and in logical order
- Update Table of Contents when adding sections
- Use consistent formatting (headers, code blocks, lists)
- Add cross-references between related sections

### 2. Generate HTML Versions

After updating markdown, regenerate HTML with navigation and theming:

```bash
# Generate HTML with pandoc (example)
pandoc user-guide.md -o user-guide.html --standalone \
  --metadata title="User Guide" \
  -H styles.html
```

### 3. Update Slideshow

For slideshow presentations:
- Add slides following existing HTML pattern
- Update slide count in header
- Add screenshots to `docs/images/` with naming convention `ss-<feature>.png`

### 4. Generate Requirements Report

```bash
# Basic status report
aida list --format markdown > docs/reports/requirements-status.md

# Filter by status
aida list --status draft
aida list --status approved
aida list --status completed

# By priority
aida list --priority critical
aida list --priority high
```

### 5. Sync Documentation with Code

After significant code changes:
1. Check git log for recent changes
2. Identify documentation gaps
3. Update relevant markdown files
4. Regenerate HTML versions
5. Commit all changes together

## CLI Reference

```bash
# List requirements for documentation
aida list
aida list --status <status>
aida list --type <type>

# Show requirement details
aida show <SPEC-ID>

# Export requirements
aida export --format markdown
aida export --format json
```

## Best Practices

1. **Keep docs in sync** - Update docs in the same commit as code changes
2. **Use consistent formatting** - Follow existing patterns in each guide
3. **Include examples** - Show concrete usage examples
4. **Cross-reference** - Link between guides when topics overlap
5. **Commit together** - Commit markdown + HTML + screenshots as a unit
"#
        .to_string()
    }

    /// Generate aida-release skill content
    fn generate_aida_release_skill(&self) -> String {
        r#"# AIDA Release Management Skill

## Purpose

Manage software releases with version bumping, release notes generation, changelog maintenance, and git tagging - all integrated with the AIDA requirements database.

## When to Use

Use this skill when:
- User wants to prepare a new release
- User asks to "bump the version" or "create a release"
- User wants to generate release notes
- User wants to tag a release in git
- User asks "what's changed since last release?"

## Workflow

### Step 1: Gather Release Context

Collect information about the current state:

```bash
# Get the last release tag
git describe --tags --abbrev=0 2>/dev/null || echo "No previous tags"

# Get current version from Cargo.toml (Rust) or package.json (Node)
grep '^version' Cargo.toml 2>/dev/null || cat package.json 2>/dev/null | grep '"version"'

# Check git status
git status --porcelain

# Check current branch
git branch --show-current
```

### Step 2: Pre-Release Validation

Run validation checks before proceeding:

1. **Git Status Check**
   ```bash
   # Must have clean working directory
   git status --porcelain
   ```
   - If files are modified, warn user and ask if they want to commit first

2. **Branch Check**
   ```bash
   git branch --show-current
   ```
   - Warn if not on main/master branch

3. **Build Check** (optional)
   ```bash
   cargo build --release 2>&1  # For Rust
   # or
   npm run build 2>&1  # For Node
   ```

4. **Test Check** (optional)
   ```bash
   cargo test 2>&1  # For Rust
   # or
   npm test 2>&1  # For Node
   ```

Display validation results:
```
Pre-Release Validation:
  [PASS] Git working directory is clean
  [PASS] On branch: main
  [PASS] Tests passing
  [PASS] Build successful
```

### Step 3: Determine Version Bump

Ask user for bump type:
```
Current version: 0.5.2
Last release tag: v0.5.2

What type of version bump?
1. patch (0.5.2 -> 0.5.3) - Bug fixes only
2. minor (0.5.2 -> 0.6.0) - New features, backwards compatible
3. major (0.5.2 -> 1.0.0) - Breaking changes
4. custom - Specify version manually
```

### Step 4: Gather Changes Since Last Release

Query requirements completed since last release:

```bash
# Get date of last tag
LAST_TAG=$(git describe --tags --abbrev=0 2>/dev/null)
LAST_TAG_DATE=$(git log -1 --format=%ai $LAST_TAG 2>/dev/null)

# List requirements (filter by modified date in your analysis)
aida list --status completed
```

Also gather git commits:
```bash
# Commits since last tag
git log ${LAST_TAG}..HEAD --oneline
```

### Step 5: Generate Release Notes

Create release notes from:
1. Completed requirements (grouped by type)
2. Git commit messages (for additional context)

Format:
```markdown
## Release v{version} - {date}

### Features
- FR-0123: User authentication system
- FR-0124: Dashboard analytics

### Bug Fixes
- BUG-0045: Fixed login timeout issue
- BUG-0046: Corrected date formatting

### Changes
- CR-0012: Updated API response format

### Implementation Tasks
- IMPL-0089: Refactored database layer
- IMPL-0090: Added caching support

### Statistics
- X features added
- Y bugs fixed
- Z commits since last release
```

### Step 6: Update CHANGELOG.md

If CHANGELOG.md exists, update it:

```bash
# Read existing changelog
cat CHANGELOG.md
```

Insert new version section after `## [Unreleased]`:
```markdown
## [Unreleased]

## [{version}] - {YYYY-MM-DD}

### Added
- FR-0123: User authentication system

### Fixed
- BUG-0045: Fixed login timeout issue

### Changed
- CR-0012: Updated API response format
```

If no CHANGELOG.md exists, offer to create one following Keep a Changelog format.

### Step 7: Update Version Files

Update version in project files:

**Cargo.toml (Rust):**
```bash
# Update version line
sed -i 's/^version = ".*"/version = "{new_version}"/' Cargo.toml
```

**package.json (Node):**
```bash
npm version {new_version} --no-git-tag-version
```

### Step 8: Commit and Tag

```bash
# Stage changes
git add Cargo.toml CHANGELOG.md  # or package.json

# Commit version bump
git commit -m "chore: release v{version}

Release notes:
- X features added
- Y bugs fixed

See CHANGELOG.md for details."

# Create annotated tag
git tag -a v{version} -m "Release v{version}

{release_notes_summary}"
```

### Step 9: Offer Push

Ask user if they want to push:
```
Release v{version} created locally.

Push to remote?
1. Push commits and tags: git push && git push --tags
2. Push commits only: git push
3. Don't push (I'll do it manually)
```

## Example Session

```
User: Let's do a release

Claude: I'll help you prepare a release. Let me check the current state...

**Current State:**
- Version: 0.5.2 (from Cargo.toml)
- Last tag: v0.5.2 (2025-11-28)
- Branch: main
- Working directory: Clean

**Pre-Release Validation:**
  [PASS] Git working directory is clean
  [PASS] On branch: main

**Changes since v0.5.2:**

Requirements completed (5):
- FR-0170: Smart parent selection for new requirements
- FR-0172: CLI add command --parent option
- FR-0146: Folder type for organizational containers
- BUG-0089: Fixed duplicate SPEC-ID issue

Commits: 12 commits since last tag

What type of version bump?
1. patch (0.5.3) - Bug fixes only
2. minor (0.6.0) - New features
3. major (1.0.0) - Breaking changes

User: minor please

Claude: Preparing release v0.6.0...

**Generated Release Notes:**

## Release v0.6.0 - 2025-12-02

### Features
- FR-0170: Smart parent selection for new requirements
- FR-0172: CLI add command --parent option
- FR-0146: Folder type for organizational containers

### Bug Fixes
- BUG-0089: Fixed duplicate SPEC-ID issue

### Statistics
- 3 features added
- 1 bug fixed
- 12 commits

Does this look correct? (yes/edit/cancel)

User: yes

Claude:
Updated Cargo.toml version to 0.6.0
Updated CHANGELOG.md with new release section
Created commit: "chore: release v0.6.0"
Created tag: v0.6.0

Push to remote? (yes/no)

User: yes

Claude:
Pushed commits to origin/main
Pushed tags to origin

Release v0.6.0 complete!
```

## CLI Reference

```bash
# List completed requirements
aida list --status completed

# Show requirement details
aida show <SPEC-ID>

# Git operations
git describe --tags --abbrev=0     # Last tag
git log v0.5.2..HEAD --oneline     # Commits since tag
git tag -a v0.6.0 -m "message"     # Create annotated tag
git push --tags                     # Push tags
```

## Integration Notes

- Uses AIDA requirements database for change tracking
- Respects semantic versioning (semver.org)
- Follows Keep a Changelog format (keepachangelog.com)
- Creates annotated git tags with release summary
- Updates version in Cargo.toml or package.json
"#
        .to_string()
    }

    /// Generate commit-msg git hook content
    fn generate_commit_msg_hook(&self) -> String {
        r#"#!/bin/bash
# AIDA commit-msg hook - Validates AI attribution in commit messages
# Generated by AIDA scaffolding
# trace:FR-0226 | ai:claude:high

COMMIT_MSG_FILE="$1"
COMMIT_MSG=$(cat "$COMMIT_MSG_FILE")

# Colors for output
RED='\033[0;31m'
YELLOW='\033[1;33m'
GREEN='\033[0;32m'
NC='\033[0m' # No Color

# Check if commit message contains AI attribution tag [AI:tool:conf]
AI_TAG_PATTERN='\[AI:[a-zA-Z]+:(high|med|low)\]'

# Check for files with trace comments in this commit
STAGED_FILES=$(git diff --cached --name-only)
HAS_TRACE_FILES=false

for file in $STAGED_FILES; do
    if [ -f "$file" ]; then
        if grep -q "trace:[A-Z]*-[0-9]*" "$file" 2>/dev/null; then
            HAS_TRACE_FILES=true
            break
        fi
    fi
done

# If staged files have trace comments but commit lacks AI tag, warn
if [ "$HAS_TRACE_FILES" = true ]; then
    if ! echo "$COMMIT_MSG" | grep -qE "$AI_TAG_PATTERN"; then
        echo -e "${YELLOW}⚠ Warning: Commit includes files with AI trace comments but no [AI:tool:conf] tag in commit message.${NC}"
        echo -e "${YELLOW}  Consider using format: [AI:claude:high] feat: description${NC}"
        echo -e "${YELLOW}  Confidence levels: high (>80% AI), med (40-80%), low (<40%)${NC}"
        echo ""
    fi
fi

# Check for Trace: references in commit message matching staged file traces
if [ "$HAS_TRACE_FILES" = true ]; then
    # Extract spec IDs from staged files
    SPEC_IDS=""
    for file in $STAGED_FILES; do
        if [ -f "$file" ]; then
            FILE_SPECS=$(grep -oE "trace:[A-Z]+-[0-9]+" "$file" 2>/dev/null | sed 's/trace://' | sort -u)
            if [ -n "$FILE_SPECS" ]; then
                SPEC_IDS="$SPEC_IDS $FILE_SPECS"
            fi
        fi
    done
    SPEC_IDS=$(echo "$SPEC_IDS" | tr ' ' '\n' | sort -u | tr '\n' ' ')

    if [ -n "$SPEC_IDS" ]; then
        # Check if commit message references these specs
        MISSING_REFS=""
        for spec in $SPEC_IDS; do
            if ! echo "$COMMIT_MSG" | grep -q "$spec"; then
                MISSING_REFS="$MISSING_REFS $spec"
            fi
        done

        if [ -n "$MISSING_REFS" ]; then
            echo -e "${YELLOW}⚠ Suggestion: Consider adding 'Trace:$MISSING_REFS' to your commit message${NC}"
            echo -e "${YELLOW}  These requirements are referenced in the staged files.${NC}"
            echo ""
        fi
    fi
fi

# Always allow the commit (non-blocking)
exit 0
"#
        .to_string()
    }

    /// Generate pre-commit git hook content
    fn generate_pre_commit_hook(&self) -> String {
        r#"#!/bin/bash
# AIDA pre-commit hook - Validates trace comments
# Generated by AIDA scaffolding
# trace:FR-0226 | ai:claude:high

# Colors for output
RED='\033[0;31m'
YELLOW='\033[1;33m'
GREEN='\033[0;32m'
NC='\033[0m' # No Color

# Get staged files
STAGED_FILES=$(git diff --cached --name-only --diff-filter=ACM)

# Track warnings
WARNINGS=0

echo -e "${GREEN}AIDA: Checking trace comments...${NC}"

for file in $STAGED_FILES; do
    # Skip non-source files
    case "$file" in
        *.rs|*.py|*.js|*.ts|*.tsx|*.jsx|*.go|*.java|*.c|*.cpp|*.h|*.hpp)
            ;;
        *)
            continue
            ;;
    esac

    if [ -f "$file" ]; then
        # Extract spec IDs from trace comments
        TRACES=$(grep -oE "trace:[A-Z]+-[0-9]+" "$file" 2>/dev/null | sed 's/trace://' | sort -u)

        for spec_id in $TRACES; do
            # Check if the spec ID exists in the requirements database
            if command -v aida &> /dev/null; then
                if ! aida show "$spec_id" &> /dev/null; then
                    echo -e "${YELLOW}⚠ Warning: $file references unknown spec ID: $spec_id${NC}"
                    WARNINGS=$((WARNINGS + 1))
                fi
            fi
        done
    fi
done

if [ $WARNINGS -gt 0 ]; then
    echo -e "${YELLOW}Found $WARNINGS warning(s). Commit will proceed.${NC}"
fi

# Always allow the commit (non-blocking by default)
# To make blocking, change exit 0 to exit 1 when WARNINGS > 0
exit 0
"#
        .to_string()
    }
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
        assert!(config.generate_commands);
        assert!(config.generate_skills);
        assert!(config.include_aida_req_skill);
        assert!(config.include_aida_implement_skill);
        assert!(config.include_aida_capture_skill);
        assert_eq!(config.project_type, ProjectType::Generic);
    }

    #[test]
    fn test_preview_generates_expected_artifacts() {
        let temp_dir = TempDir::new().unwrap();
        let config = ScaffoldConfig::default();
        let scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config);
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
        let scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config);
        let store = create_test_store();

        let preview = scaffolder.preview(&store);
        let result = scaffolder.apply(&preview);

        assert!(result.is_ok());

        // Check that CLAUDE.md was created
        assert!(temp_dir.path().join("CLAUDE.md").exists());

        // Check that .claude directories were created
        assert!(temp_dir.path().join(".claude/commands").exists());
        assert!(temp_dir.path().join(".claude/skills").exists());
    }

    #[test]
    fn test_project_type_labels() {
        assert_eq!(ProjectType::Rust.label(), "Rust");
        assert_eq!(ProjectType::Python.label(), "Python");
        assert_eq!(ProjectType::Generic.label(), "Generic");
    }
}
