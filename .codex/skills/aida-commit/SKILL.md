---
name: aida-commit
description: Commit changes with automatic requirement linking. Analyzes staged changes for requirement traces and creates properly formatted commits.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
  - Grep
---
<!-- AIDA Generated: v2.0.0 | checksum:449c1173 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Commit Skill

## Purpose

Create git commits with automatic requirement linkage, ensuring all implemented work is tracked in the requirements database.

## When to Use

Use this skill when:
- User wants to commit changes with requirement traceability
- User says "commit" or "save changes" after implementing features
- User wants to ensure implemented work is captured in requirements

## Core Philosophy

**No implementation without a requirement.** This skill detects untraced code, prompts to create requirements before committing, and automatically links commits to requirements.

## Autonomy mode — `$AIDA_ZEN` / `$AIDA_HEADLESS` (TASK-297)

This skill's only interactive pause is **Step 4** (prompt to create or link
a requirement for untraced work). It is a `kind:confirmation` prompt — its
default action (file a `completed` requirement for the untraced work, or
proceed when the change is trivial) is the obvious one. Before surfacing it,
check the autonomy mode:

```bash
aida zen status            # prints: zen | interactive
echo "${AIDA_HEADLESS:-}"
```

- **`interactive`** (default) — surface the prompt unchanged.
- **`zen`** (corroborated — never the bare `$AIDA_ZEN` env var, BUG-237) —
  auto-resolve Step 4 to its safe default and proceed, printing
  `↳ zen: auto-resolved "create requirement?" → option 1`.
- **`AIDA_HEADLESS=1`** (a `--no-human` drain) is the stronger mode and
  overrides `--zen`. Calling AskUserQuestion under `--no-human=both` is
  permission-denied at the harness layer and crashes the session ~10s in
  (SPIKE-7 / BUG-280) — so under headless the skill **never** prompts: it
  auto-resolves Step 4 to the safe default (file the untraced work as a
  `completed` requirement, or skip when the change is already traced) and
  commits without pausing. A headless implementer already carries its spec
  id, so the common case is fully traced; this guard only catches a stray
  untraced file. `--no-human` > `--zen` > default.

An un-annotated prompt defaults to `design-fork` (pause-safe). Author
guidance: `docs/aida/discipline/skill-prompt-kinds.md`. trace:TASK-297

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
- `[AI:claude]` - High (>80% AI-generated)
- `[AI:claude:med]` - Medium (40-80% AI with modifications)
- `[AI:claude:low]` - Low (<40% AI, mostly human)

## Staged Changes

!`git diff --cached --stat 2>/dev/null | tail -5`
!`git diff --cached --name-only 2>/dev/null | xargs grep -l "trace:" 2>/dev/null | head -10`

## Workflow

### Step 1: Analyze Staged Changes

```bash
git status --porcelain
git diff --cached --name-only
```

Identify new files, modified files, and their locations (src/, tests/, docs/).

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
### Traced (linked to requirements)
- src/feature.rs → FR-0042

### Untraced (no requirement link)
- src/helper.rs (new file, 150 lines)
```

### Step 4: Prompt for Missing Requirements

<!-- kind:confirmation -->
Under `$AIDA_ZEN` or `AIDA_HEADLESS=1` this prompt auto-resolves to the safe
default (see *Autonomy mode* above) — never block a headless commit on it.
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
  --status completed \
  --tags "comma,separated"
```

### Step 5: Determine Commit Message Components

Based on analysis:

1. **AI Tag**: Include `[AI:claude]` if any staged files have `trace:` comments with `ai:` attribution
2. **Type**: Determine from changes (feat for new functionality, fix for bug fixes, etc.)
3. **Scope**: Extract from file paths or requirement feature category
4. **Description**: Summarize the change concisely
5. **REQ-ID**: Use primary requirement ID from traces

### Step 6: Create Commit

```bash
git commit -m "$(cat <<'EOF'
[AI:claude] feat(auth): add login validation (FR-0042)

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>
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

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>
EOF
)"
```

### Step 7: Update Requirement Statuses

For each linked requirement in `approved` or `in-progress` status:

```bash
aida edit <SPEC-ID> --status completed
aida comment add <SPEC-ID> "Committed in $(git rev-parse --short HEAD)"
```

## Configuration

Environment variables:
- `AIDA_COMMIT_STRICT=true` - Reject non-conforming commits
- `AIDA_REQUIRE_REQ_FOR_FEAT=true` - Require REQ-ID for feat/fix commits (default: true)
- `AIDA_REQUIRE_AI_TAG=true` - Require AI tag when trace comments exist (default: true)

## CLI Reference

```bash
# Search requirements database
aida search "<keyword>"
aida add --title "..." --description "..." --status completed --tags "comma,separated"
aida edit <SPEC-ID> --status completed
aida comment add <SPEC-ID> "..."
```

## Best Practices

- Always include REQ-ID for feat/fix commits; add commit hash to requirement comments for bidirectional traceability
- Use `[AI:claude]` when the commit includes AI-assisted code
- Don't skip trace comments for substantial code (>20 lines of logic)
- Shipped a new CLI slice verb? Update its parent skill to call it (no re-impl) — see `docs/aida/discipline/skill-cli-symmetry.md`. <!-- trace:TASK-736 -->

## Related skills / commands

- `/aida-rebase` — before committing on a session that's been open a
  while, run `aida rebase --dry-run --json` first. Committing on a
  stale base creates divergent-branch grief at push time; the
  proactive-invocation playbook in the `/aida-rebase` skill names "about
  to commit, session open >15 min" as a trigger. trace:TASK-105
- `/aida-pr` — once the batch's commits are in, `/aida-pr` opens the PR
  with linked specs and a test plan.