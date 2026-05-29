use super::*;

/// The literal line CLAUDE.md must contain so Claude Code pulls in the AIDA
/// conventions block at session start. AIDA's only managed surface in
/// CLAUDE.md.
/// trace:BUG-1-065 | ai:claude
pub const CLAUDE_AIDA_IMPORT: &str = "@.claude/AIDA.md";

/// True when the on-disk CLAUDE.md already includes the AIDA conventions
/// import line. Presence-only check — the line can sit anywhere.
pub fn claude_md_has_import(content: &str) -> bool {
    content.contains(CLAUDE_AIDA_IMPORT)
}

/// Insert the `@.claude/AIDA.md` import line into an existing CLAUDE.md,
/// preserving everything the user already wrote. Insertion strategy:
/// place the import right before the first `## ` heading (the natural slot
/// in the canonical layout). Falls back to appending when no `## ` heading
/// is found.
/// trace:BUG-1-065 | ai:claude
pub fn insert_claude_md_import(actual: &str) -> String {
    if claude_md_has_import(actual) {
        return actual.to_string();
    }
    let block = format!("{}\n\n", CLAUDE_AIDA_IMPORT);

    if let Some(idx) = actual.find("\n## ") {
        // Insert just after the preceding newline → before the `## ` line.
        let (head, tail) = actual.split_at(idx + 1);
        return format!("{}{}{}", head, block, tail);
    }
    // No level-2 heading: append at the end with a leading blank line.
    let needs_sep = !actual.is_empty() && !actual.ends_with('\n');
    let sep = if needs_sep { "\n\n" } else { "\n" };
    format!("{}{}{}", actual, sep, block.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_into_canonical_layout_lands_before_first_h2() {
        // trace:BUG-1-065 | ai:claude
        let original = "# CLAUDE.md\n\nIntro paragraph.\n\n## Project overview\n\nbody\n";
        let updated = insert_claude_md_import(original);
        assert!(updated.contains(CLAUDE_AIDA_IMPORT));
        // The import should come before "## Project overview".
        let idx_import = updated.find(CLAUDE_AIDA_IMPORT).unwrap();
        let idx_h2 = updated.find("## Project overview").unwrap();
        assert!(idx_import < idx_h2);
    }

    #[test]
    fn insert_is_idempotent_when_already_present() {
        let original = "# CLAUDE.md\n\n@.claude/AIDA.md\n\n## Body\n";
        let updated = insert_claude_md_import(original);
        assert_eq!(updated, original);
    }

    #[test]
    fn insert_appends_when_no_h2_present() {
        let original = "# CLAUDE.md\n\nNo subsections, just prose.\n";
        let updated = insert_claude_md_import(original);
        assert!(updated.contains(CLAUDE_AIDA_IMPORT));
        // Prose still present at the top, untouched.
        assert!(updated.starts_with("# CLAUDE.md\n\nNo subsections, just prose.\n"));
    }

    #[test]
    fn has_import_detects_presence_anywhere() {
        assert!(claude_md_has_import("text @.claude/AIDA.md text"));
        assert!(!claude_md_has_import("@.claude/OTHER.md"));
        assert!(!claude_md_has_import(""));
    }

    #[test]
    fn generated_claude_md_has_discipline_section() {
        // trace:STORY-255 | ai:claude
        // trace:TASK-573 | ai:claude — pack discovery is now an @-import
        let scaffolder = Scaffolder::new(
            std::path::PathBuf::from("/tmp/aida-story-255-test"),
            ScaffoldConfig::default(),
        );
        let store = RequirementsStore::default();
        let md = scaffolder.generate_claude_md(&store);
        // The discipline section is a stub heading + @-import of the pack's
        // README. The full pointer list (advisor-role, machinery-glossary,
        // tag-conventions, …) lives in that README; Claude Code expands the
        // import at session start.
        assert!(md.contains("## Discipline for AIDA-using sessions"));
        assert!(md.contains("@docs/aida/discipline/README.md"));
        // The change is additive — the AIDA conventions import still lands.
        assert!(md.contains(CLAUDE_AIDA_IMPORT));
        assert!(md.contains("## Project overview"));
        // No leftover inlined bullets — the README is the single source of
        // truth for pointers; CLAUDE.md must not duplicate them.
        assert!(!md.contains("- **Roles**"));
        assert!(!md.contains("- **Start here**"));
    }
}

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

{project_name}{description}{tech_stack}{features}{type_section}{discipline}
"#,
            project_name = project_name,
            description = description,
            tech_stack = tech_stack_section,
            features = features_section,
            type_section = type_section,
            discipline = Self::discipline_section(),
        )
    }

    /// The "Discipline for AIDA-using sessions" section appended to a
    /// scaffolded CLAUDE.md. A 3-line stub that `@`-imports the discipline
    /// pack's README so Claude Code expands the canonical pointer list at
    /// session start — no duplicate maintenance between this file and the
    /// scaffolded `docs/aida/discipline/README.md`.
    /// trace:STORY-255 | ai:claude
    /// trace:TASK-338 | ai:claude — added the machinery-glossary bullet
    /// trace:STORY-443 | ai:claude — pack relocated under docs/aida/ namespace
    /// trace:TASK-573 | ai:claude — collapsed inline bullets to a single @-import
    fn discipline_section() -> &'static str {
        r#"

## Discipline for AIDA-using sessions

@docs/aida/discipline/README.md
"#
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
