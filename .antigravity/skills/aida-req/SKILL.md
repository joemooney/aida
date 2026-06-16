---
name: aida-req
description: Add a new requirement to the AIDA database with AI evaluation. Use when user wants to create a spec, add a feature request, or capture an idea.
allowed-tools:
  - Bash
  - Read
---
<!-- AIDA Generated: v2.0.0 | checksum:b7056ac4 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Requirement Creation Skill

## Purpose

Add a new requirement to the AIDA requirements database with AI-powered evaluation feedback.

## When to Use

Use this skill when:
- User wants to add a new requirement or feature request
- User describes something they want the system to do
- User has an idea that should be captured as a requirement
- User asks to "add a requirement" or "create a spec"

## Current Project Context

- Recent specs: !`aida list 2>/dev/null | head -15 || echo "none yet — run 'aida init' first"`
- Still to do: !`aida list open 2>/dev/null | head -10 || echo "none"`

## Workflow

### Step 1: Gather Requirement Information

Ask the user for the following information (in conversational style):

1. **Description** (required): What should the system do? This can be:
   - A formal requirement: "The system shall..."
   - A question or idea to be formalized
   - A rough note that needs refinement

2. **Type** (optional, default: functional):
   - `functional` (FR) - System behaviors and features
   - `non-functional` (NFR) - Quality attributes (performance, security)
   - `user` (UR) - User needs/goals
   - `system` (SR) - Technical constraints

   Note: Use lowercase with hyphen for non-functional. "Feature" is NOT a type -
   use `--feature <name>` to assign a feature category.

3. **Priority** (optional, default: Medium):
   - High, Medium, Low

4. **Feature** (optional): Which feature area does this belong to?

5. **Tags** (optional): Comma-separated keywords

### Step 2: Add Requirement to Database

Use the `aida` CLI to add the requirement immediately:

```bash
aida add \
  --title "<generated-title>" \
  --description "<user-description>" \
  --type <type> \
  --priority <priority> \
  --status draft \
  --feature "<feature>" \
  --tags "<tags>"
```

**Title Generation**: Generate a concise title (5-10 words) from the description that captures the essence of the requirement.

### Step 3: Show Confirmation

After adding, display:
```
Requirement added: <SPEC-ID>
Title: <title>
Status: Draft (evaluation pending...)
```

### Step 4: Run AI Evaluation

Evaluate the requirement quality using the AI evaluation prompt. The evaluation should assess:

1. **Clarity** (1-10): Is the requirement clear and unambiguous?
2. **Testability** (1-10): Can this requirement be verified?
3. **Completeness** (1-10): Does it include all necessary information?
4. **Consistency** (1-10): Does it conflict with other requirements?

Provide:
- Overall quality score
- Issues found (if any)
- Suggestions for improvement
- Whether this should be split into multiple requirements

### Step 5: Offer Follow-up Actions

Based on the evaluation, offer:
- **Improve**: Let AI suggest improved description text
- **Split**: Generate child requirements if too broad
- **Link**: Suggest relationships to existing requirements
- **Accept**: Keep as-is and approve

### Step 6: Verify Acceptance Criteria Against the Primary Caller(s)

Before treating a spec's acceptance criteria as done, **name the primary
caller(s)** — the feature's top 1-3 invocation paths:

- user-typed CLI in an interactive terminal (TTY)
- a skill invoked via Claude Code's Bash tool (**always non-TTY**)
- a git or Claude Code hook (non-interactive)
- the MCP server (a different surface than the CLI)
- a headless drive (`claude -p`) vs an interactive session

For **each acceptance criterion**, ask: *does this criterion hold in those
caller environments?* A criterion is **environment-coupled** when its truth
depends on the runtime context — TTY vs piped stdout, headless vs
interactive, first-time vs returning user, solo vs multi-node, same vs
cross-worktree. If a criterion would *degrade the feature in its primary
caller's environment*, the criterion is wrong — fix it **before filing**, not
at implementer design-checkpoint.

Worked failure: a criterion reading *"non-TTY mode degrades to a single-line
summary"* on a feature whose primary caller is a skill (always non-TTY) would
make the feature never render in its own main use case. Naming the caller
first catches that at filing time.

See `docs/aida/discipline/session-discipline.md` →
"Verify acceptance criteria against the primary caller" for the full rule and
the four worked examples that surfaced it.

## CLI Reference

```bash
# Add requirement (NOTE: use --tags not --tag)
aida add --title "..." --description "..." --type functional --priority high --status draft --tags "comma,separated"

# Show requirement details
aida show <SPEC-ID>

# Edit requirement
aida edit <SPEC-ID> --description "..."

# See what's still to do
aida list open
```

## Integration Notes

- Requirements are stored git-canonically: one YAML file per spec on the `aida-store` orphan branch, with a rebuildable `.aida/cache.db` read cache
- SPEC-IDs are auto-generated based on type prefix configuration