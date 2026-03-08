use super::*;

impl Scaffolder {
    /// Generate AGENTS.md content for Codex-compatible coding agents
    pub(super) fn generate_agents_md(&self, store: &RequirementsStore) -> String {
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

        let db_filename = self.database_filename();

        format!(
            r#"# AGENTS.md

Guidance for Codex-compatible coding agents working in this repository.

## Project Overview

{}{}

## Requirements Workflow

- Source of truth: AIDA requirements database (`{}`)
- Use requirement-first development for feature/fix work.
- Add trace comments in implementation code with format: `trace:<SPEC-ID> | ai:<tool>`
- Keep requirement status updated during implementation.

## Core Commands

```bash
aida list
aida show <SPEC-ID>
aida add --title "..." --description "..." --status draft
aida edit <SPEC-ID> --status in-progress
aida comment add <SPEC-ID> "..."
```

## Agent Skills

Codex-compatible workflow skills are scaffolded under:
- `.codex/skills/<skill>/SKILL.md`

Transferable AIDA skills include:
- `aida-req`, `aida-plan`, `aida-implement`, `aida-capture`
- `aida-evaluate`, `aida-review`, `aida-test`
- `aida-commit`, `aida-search`, `aida-sprint`, `aida-standup`
- `aida-docs`, `aida-release`, `aida-onboard`, `aida-sync`

## Git & Traceability

- For `feat`/`fix` commits, include a requirement ID in commit message when possible.
- Keep AI attribution consistent (`[AI:<tool>]`) when AI-assisted code is committed.
- If code changes are not linked to a requirement, capture them before commit.
"#,
            project_name, description, db_filename
        )
    }
}
