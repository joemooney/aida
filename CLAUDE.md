# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

AI Design Assistant
## Requirements Management

This project uses AIDA for requirements tracking. **Do NOT maintain a separate REQUIREMENTS.md file.**

Requirements database: `requirements.db`

### Database Storage
AIDA supports both YAML and SQLite backends:
- **YAML**: Human-readable, git-friendly, good for single-user scenarios
- **SQLite**: Better for concurrent access (GUI + CLI), optimistic locking

To migrate: `aida db migrate --from yaml --to sqlite`

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

### Proactive Requirements Workflow (IMPORTANT)

**Requirement-first development**: Before implementing any feature or fix, ensure a requirement exists:

1. **Before coding**: Check if work has a SPEC-ID. If not, create one:
   ```bash
   aida add --title "..." --description "..." --status approved
   ```

2. **During coding**: Add trace comments linking code to requirements:
   ```rust
   // trace:FR-0042 | ai:claude:high
   ```

3. **Before committing**: Use `/aida-commit` to ensure all changes are linked to requirements

### Session Workflow
- **Proactive capture**: Create requirements BEFORE implementation, not after
- **Commit-time validation**: Use `/aida-commit` to catch untraced work
- **Safety net**: If you work conversationally without explicit /aida-req calls, use `/aida-capture` at session end to review and capture any requirements that were discussed but not yet added to the database

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
- `--type`: `functional`, `non-functional`, `system`, `user` (lowercase!)
- `--feature`: Feature category name (NOT a type!)
- `--status`: `draft`, `approved`, `in-progress`, `completed`, `rejected`
- `--priority`: `high`, `medium`, `low`

