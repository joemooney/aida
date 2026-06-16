---
name: aida-implement
description: Implement an approved requirement with full traceability. Use when user wants to implement a feature, fix a bug, or work on a requirement.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
  - Edit
  - Write
  - Glob
  - Grep
---
<!-- AIDA Generated: v2.0.0 | checksum:d2ab78f7 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Implementation Skill

## Purpose

Implement an approved requirement with full traceability, evolving the requirement database to capture implementation details and creating child requirements as needed.

## When to Use

Use this skill when:
- User says "implement <SPEC-ID>" or "work on <requirement>"
- An approved requirement is ready to be implemented
- Continuing implementation of a requirement from a previous session

## Core Principles

### Living Documentation
The requirements database should evolve during implementation to accurately reflect:
- What was actually built (vs. what was initially specified)
- Implementation decisions and trade-offs
- Child requirements discovered during development
- Technical constraints encountered

### Traceability
All AI-generated code must include inline traceability comments linking back to requirement IDs.

### AI Authorship Attribution
When adding requirements or comments via the CLI, authorship should reflect AI assistance.

**Set the AIDA_AUTHOR environment variable:**
```bash
export AIDA_AUTHOR="ai:claude:$USER"
```

This ensures all `aida add` and `aida comment add` commands automatically use the AI author format.
Format: `ai:<tool>:<username>` (e.g., `ai:claude:joe`)

## Approved Requirements

!`aida list --status approved 2>/dev/null | head -15 || echo "none"`

## Autonomy mode — `$AIDA_ZEN` (STORY-287)

This skill's user-facing prompts carry a `kind:` annotation in an HTML
comment directly above each one:

- `<!-- kind:confirmation -->` — a mechanical yes/no whose default
  (option 1) is obvious.
- `<!-- kind:design-fork -->` — a genuine choice between meaningful
  alternatives, where guessing wrong has real cost.

Before surfacing any prompt, check the autonomy mode:

```bash
aida zen status
```

- **`zen`** — *advisor-on-standby* mode, **corroborated** (`aida queue
  work --zen`, or a live `--auto-complete --zen` orchestrator).
  Auto-resolve every `kind:confirmation` prompt to option 1 and proceed,
  printing `↳ zen: auto-resolved "<prompt>" → option 1`. Still surface
  every `kind:design-fork` prompt unchanged — implementation approach
  decisions are exactly what the advisor stays at the keyboard for. **The
  moment you surface a `kind:design-fork` prompt to the standby advisor (or
  raise a punt), record it** so the end-of-session finish checkpoint knows a
  human was in the loop and pauses instead of auto-ending (STORY-564):
  ```bash
  aida zen needs-human --reason "<the fork, one line>"
  ```
  A `--zen` session that only auto-resolves `kind:confirmation` prompts
  never marks itself — and so `aida zen finish` lets it auto-exit cleanly.
  trace:STORY-564
- **`interactive`** — default mode: surface every prompt, no change.
  `aida zen status` prints `interactive` whenever zen is off *or*
  `AIDA_ZEN=1` is set but its provenance cannot be corroborated — a
  stale / leaked `AIDA_ZEN=1` never silently enables zen (BUG-237).
  Branch off this word, **not** the bare `$AIDA_ZEN` env var. trace:BUG-237

A headless `--no-human` drain (`AIDA_HEADLESS=1`) is the stronger mode and
overrides `--zen`. An un-annotated prompt defaults to `design-fork`
(pause-safe). Author guidance: `docs/aida/discipline/skill-prompt-kinds.md`.
trace:STORY-287

**Graceful exit under the orchestrator (TASK-329).** If this skill runs
inside an `aida queue work --auto-complete` session and `$AIDA_EXIT_SENTINEL`
is set, then under `$AIDA_ZEN` (or a headless drain) — once every commit, PR,
and comment is done and there is no hand-off to another skill — the
**absolute last action of the session** is:

```bash
[ -n "${AIDA_EXIT_SENTINEL:-}" ] && touch "$AIDA_EXIT_SENTINEL"
```

The orchestrator polls for that file and reaps the otherwise-idle REPL (a
skill cannot synthesize the Ctrl+D it would press interactively — BUG-230).
Touch it **once, last**; skip it entirely in default interactive mode. Full
protocol: `docs/aida/discipline/skill-prompt-kinds.md`. trace:TASK-329

Key this off `$AIDA_EXIT_SENTINEL` being set — a per-session absolute path
the orchestrator minted — not the bare `AIDA_AUTO_COMPLETE` env var. If you
ever need the corroborated orchestrator verdict explicitly, run
`aida orchestrator status` (`orchestrated` only when a live orchestrator run
owns the session — BUG-233); never trust `AIDA_AUTO_COMPLETE` on its own.
trace:BUG-233

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

### Step 2: Analyze Implementation Scope

Before writing code:
1. Identify files that will be created or modified
2. Identify any sub-tasks or child requirements
3. <!-- kind:design-fork --> Confirm approach with the user when there
   are significant decisions — a real choice between approaches with
   meaningful trade-offs. This is a `design-fork` prompt: surface it even
   under `$AIDA_ZEN` (advisor-on-standby still wants the real questions).

If the requirement is too broad, suggest splitting:
```bash
# Create child requirements (NOTE: use --tags not --tag)
aida add --title "..." --description "..." --type functional --status draft --tags "comma,separated"

# Link as child
aida rel add --from <PARENT-ID> --to <CHILD-ID> --type Parent
```

### Step 3: Implement with Traceability

When writing or modifying code, add inline traceability comments:

**Generic (use language-appropriate comment syntax):**
```
// trace:<SPEC-ID> - Feature title | ai:claude:high | impl:2025-12-10 | by:joe
// Your implementation here
```

**Comment Format:**
```
// trace:<SPEC-ID> - <title> | ai:<tool>:<confidence> | impl:<date> | by:<user>
```

Where:
- `<SPEC-ID>`: The requirement being implemented (e.g., FR-0100)
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
aida comment add <SPEC-ID> "Implementation note: Used async/await pattern for..."

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
aida comment add <SPEC-ID> "Implementation complete. Files modified: src/foo.rs, src/bar.rs"
```

3. Create "Verifies" relationship if tests were added:
```bash
aida rel add --from <TEST-SPEC-ID> --to <SPEC-ID> --type Verifies
```

4. **Under a headless drain (`AIDA_HEADLESS=1`)** — file conversational
   flags raised at the end of the spec (a deviation from the acceptance
   criteria, a non-obvious design call, a pre-existing bug spotted, a
   follow-up suggestion) as draft `from-implementer:<SPEC-ID>` findings so
   they reach the advisor instead of vanishing into conversation history.
   The full procedure — categories, severity rubric, idempotency probe — is
   `/aida-pickup` Step 5b; the queue-driven pickup loop is the canonical
   home for it. In an interactive session a human reads the flags directly;
   skip this step. trace:STORY-285

5. **Finish-state communication (TASK-359).** When this skill is the
   skill that ends the session — either a structured *"how should I
   finish?"* menu or a closing summary block under autonomous drive —
   apply the six-element finish-state rubric: labelled **State
   snapshot**, **deciding factor** when one is in play, explicit
   **recommendation + rationale** (or **`→ Next:` line** naming the
   user-action on a closing summary), **per-option drain-state +
   reversibility**, an **advise escape** when the call is genuinely
   ambiguous, and **decoupled coupled decisions** (push/PR is one
   prompt, followup-filing the next — never bundled). Silence on the
   next user-action is not acceptable: a closing summary must name
   *"→ Press Ctrl+D to advance the orchestrator"* or *"→ session will
   auto-exit; nothing else needed"* explicitly. Full rubric:
   `docs/aida/discipline/session-discipline.md` § *Finish-state
   communication rubric*. The worked templates live in `/aida-pickup`
   Step 6 (menu) and `/aida-pr` orchestrator-mode (closing block).
   trace:TASK-359

### Step 7: Exit after `aida pr ship` (or `aida queue done`) — do NOT linger watching CI

**Your work ends the moment `aida pr ship` returns zero (or `aida queue
done` if you intentionally stopped one step earlier).** Both commands
print a loud "IMPLEMENTER COMPLETE — EXIT NOW" banner at success — that
banner is the substrate's signal that the implementer Claude has
nothing left to do. Read it and exit. trace:BUG-376 | ai:claude

Do **NOT**, after a successful `aida pr ship`:

- Watch CI further (`gh pr checks <N> --watch`, `gh run watch …`) — CI
  already ran inside `aida pr ship` step 2 *before* the merge; it is
  green by the time you see the banner. Re-watching it is theatre.
- Wait for the merge to land — `aida pr ship` step 3 already merged it.
- Run `aida pull` to verify the auto-bump fired — `aida pr ship` step 4
  already ran `aida pull`.
- Run `aida status` / `aida session leases` to confirm the lease
  cleared — `aida pr ship` step 5 already ran `aida session end`.
- "Helpfully" stay around in case something needs follow-up — the
  orchestrator (under `--auto-complete`), the reviewer (under a
  separate review session), or the next-phase agent owns everything
  that happens after the implementer's chair empties. Lingering only
  burns operator attention and forces a manual Ctrl-D.

After `aida queue done` (without a subsequent `aida pr ship`): the same
rule applies — the spec is on the branch, the queue position is closed,
and whichever caller spawned this implementer (orchestrator, interactive
shell, drain) owns the next phase. Print one closing line naming
*"→ Press Ctrl+D to exit"* and exit. Under `$AIDA_EXIT_SENTINEL` the
sentinel touch from Step 6 of the autonomy-mode section is the
machine-readable equivalent — do that and exit; never both poll CI
*and* touch the sentinel.

The motivating incident is BUG-376: an interactive implementer ran
`aida pr ship` correctly, then said *"Next action: watch CI on PR-296
and merge when green"* and lingered. The PR was already merged; the
lease was already released; the only thing left was the Ctrl-D the user
then had to type twice manually. Don't reproduce that shape.

### Substrate-as-bouncer for pending briefs (BUG-378)

`aida queue done` and `aida edit --status done|completed` now scan the
brief surface (`.aida/agent-briefs/<your-agent-type>/`) and print a loud
`NEW BRIEF(S) PENDING` banner to stderr if work is queued for your agent
type. **If you see that banner, read the listed brief file(s) before
exiting — even if your local scratchpad / task.md says "all done."** Your
internal session state is a private draft, not ground truth; the brief
surface is the canonical pickup queue. The motivating incident is the
scratchpad-drift loop where an agent re-reads its own `task.md` and keeps
re-rendering "all shipped" while a new brief sits unread. trace:BUG-378

## State Transitions

During implementation, requirements should transition through:

1. **Approved** -> **In Progress** (when starting implementation)
2. **In Progress** -> **Done** (work finished on a branch — set by
   `aida queue done` automatically)
3. **Done** -> **Completed** (auto-bumped by `aida pull` /
   `aida db sync --pull` when the referencing commit lands on the
   default branch — no manual step required)
4. **In Progress** -> **Draft** (if significant changes needed)

STORY-86: Don't set `--status completed` manually from a feature
branch — that bypasses the "merged to main" gate. Use `--status done`
or `aida queue done` and let auto-bump promote it once the PR merges.

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
aida comment add <SPEC-ID> "Comment text"

# Add relationship
aida rel add --from <FROM-ID> --to <TO-ID> --type <Parent|Verifies|References|Duplicate>

# Create new requirement (NOTE: use --tags not --tag)
aida add --title "..." --description "..." --type <type> --status draft --tags "comma,separated"

# List requirements by feature
aida list --feature <feature-name>
```