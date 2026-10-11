---
name: aida-capture
description: Review conversation and capture missed requirements.
allowed-tools:
  - Bash
  - Read
  - Grep
---

# AIDA Session Capture Skill

## Purpose

Review the current conversation and capture any requirements, features, or implementation details that were discussed but not yet added to the requirements database, plus **FAQ candidates**: questions the session answered with a verified answer.

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

**FAQ captures under zen/headless** land as `--status draft` with the extra
tag `faq:unvetted`, so unreviewed knowledge is never presented as settled.
Interactive FAQ captures land `completed` (Step 5). trace:STORY-1643 | ai:claude

An un-annotated prompt defaults to `design-fork` (pause-safe). Author
guidance: `.aida/discipline/skill-prompt-kinds.md`. trace:TASK-297

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

**FAQ lens** — also look for knowledge worth keeping as an FAQ:
- A question the user asked that needed investigation to answer
- A surprising behaviour that was explained (root cause found)
- A convention or rule agreed in conversation
- A workaround for an open bug
- A "how do I...?" answered with concrete commands

### Step 2: Check Against Database

For each finding, check if it already exists:

```bash
aida search "<keyword>"
```

For FAQ candidates, search both lenses — the default FAQ list hides
completed entries, and nearly every FAQ is `completed`:

```bash
aida search "<words from the question>"
aida faq list completed
aida faq list completed --tags topic:<t>   # the candidate's topic tag(s)
```

If an existing FAQ already covers it, the action is **update** (edit its body
or add a comment), never a duplicate.

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

### FAQ candidates
- <The question, as someone would search for it?> — <one-line answer>
- (update) <existing FAQ title> — <what changes>
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

For FAQ candidates, follow the FAQ convention (`aida search "convention for
writing and filing an FAQ"`):

- **Title** = the question, worded the way someone would search for it.
- **Body** (markdown): a bold one-line answer first, then `## Why`, `## How`,
  and `## See also` (spec IDs). Add `## Caveat` when the answer depends on an
  open bug.
- **Tags**: 1–2 `audience:` tags (`audience:user`, `audience:dev`) and 1–3
  `topic:` tags from the vocabulary in that FAQ's `## Tags` section; add a
  new topic only if none fits.

```bash
cat > /tmp/faq-answer.md <<'MD'
**One-line answer.**

## Why
...

## How
...

## See also
SPEC-ID
MD
aida faq add --title "How do I ...?" --description-from-file /tmp/faq-answer.md \
  --tags "audience:user,topic:drain" --status completed
# zen/headless: --status draft and add faq:unvetted to --tags
```

`aida faq add` prints the FAQ's short `FAQ-N` id at creation; no merge-gate
step is needed. When one new FAQ references another, add the referenced FAQ
first and cite the id it printed. Never cite a guessed `FAQ-N`.

## CLI Reference

```bash
# Search for existing requirements
aida search "<keyword>"

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
- An FAQ answer must be verified in this session (a command was run, the code
  was read). Leave unverified answers out, or file them with a `## Caveat`
  saying what is unconfirmed.
- `/aida-doc` captures narrative (the WHY behind a spec); FAQs belong here.
