---
name: aida-search
description: Unified search across requirements and code. Find requirements by keyword, trace links between specs and implementation, and correlate results.
allowed-tools:
  - Bash
  - Grep
  - Glob
---
<!-- AIDA Generated: v2.0.0 | checksum:12e4ffa8 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Unified Search Skill

## Purpose

Search across both requirements database and codebase simultaneously, correlating results to show the full picture of how specs relate to implementation.

## When to Use

Use this skill when:
- User wants to find requirements related to a topic
- User asks "where is X implemented?" or "what requirement covers Y?"
- User needs to understand the relationship between specs and code
- User wants to trace from code back to requirements or vice versa

## Workflow

### Step 1: Search Requirements

```bash
# Search requirement titles and descriptions
aida search "<query>" 2>/dev/null

# Advanced regex search
aida grep "<pattern>" -i 2>/dev/null

# Search specific fields
aida grep "<pattern>" -f description 2>/dev/null
aida grep "<pattern>" -f comments 2>/dev/null
```

### Step 2: Search Code

Search for trace comments and related implementation:

```bash
# Find trace comments mentioning the topic
grep -r "trace:.*<keyword>" --include="*.rs" --include="*.py" --include="*.ts" --include="*.js" src/ 2>/dev/null

# Find implementation code related to the topic
grep -rn "<keyword>" --include="*.rs" --include="*.py" --include="*.ts" --include="*.js" src/ 2>/dev/null | head -20
```

### Step 3: Correlate Results

For each requirement found, check if there's matching code:

```bash
# For each SPEC-ID found in requirements
grep -r "trace:<SPEC-ID>" src/ 2>/dev/null
```

For each trace comment found in code, verify the requirement exists:

```bash
aida show <SPEC-ID> 2>/dev/null
```

### Step 4: Present Unified Results

```
## Search Results: "<query>"

### Requirements (N matches)
- [FR-0042] Login validation — Status: Completed
  Implementation: src/auth/login.rs:45
- [FR-0043] Password strength — Status: In Progress
  Implementation: (none found)

### Code References (M matches)
- src/auth/login.rs:45 — trace:FR-0042
- src/auth/register.rs:12 — trace:FR-0050
- src/utils/crypto.rs:8 — (no trace comment)

### Gaps
- FR-0043: Requirement exists but no implementation found
- src/utils/crypto.rs: Implementation exists but no requirement link
```

### Step 5: Offer Actions

- **Show details**: View full requirement or file content
- **Add trace**: Insert trace comment in unlinked code
- **Create requirement**: Add spec for untracked implementation
- **Narrow search**: Refine with more specific terms

## CLI Reference

```bash
aida search "<query>"                            # Simple search
aida grep "<pattern>" -i                         # Case-insensitive regex
aida grep "<pattern>" -f description             # Search specific field
aida grep -l "<pattern>"                         # List matching SPEC-IDs only
aida show <SPEC-ID>                              # Show full requirement
```