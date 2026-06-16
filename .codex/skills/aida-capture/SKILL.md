---
name: aida-capture
description: Review conversation and capture missed requirements. Use at end of sessions to ensure all discussed features and decisions are tracked.
allowed-tools:
  - Bash
  - Read
  - Grep
---
<!-- AIDA Generated: v2.0.0 | checksum:9c27e59c | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Session Capture Skill

## Purpose

Review the current conversation and capture any requirements, features, or implementation details that were discussed but not yet added to the requirements database.

## When to Use

Use this skill when:
- User says "capture requirements" or "review session"
- At the end of a conversational coding session
- User asks to update requirements based on what was discussed
- After implementing features without explicitly creating requirements

## Autonomy mode — `$AIDA_ZEN` / `$AIDA_HEADLESS` (TASK-297)

This skill's interactive pause is **Step 4** (per-finding "Offer Actions":
add / update / skip). It is a `kind:confirmation` prompt — capture is
best-effort bookkeeping whose safe default (file the finding) is obvious.
Before surfacing it, check the autonomy mode:

```bash
aida zen status            # prints: zen | interactive
echo "${AIDA_HEADLESS:-}"
```

- **`interactive`** (default) — surface the per-finding prompt unchanged.
- **`zen`** (corroborated — never the bare `$AIDA_ZEN` env var, BUG-237) —
  auto-resolve Step 4 to its safe default and proceed, printing
  `↳ zen: auto-resolved "capture this finding?" → option 1`.
- **`AIDA_HEADLESS=1`** (a `--no-human` drain) is the stronger mode and
  overrides `--zen`. AskUserQuestion under `--no-human=both` is
  permission-denied and crashes the session ~10s in (SPIKE-7 / BUG-280) — so
  under headless the skill **never** prompts: auto-capture each clear finding
  as a `draft` requirement (the conservative status — a human triages it
  later) and skip ambiguous ones rather than guessing a wrong type/status.
  Losing a capture is recoverable; a hung drain is not. `--no-human` >
  `--zen` > default.

An un-annotated prompt defaults to `design-fork` (pause-safe). Author
guidance: `docs/aida/discipline/skill-prompt-kinds.md`. trace:TASK-297

## In-Progress Work

!`aida list --status in-progress 2>/dev/null | head -10 || echo "none"`

## Workflow

### Step 1: Scan Conversation

Review the conversation history for:
- Features that were discussed or requested
- Bugs that were identified or fixed
- Implementation decisions that were made
- Ideas or future enhancements mentioned
- Any work that was completed

### Step 2: Check Against Database

For each finding, check if it already exists:

```bash
aida list --search "<keyword>"
```

### Step 3: Present Findings

Present a summary to the user:
```
## Session Review

### Implemented (not in database)
- [Description of implemented work]

### Discussed (not captured)
- [Description of discussed feature/idea]

### Existing Requirements Updated
- [SPEC-ID] - Status changed / notes added
```

### Step 4: Offer Actions

<!-- kind:confirmation -->
Under `$AIDA_ZEN` or `AIDA_HEADLESS=1` this prompt auto-resolves (see
*Autonomy mode* above) — headless captures land as `draft` for later triage
rather than blocking the run. For each finding, offer to:
1. **Add as new requirement**: Create with appropriate type and status
2. **Update existing**: Add comments or change status
3. **Skip**: Don't capture this item

### Step 5: Execute Updates

For new requirements:
```bash
aida add --title "..." --description "..." --type functional --status completed --tags "tag1,tag2"
```

For existing requirements:
```bash
aida comment add <SPEC-ID> "Session note: ..."
aida edit <SPEC-ID> --status completed
```

## CLI Reference

```bash
# Search for existing requirements
aida list --search "<keyword>"

# Add new requirement (NOTE: use --tags not --tag)
aida add --title "..." --description "..." --status <status> --tags "comma,separated"

# Update requirement
aida edit <SPEC-ID> --status <status>

# Add comment
aida comment add <SPEC-ID> "Comment text"
```

## Best Practices

- Use status `completed` for work that was already implemented
- Use status `draft` for ideas that need refinement
- Link related requirements that were discovered during the session
- Add implementation comments with file paths that were modified