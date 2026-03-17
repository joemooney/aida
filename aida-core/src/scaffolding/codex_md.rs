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
        let req_count = store.requirements.len();

        format!(
            r#"# AGENTS.md

Guidance for AI coding agents (Codex CLI, Claude Code, or any MCP-compatible agent) working in this repository.

## Project Overview

{project_name}{description}

## Requirements Database

- **Source of truth**: AIDA requirements database (`{db_filename}`)
- **Requirements**: {req_count} tracked
- **Query via CLI**: `aida list`, `aida show <SPEC-ID>`, `aida search "keyword"`
- **Query via MCP**: if configured, use `list_requirements`, `show_requirement`, `search_requirements` tools

## Development Workflow

### Requirement-First Development

1. **Before coding**: check if a requirement exists for the work
   ```bash
   aida search "feature description"
   aida list --status approved
   ```

2. **During coding**: add trace comments linking code to requirements
   ```
   // trace:FR-042 | ai:codex
   ```

3. **Before committing**: ensure all work is traced
   ```bash
   aida show FR-042                    # verify requirement exists
   aida edit FR-042 --status completed # update status
   ```

### Core CLI Commands

```bash
aida list                              # List all requirements
aida list --status approved            # Filter by status
aida show <SPEC-ID>                    # Show details (e.g., FR-042)
aida search "<query>"                  # Search by keyword
aida add --title "..." --description "..." --status draft
aida edit <SPEC-ID> --status completed
aida comment add <SPEC-ID> "Implementation note..."
aida rel add --from <ID> --to <ID> --type references
```

### Commit Format

```
[AI:codex] type(scope): description (SPEC-ID)
```

Examples:
```
[AI:codex] feat(auth): add login validation (FR-042)
[AI:codex] fix(api): handle null response (BUG-023)
```

## MCP Integration

If AIDA is configured as an MCP server, these tools are available:

| Tool | Purpose |
|------|---------|
| `list_requirements` | List requirements with optional status/type filters |
| `show_requirement` | Show full details of a requirement by SPEC-ID |
| `search_requirements` | Search by keyword across titles and descriptions |
| `add_requirement` | Create a new requirement |
| `update_requirement` | Update status, priority, owner, etc. |
| `add_comment` | Add implementation notes to a requirement |
| `list_features` | List feature categories |

To configure MCP for Codex CLI:
```bash
codex mcp add aida -- aida mcp-serve
```

## Non-Interactive Workflows (codex exec)

```bash
# Implement a specific requirement
codex exec "Implement FR-042. Use 'aida show FR-042' to see the details first."

# Sprint standup
codex exec "Run 'aida list --status in-progress' and 'git log --since=yesterday'. Generate a standup report."

# Capture untraced work
codex exec "Review today's git commits. For each, check if trace comments exist. Create requirements for untraced code."

# Code review with traceability
codex review --base main "Check that all new functions have trace comments (// trace:SPEC-ID format)"
```

## Storage Modes

AIDA supports multiple backends:
- **SQLite** (default): `requirements.db`
- **PostgreSQL**: for teams — `aida --file "postgres://..." list`
- **Git (distributed)**: `aida init --distributed` — offline-capable with node-namespaced IDs
- **YAML**: simplest, git-friendly

## Key Principles

- **No implementation without a requirement.** Create requirements before coding.
- **Trace everything.** Every function, every commit links back to why it exists.
- **Status is truth.** Keep requirement statuses current (draft → approved → completed).
- **AI attribution.** Mark AI-assisted code with `[AI:codex]` or `[AI:claude]` in commits.
"#
        )
    }
}
