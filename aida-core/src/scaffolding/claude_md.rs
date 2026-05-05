use super::*;

impl Scaffolder {
    /// Generate the project-local CLAUDE.md stub. After FR-1-035 this is a
    /// thin seed-class file: project intro + literal `@.claude/AIDA.md`
    /// import line. Claude Code expands the `@` import at runtime, so the
    /// model sees the full AIDA conventions without us duplicating ~80
    /// lines of content here. After init the file is user-owned —
    /// `scaffold status` won't fight you when you tailor it.
    /// trace:FR-1-035 | ai:claude
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
                "\n\n## Tech stack\n\n{}",
                self.config
                    .tech_stack
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
                    let prefix = if f.prefix.is_empty() {
                        "N/A"
                    } else {
                        &f.prefix
                    };
                    format!("- **{}** ({})", f.name, prefix)
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("\n\n## Features\n\n{}", features_list)
        } else {
            String::new()
        };

        let type_section = self.generate_type_specific_section();

        format!(
            r#"# CLAUDE.md

Guidance for Claude Code working in this repository. AIDA conventions
(trace format, commit format, daily commands, capture rules) live in
`.claude/AIDA.md` — Claude Code expands the import below automatically,
so you'll see them in context without this file having to duplicate
them.

@.claude/AIDA.md

## Project overview

{project_name}{description}{tech_stack}{features}{type_section}
"#,
            project_name = project_name,
            description = description,
            tech_stack = tech_stack_section,
            features = features_section,
            type_section = type_section,
        )
    }

    /// Generate type-specific sections based on project type
    fn generate_type_specific_section(&self) -> String {
        match self.config.project_type {
            ProjectType::Rust => r#"

## Common commands

```bash
cargo build --workspace --release   # Build all crates
cargo test --workspace              # Run all tests
cargo check --workspace             # Quick syntax check
cargo clippy --workspace            # Linting
```
"#
            .to_string(),

            ProjectType::Python => r#"

## Common commands

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

## Common commands

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

## Common commands

```bash
npm install                         # Install dependencies
npm run dev                         # Start development server
npm run build                       # Build for production
npm test                            # Run tests
```
"#
            .to_string(),

            ProjectType::Api => r#"

## Common commands

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

## Common commands

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
