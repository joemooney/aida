---
name: aida-commit
description: Commit changes with automatic requirement linking. Analyzes staged changes for requirement traces and creates properly formatted commits.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
  - Grep
---

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

## Commit Message Format

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

**AI Confidence Levels:**
- `[AI:claude]` - High confidence (implied, >80% AI-generated)
- `[AI:claude:med]` - Medium confidence (40-80% AI with modifications)
- `[AI:claude:low]` - Low confidence (<40% AI, mostly human)

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

### Step 5: Determine Commit Message Components

Based on analysis:

1. **AI Tag**: Include `[AI:claude]` if any staged files have `trace:` comments with `ai:` attribution
2. **Type**: Determine from changes (feat for new functionality, fix for bug fixes, etc.)
3. **Scope**: Extract from file paths or requirement feature category
4. **Description**: Summarize the change concisely
5. **REQ-ID**: Use primary requirement ID from traces (prefer the one most relevant to the change)

### Step 6: Create Commit

Generate commit message in the standard format:

```bash
git commit -m "$(cat <<'EOF'
[AI:claude] feat(auth): add login validation (FR-0042)

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

For commits touching multiple requirements, include them in the body:
```bash
git commit -m "$(cat <<'EOF'
[AI:claude] feat(auth): implement user authentication (AUTH-0001)

Also addresses:
- FR-0042: Login form validation
- FR-0043: Password strength requirements

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

### Step 7: Update Requirement Statuses

For each linked requirement that was in `approved` or `in-progress` status:

```bash
aida edit <SPEC-ID> --status completed
aida comment add <SPEC-ID> "Committed in $(git rev-parse --short HEAD)"
```

## Configuration

The commit-msg hook respects these environment variables:
- `AIDA_COMMIT_STRICT=true` - Reject commits that don't meet format requirements
- `AIDA_REQUIRE_REQ_FOR_FEAT=true` - Require REQ-ID for feat/fix commits (default: true)
- `AIDA_REQUIRE_AI_TAG=true` - Require AI tag when trace comments exist (default: true)

Or create `.aida/commit-config`:
```bash
STRICT_MODE=true
REQUIRE_REQ_FOR_FEAT=true
REQUIRE_AI_TAG=true
```

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

- Always include REQ-ID for feat/fix commits
- Use `[AI:claude]` when the commit includes AI-assisted code
- Add commit hash to requirement comments for bidirectional traceability
- Group related changes into single commits with primary REQ-ID in subject, others in body
- Don't skip trace comments for substantial code (>20 lines of logic)
