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

### Session Workflow
If you work conversationally without explicit /aida-req calls, use `/aida-capture` at session end to review and capture any requirements that were discussed but not yet added to the database.

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

