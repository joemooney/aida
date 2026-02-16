---
name: aida-review
description: Review code changes against requirement specs. Use to verify implementation coverage, check for spec gaps, and validate trace comments.
allowed-tools:
  - Bash
  - Read
  - Glob
  - Grep
---

# AIDA Code Review Skill

## Purpose

Review code changes against linked requirements to verify implementation coverage, identify spec gaps, and validate traceability.

## When to Use

Use this skill when:
- User wants to review code changes against requirements
- Before creating a pull request
- User asks "did I cover everything?" or "any spec gaps?"
- After a coding session to verify completeness

## Recent Changes

!`git log --oneline -10 2>/dev/null || echo "no git history"`

## Workflow

### Step 1: Gather Changes

```bash
# Changes since last commit or against a branch
git diff --name-only HEAD 2>/dev/null
git diff --stat HEAD 2>/dev/null

# Or changes in a branch
git diff --name-only main...HEAD 2>/dev/null
```

### Step 2: Extract Requirement Traces

For each changed file, extract trace comments:

```bash
# Find all trace comments in changed files
git diff --name-only HEAD 2>/dev/null | xargs grep -n "trace:" 2>/dev/null
```

Build a map of: `file → [SPEC-ID, ...]`

### Step 3: Load Linked Requirements

For each unique SPEC-ID found:

```bash
aida show <SPEC-ID>
```

Gather the requirement's description, acceptance criteria, and status.

### Step 4: Analyze Coverage

For each linked requirement, check:
1. **Implementation completeness**: Does the code cover all behaviors described?
2. **Acceptance criteria**: Are all criteria addressed?
3. **Edge cases**: Are error conditions handled?
4. **Status consistency**: Is the requirement status appropriate for the changes?

### Step 5: Identify Gaps

Check for:
- **Untraced files**: Implementation files without trace comments
- **Missing requirements**: Functionality added without corresponding specs
- **Partial implementations**: Requirements only partially addressed
- **Status mismatches**: Completed code for draft/unapproved requirements

```bash
# Find implementation files without trace comments
git diff --name-only HEAD 2>/dev/null | grep -E '\.(rs|py|ts|js)$' | while read f; do
    if ! grep -q "trace:" "$f" 2>/dev/null; then
        echo "UNTRACED: $f"
    fi
done
```

### Step 6: Report

Present review results:

```
## Code Review: Requirement Coverage

### Traced Requirements
- FR-0042: Login validation — FULLY COVERED
- FR-0043: Password strength — PARTIALLY COVERED (missing edge case)

### Untraced Changes
- src/utils/helper.rs (new, 80 lines) — no requirement link

### Recommendations
1. Add acceptance criteria test for FR-0043 edge case
2. Create requirement for utility helper or add trace to existing spec
3. Update FR-0042 status to completed
```

### Step 7: Offer Actions

- **Add traces**: Insert trace comments in untraced files
- **Create requirements**: Add specs for untraced functionality
- **Update statuses**: Transition requirements to appropriate status
- **Add comments**: Document review findings on requirements

## CLI Reference

```bash
aida show <SPEC-ID>                              # Show requirement details
aida search "<keyword>"                          # Search requirements
aida edit <SPEC-ID> --status completed           # Update status
aida comment add <SPEC-ID> "Review: ..."         # Add review note
```

## Best Practices

- Review before committing to catch gaps early
- Every implementation file should have at least one trace comment
- Use this skill together with `/aida-commit` for comprehensive traceability
