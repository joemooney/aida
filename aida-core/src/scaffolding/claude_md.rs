use super::*;

impl Scaffolder {
    /// Generate CLAUDE.md content
    pub(super) fn generate_claude_md(&self, store: &RequirementsStore) -> String {
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

        // trace:TASK-0344 | ai:claude
        let traceability_section = r#"
## Code Traceability

### Inline Trace Comments
When implementing requirements, add inline trace comments:

```rust
// trace:FR-0042 | ai:claude
fn implement_feature() {
    // Implementation
}
```

Format: `// trace:<SPEC-ID> | ai:<tool>[:<confidence>]`

### Commit Message Format
**Standard format:**
```
[AI:tool] type(scope): description (REQ-ID)
```

**Examples:**
```
[AI:claude] feat(auth): add login validation (FR-0042)
[AI:claude:med] fix(api): handle null response (BUG-0023)
chore(deps): update dependencies
docs: update README
```

**Rules:**
- `[AI:tool]` - Required when commit includes AI-assisted code
- `type` - Required: feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert
- `(scope)` - Optional: component or area affected
- `(REQ-ID)` - Required for feat/fix commits, optional for chore/docs

**Confidence levels:**
- `[AI:claude]` - High confidence (implied, >80% AI-generated)
- `[AI:claude:med]` - Medium (40-80% AI with modifications)
- `[AI:claude:low]` - Low (<40% AI, mostly human)

**Configuration:**
Set `AIDA_COMMIT_STRICT=true` to reject non-conforming commits, or create `.aida/commit-config`.
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

### Core Skills
- `/aida-req` — Add new requirements with AI evaluation and quality feedback
- `/aida-implement` — Implement requirements with code traceability and status tracking
- `/aida-plan` — Plan requirement implementation: decompose, document decisions, identify files
- `/aida-evaluate` — Evaluate requirement quality (clarity, testability, completeness)
- `/aida-capture` — Review session and capture missed requirements (use at end of sessions)

### Development Workflow
- `/aida-commit` — Commit with automatic requirement linking and status updates
- `/aida-review` — Review code changes against requirement specs, identify gaps
- `/aida-test` — Generate tests linked to requirements with `Verifies` relationships
- `/aida-search` — Unified search across requirements database and code

### Project Management
- `/aida-sprint` — Sprint planning: select approved requirements, group by feature
- `/aida-standup` — Daily standup summary from recent commits and requirement changes
- `/aida-onboard` — Project onboarding: architecture overview, requirements status, first tasks

### Maintenance
- `/aida-release` — Release management: version bump, changelog, requirement export
- `/aida-docs` — Documentation generation and management
- `/aida-sync` — Template synchronization and scaffold status checking
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
}
