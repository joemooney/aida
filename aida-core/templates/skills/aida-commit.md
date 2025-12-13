# AIDA Commit Skill

## Purpose

Create git commits with automatic requirement linkage, ensuring all implemented work is tracked in the requirements database.

## When to Use

Use this skill when:
- User wants to commit changes with requirement traceability
- User says "commit" or "save changes" after implementing features
- User wants to ensure implemented work is captured in requirements

## Core Philosophy

**No implementation without a requirement.** This skill bridges the gap between code changes and requirements tracking by:
1. Detecting implemented code that lacks requirement traces
2. Prompting to create requirements before committing
3. Automatically linking commits to requirements

## Workflow

### Step 1: Analyze Staged Changes

```bash
git status --porcelain
git diff --cached --name-only
```

Identify:
- New files created
- Modified files
- File types and locations (src/, tests/, docs/)

### Step 2: Extract Existing Requirement Traces

Search staged changes for trace comments:

```bash
git diff --cached | grep -E "trace:[A-Z]+-[0-9]+"
```

Build a list of SPEC-IDs found in the staged code.

### Step 3: Identify Untraced Implementation

For each new or modified source file without trace comments, flag it as potentially untracked work.

Present to user:
```
## Staged Changes Analysis

### Traced (linked to requirements)
- src/feature.rs → FR-0042
- src/auth.rs → AUTH-0001

### Untraced (no requirement link)
- src/helper.rs (new file, 150 lines)
- src/utils.rs (modified, +45 lines)
```

### Step 4: Prompt for Missing Requirements

For untraced work, offer options:

1. **Create new requirement**: Add to database with `completed` status
2. **Link to existing**: Search database for relevant requirements
3. **Skip**: Minor changes that don't need tracking (refactoring, formatting)

For new requirements:
```bash
aida add \
  --title "<generated title from code context>" \
  --description "Implementation of <feature description>" \
  --type functional \
  --status completed
```

### Step 5: Create Commit

Generate commit message that includes requirement links:

```bash
git commit -m "$(cat <<'EOF'
<user message or generated summary>

Requirements:
- FR-0042: Feature title
- AUTH-0001: Auth feature

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

### Step 6: Update Requirement Statuses

For each linked requirement that was in `approved` or `in-progress` status:

```bash
aida edit <SPEC-ID> --status completed
aida comment add <SPEC-ID> "Committed in $(git rev-parse --short HEAD)"
```

## Integration with Git Workflow

This skill can be invoked:
1. **Manually**: User calls `/aida-commit` before committing
2. **By habit**: CLAUDE.md encourages using this instead of raw `git commit`

## CLI Reference

```bash
# Check git status
git status --porcelain
git diff --cached

# Search for trace comments
git diff --cached | grep -E "trace:[A-Z]+-[0-9]+"

# Search requirements database
aida search "<keyword>"
aida list --status approved

# Add requirement
aida add --title "..." --description "..." --status completed

# Update requirement
aida edit <SPEC-ID> --status completed
aida comment add <SPEC-ID> "..."

# Commit
git commit -m "..."
```

## Best Practices

- Use status `completed` for requirements implemented in this commit
- Add commit hash to requirement comments for traceability
- Group related changes into single commits with multiple requirement links
- Don't skip trace comments for substantial code (>20 lines of logic)
