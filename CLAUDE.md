# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

AI Design Assistant

### React Dashboard
The React dashboard is located at `aida-web-react/` and runs on port 5173 (dev) via Vite, connecting to the REST API on port 8080. Stack: React 19, Vite 8, Tailwind CSS 4, @tanstack/react-query. Shared TypeScript types are generated from Rust structs in `shared/types.ts`. Views: Dashboard (project-wide status cards + active sprint summary + queue widget with clickable navigation to filtered List View), Kanban Board (with tag filter dropdown and active filter chips), List View (with flat/tree toggle for parent/child hierarchy, inline tag pills, drag-to-queue and drag-to-reparent in tree mode via @dnd-kit, advanced query builder with AND/OR grouping via react-querybuilder + json-logic-js), My Queue (personal focus inbox with drag-to-reorder via @dnd-kit/sortable, owner-scoped via `?user=` URL param with owner-picker dropdown and read-only mode for other users' queues), My Activity (planned vs. actual work reconciliation — shows user's actual work cross-referenced against queue, with stats bar highlighting unqueued work and untouched queue items, time range filtering, and owner-scoped via `?user=` URL param), Sprint Planning (with drag-and-drop backlog/sprint assignment), Timeline (chronological event feed from history/comments/creation), Skills Browser (view/edit skills and commands, run executable skills like compiler-warnings with real-time SSE output, structured results with risk-level categorization, action buttons for auto-fix/create-defect/create-task, and context-aware AI chat follow-up), Chat (AI-powered Q&A for PMs/stakeholders with streaming responses, full requirements context, and auto-linked spec IDs via LinkedMarkdown — requires `ANTHROPIC_API_KEY` env var or runtime key via Admin settings), Settings (store metadata, relationship/type/reaction definitions, ID config, prefix management via `/api/v2/settings/` endpoints, Admin tab with dev-mode server rebuild & restart via SSE and runtime API key management). Features: structured search (`owner:joe`, `tag:frontend` syntax in search bar), tag-based filtering across views, advanced query builder (react-querybuilder with json-logic evaluation, URL-persisted queries via `?aq=` param, localStorage saved queries, sprint/tags/custom field support), markdown description rendering in detail panel (with enhanced editor: expand/collapse, live preview, markdown help toolbar, `::color[text]` colored text syntax, and syntax-highlighted code blocks via react-syntax-highlighter), "Add to Queue" actions in detail header and list rows, centralized keyboard shortcuts system with chord navigation (g+key for view switching), j/k list selection, quick pickers (s/p/o for status/priority/owner), `f` to toggle advanced filter, and `?` help modal.

## Requirements Management

This project uses AIDA for requirements tracking. **Do NOT maintain a separate REQUIREMENTS.md file.**

Requirements database: `requirements.db`

### Database Storage
AIDA supports three storage backends:
- **YAML**: Human-readable, git-friendly, good for single-user scenarios
- **SQLite**: Better for concurrent access (GUI + CLI), optimistic locking
- **PostgreSQL**: Enterprise-grade, multi-user, native JSONB support

To migrate between backends:
```bash
aida db migrate --from yaml --to sqlite
aida db migrate --from sqlite --to postgres --output "postgres://user:pass@host:5432/db"
aida db migrate --from postgres --to yaml --output requirements.yaml
```

To use PostgreSQL directly:
```bash
aida --file "postgres://user:pass@localhost:5432/aida" list
```

### Project Initialization
```bash
aida init                              # Initialize AIDA in current directory
aida init --no-skills                  # Skip .claude/skills/ and .claude/commands/
aida init --no-hooks                   # Skip .claude/hooks/ and git hooks
aida init --force                      # Overwrite existing files if already initialized
```

`aida init` creates:
- `requirements.db` — SQLite database with seeded META requirements
- `.mcp.json` — Claude Code MCP integration config
- `CLAUDE.md` — Project context for AI sessions
- `.claude/skills/` — 15 workflow skills (unless `--no-skills`)
- `.claude/commands/` — Slash commands (unless `--no-skills`)
- `.claude/hooks/` — Commit validation hooks (unless `--no-hooks`)
- `docs/plans/` — Implementation plan archive

### CLI Commands
```bash
aida list                              # List all requirements
aida list --status draft               # Filter by status
aida search "<query>"                  # Simple case-insensitive search
aida grep "<pattern>" -i               # Advanced regex search
aida show <ID>                         # Show requirement details (e.g., FR-0042)
aida add --title "..." --description "..." --status draft --tags "tag1,tag2"  # Add new requirement
aida edit <ID> --status completed      # Update status
aida comment add <ID> "..."            # Add implementation note
```

### During Development
- When implementing a feature, update its requirement status
- Add comments to requirements with implementation decisions
- Create child requirements for sub-tasks discovered during implementation
- Link related requirements with: `aida rel add --from <FROM> --to <TO> --type <Parent|Verifies|References>`

### Proactive Requirements Workflow (IMPORTANT)

**Requirement-first development**: Before implementing any feature or fix, ensure a requirement exists:

1. **Before coding**: Check if work has a SPEC-ID. If not, create one:
   ```bash
   aida add --title "..." --description "..." --status approved
   ```

2. **During coding**: Add trace comments linking code to requirements:
   ```rust
   // trace:FR-0042 | ai:claude
   ```

3. **Before committing**: Use `/aida-commit` to ensure all changes are linked to requirements

### Session Workflow
- **Proactive capture**: Create requirements BEFORE implementation, not after
- **Commit-time validation**: Use `/aida-commit` to catch untraced work
- **Safety net**: If you work conversationally without explicit /aida-req calls, use `/aida-capture` at session end to review and capture any requirements that were discussed but not yet added to the database

### Plan Archival (IMPORTANT)

**Every implementation plan must be saved to `docs/plans/`.**

When you create or receive an implementation plan (via plan mode or user-provided), save it before implementing:

1. **Create** `docs/plans/` directory if it doesn't exist
2. **Save** the plan as `docs/plans/YYYY-MM-DD-<slug>.md` where `<slug>` is a short kebab-case description (e.g., `2026-02-20-sprint-charts.md`)
3. **Include** in the plan file:
   - The full plan content (phases, files, approach)
   - A `## Related Requirements` section listing any AIDA requirement IDs this plan addresses
   - A `## Status` section (initially "In Progress", updated to "Completed" when done)
4. **Update** the status to "Completed" after successful implementation

This ensures all architectural decisions and implementation approaches are preserved for future reference, even across chat sessions.

**Scaffolding**: When initializing new projects with `aida init`, the `docs/plans/` directory should be part of the standard project structure.

## Code Traceability

### Inline Code Traces
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
- `[AI:tool]` - Required when commit includes AI-assisted code (files with `trace:` comments)
- `type` - Required: feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert
- `(scope)` - Optional: component or area affected
- `(REQ-ID)` - Required for feat/fix commits, optional for chore/docs

**Confidence levels:**
- `[AI:claude]` - High confidence (implied, >80% AI-generated)
- `[AI:claude:med]` - Medium (40-80% AI with modifications)
- `[AI:claude:low]` - Low (<40% AI, mostly human)

**Configuration:**
- Set `AIDA_COMMIT_STRICT=true` to reject non-conforming commits
- Or create `.aida/commit-config` with settings

## Claude Code Skills

This project uses AIDA requirements-driven development with 16 skills:

### Core Skills
- `/aida-req` — Add new requirements with AI evaluation, quality feedback, and follow-up actions
- `/aida-implement` — Implement requirements with traceability, child breakdown, and inline trace comments
- `/aida-capture` — Review session and capture missed requirements as a safety net
- `/aida-evaluate` — Evaluate requirement quality (clarity, testability, completeness) with scoring

### Development Workflow
- `/aida-commit` — Commit with automatic requirement linking and untraced code detection
- `/aida-review` — Review code changes against requirement specs, identify coverage gaps
- `/aida-test` — Generate and run tests linked to requirements with verification relationships
- `/aida-plan` — Create implementation plans from requirements with architecture considerations

### Project Management
- `/aida-sprint` — Sprint planning: select approved requirements, group by feature, create sprint container
- `/aida-standup` — Generate daily standup from recent commits and requirement progress
- `/aida-onboard` — Interactive project onboarding: architecture summary, requirement stats, first task suggestions
- `/aida-search` — Unified search across requirements database and codebase

### Code Quality
- `/aida-compiler-warnings` — Analyze compiler/clippy warnings, categorize by risk level, recommend prioritized action plan

### Maintenance
- `/aida-release` — Release management: version bump, changelog generation, requirement status updates
- `/aida-docs` — Generate and update project documentation from requirements
- `/aida-sync` — AIDA template management: check templates, verify symlinks, ensure CLAUDE.md is current

### MCP Server
Run `aida mcp-serve` for native Claude Code tool integration via `.mcp.json`:
- Tools: `list_requirements`, `show_requirement`, `add_requirement`, `update_requirement`, `search_requirements`, `add_comment`, `list_features`
- Resources: `aida://project/summary`, `aida://requirements/tree`

## Template Architecture (IMPORTANT for AIDA Development)

**This section is critical for developers working on AIDA itself.**

AIDA has a dual-copy template system to support both development and standalone binary distribution:

### Template Locations
1. **Master templates**: `aida-core/templates/` - Embedded into binary at compile time
2. **Project-local templates**: `.claude/skills/` and `.claude/commands/` - Used by Claude Code directly

### Why Two Copies?
- The master templates (`aida-core/templates/`) get compiled into the binary via `build.rs`, allowing `aida init` to bootstrap new projects without external files
- The project-local templates (`.claude/`) are what Claude Code actually reads during development
- In the AIDA repo, we use **symlinks** from `.claude/` to `aida-core/templates/` to keep them in sync

### When Editing Skills/Commands
**CRITICAL**: When modifying any skill or command template:
1. Edit ONLY the master copy in `aida-core/templates/`
2. The symlinks ensure `.claude/` stays in sync automatically
3. Run `make sync-templates` to verify symlinks are correct
4. Changes will be embedded in the next binary build

### CLI Reference (Authoritative)
Always verify CLI arguments with `aida <command> --help`. Key parameters:
- `--type`: `functional`, `non-functional`, `system`, `user`, `bug`, `epic`, `story`, `task`, `spike`, `sprint`, `folder` (lowercase!)
- `--feature`: Feature category name (NOT a type!)
- `--status`: `draft`, `approved`, `in-progress`, `completed`, `rejected`
- `--priority`: `high`, `medium`, `low`

### Requirement Types

**Requirements** (use for features, behaviors, constraints):
- `functional` - Functional requirements (what the system does)
- `non-functional` - Performance, security, usability constraints
- `system` - Technical/infrastructure requirements
- `user` - User stories

**Agile artifacts** (use for project management):
- `epic` - Large features spanning multiple stories
- `story` - User stories for agile development
- `task` - Individual work items, chores, documentation
- `bug` - Bug reports and defects
- `spike` - Research and investigation tasks
- `sprint` - Sprint planning containers

**Organizational**:
- `folder` - Organizational folders (stateless)
- `meta` - AI prompts, templates, and configuration (stateless)

Use `task` type for chores, documentation, tooling, and other work that doesn't fit traditional requirements.

### Meta Requirements and AI Prompt Customization

Meta requirements store AI prompts as editable requirements in the database:

```bash
# List META requirements (AI prompts)
aida list --type meta

# View a prompt template
aida show META-002  # "Evaluate Requirement"

# Edit a prompt to customize AI behavior
aida edit META-002 --description "Your custom prompt template..."
```

**Default META prompts** (created automatically on `aida init`):
- META-002: Evaluate Requirement
- META-003: Find Duplicates
- META-004: Suggest Relationships
- META-005: Improve Description
- META-006: Generate Children

The AI system checks database prompts first, then falls back to embedded defaults.

### Tree Export/Import

Export requirement hierarchies for sharing between projects:

```bash
# Export a requirement tree (includes all descendants)
aida export --format tree --id FOLDER-001 -o templates.json

# Import into current database
aida import templates.json

# Import under a parent with conflict handling
aida import templates.json --parent FOLDER-002 --on-conflict skip
```

Conflict strategies: `skip` (skip existing), `rename` (add suffix), `replace` (overwrite)

