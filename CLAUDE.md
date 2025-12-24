# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

AI Design Assistant
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

### CLI Commands
```bash
aida list                              # List all requirements
aida list --status draft               # Filter by status
aida search "<query>"                  # Simple case-insensitive search
aida grep "<pattern>" -i               # Advanced regex search
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

### /aida-evaluate
Evaluate a requirement's quality using AI analysis:
- Load requirement from database
- Assess clarity, testability, completeness, and consistency
- Generate quality score (1-10) with detailed feedback
- Offer follow-up actions: improve, split, or accept

### /aida-commit
Commit changes with automatic requirement linking:
- Analyze staged changes for requirement traces
- Identify untraced implementation code
- Prompt to create requirements for untracked work
- Create commit with requirement references
- Update linked requirement statuses

### /aida-sync
Meta-level skill for AIDA template management:
- Check if templates have been modified and need rebuilding
- Verify symlink integrity in AIDA repo
- Check scaffold status in scaffolded projects
- Ensure CLAUDE.md documents all skills
- Use after modifying templates to propagate changes

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

