# AIDA Implementation Skill

## Purpose

Implement an approved requirement with full traceability, evolving the requirement database to capture implementation details and creating child requirements as needed.

## When to Use

Use this skill when:
- User says "implement <SPEC-ID>" or "work on <requirement>"
- User triggers "Copy for Claude Code" from the aida-gui AI menu
- A requirement in `Planned` or `In Progress` status is ready to be implemented
- Continuing implementation of a requirement from a previous session

## Prerequisites

Before implementing, check the requirement status:
- **Planned** or **In Progress**: Proceed with implementation
- **Approved**: Suggest running `/aida-plan` first to plan the implementation
- **Draft**: Cannot implement - needs approval first
- **Completed**: Already implemented
- **Rejected**: Cannot implement - requirement was rejected

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

**Check status before proceeding:**
- If `Approved`: Tell the user this requirement hasn't been planned yet. Suggest running `/aida-plan <SPEC-ID>` first to decompose and document the implementation approach.
- If `Planned`: Proceed to Step 2. Update status to `In Progress`:
  ```bash
  aida edit <SPEC-ID> --status in_progress
  ```
- If `In Progress`: Continue implementation (resuming from previous session)
- If `Draft`, `Completed`, or `Rejected`: Stop and inform user why implementation cannot proceed

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

**Generic (use language-appropriate comment syntax):**
```
// trace:FR-0042 - Feature title | ai:claude:high | impl:2025-12-10 | by:joe
// Your implementation here
```

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
aida comment add --id <SPEC-ID> --content "Implementation note: Used async/await pattern for..."

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
aida comment add --id <SPEC-ID> --content "Implementation complete. Files modified: src/foo.rs, src/bar.rs"
```

3. Create "Verifies" relationship if tests were added:
```bash
aida rel add --from <TEST-SPEC-ID> --to <SPEC-ID> --type Verifies
```

### Step 7: Commit with AI Attribution

When committing code with trace comments, include the AI attribution tag in the commit message:

**Commit Message Format:**
```
[AI:claude:high] feat: description of changes

Trace: FR-0042, FR-0043
```

Where:
- `[AI:tool:conf]`: AI attribution tag (e.g., `[AI:claude:high]`)
  - `tool`: AI tool used (e.g., `claude`, `copilot`)
  - `conf`: Confidence level (`high`, `med`, `low`)
- `Trace:`: Optional list of requirement IDs implemented in this commit

The git hook will warn if files have trace comments but the commit message lacks the AI tag.

**Example:**
```bash
git commit -m "[AI:claude:high] feat: add grep command for searching requirements

Implements pattern search with regex support, context lines, and field filtering.

Trace: FR-0250"
```

## State Transitions

During implementation, requirements should transition through:

1. **Planned** -> **In Progress** (when starting implementation)
2. **In Progress** -> **Completed** (when implementation is verified)
3. **In Progress** -> **Draft** (if significant changes needed - requires re-approval and re-planning)

Note: Requirements should be in `Planned` status before implementation begins. If a requirement is still `Approved`, use `/aida-plan` first.

Update via:
```bash
aida edit <SPEC-ID> --status <new-status>
```

## CLI Reference

```bash
# Show requirement
aida show <SPEC-ID>

# Search for related requirements or design decisions
aida grep "keyword"                          # Search all fields
aida grep -i "pattern" -f description        # Case insensitive, description only
aida grep -E "TODO|FIXME" -f comments        # Regex search in comments
aida grep -l "database" --status approved    # List SPEC-IDs only, filter by status
aida grep -C 2 "error"                       # Show 2 lines of context

# Edit requirement
aida edit <SPEC-ID> --description "..." --status <status>

# Add comment
aida comment add --id <SPEC-ID> --content "Comment text"

# Add relationship
aida rel add --from <FROM-ID> --to <TO-ID> --type <Parent|Verifies|References|Duplicate>

# Create new requirement
aida add --title "..." --description "..." --type <type> --status draft

# List requirements by feature
aida list --feature <feature-name>
```
