---
name: aida-learn
description: Capture a rule, lesson, or convention from a mistake (or about-to-be mistake) and route it to the right AIDA substrate. Use when Claude does something wrong, when a reviewer flags a recurring pattern, or when the operator says "remember this", "update CLAUDE.md so you don't repeat this", or "add a rule".
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
  - Write
  - Edit
  - Grep
---

# AIDA Learn Skill — capture a rule from a mistake

## Purpose

This is the substrate-aware version of Boris Cherny's "update CLAUDE.md so you do not repeat this" pattern. When Claude makes a mistake, an operator notices a recurring papercut, or a reviewer flags a convention drift, this skill captures the **rule** and routes it to the right home in AIDA's substrate — not just a single `CLAUDE.md` dump file.

## When to use

- Operator says "remember this", "update CLAUDE.md so you don't repeat this", "add a rule", or "I keep having to tell you this"
- Claude just made a mistake the operator caught
- A reviewer comment names a recurring convention drift
- A session-end retrospective surfaces a pattern that wasn't quite worth filing as a bug

## Substrate routing — where the rule belongs

AIDA has four distinct substrates for rules. Pick by **scope and audience**:

| Substrate | Scope | When to use |
|---|---|---|
| `CLAUDE.md` (project root) | Project-wide convention every session needs | "Always use `bun`, not `npm`" / "Open PRs against main; never push direct" |
| `docs/aida/discipline/<topic>.md` | General AIDA-using guidance, beyond this project | Patterns like "verify substrate state before declaring complete", "use `--force-claim` when you mean to take over" |
| `~/.claude/projects/<slug>/memory/feedback_<topic>.md` | User-personal preference / interaction style | "I prefer concise terminal output", "Don't suggest sleep / wellness framing" |
| `aida findings add` (observation) | Recurring pattern not yet rule-shaped; awaits recurrence-3 promotion | "Saw X behavior twice; if it happens again, it's a real bug" |

## Workflow

### Step 1: Surface the mistake

Operator describes what went wrong, or you (Claude) name what you just did wrong. Be concrete:

- What was the desired behavior?
- What actually happened?
- What context made the wrong path tempting?

### Step 2: Classify the rule's scope

Ask yourself:
- Is this **THIS PROJECT'S** convention, or **how to use AIDA in general**?
- Is this a **technical rule** (about code, tools, commands) or a **personal preference** (about interaction style)?
- Has this happened **once** or **multiple times**?

Then route:

```
THIS PROJECT, technical          → CLAUDE.md
GENERAL AIDA, technical          → docs/aida/discipline/<topic>.md
USER-PERSONAL, any kind          → ~/.claude/projects/<slug>/memory/feedback_*.md
ONCE-OFF observation             → aida findings add (let recurrence promote)
```

### Step 3: Check for existing related rules

Before writing a new rule, search for related ones:

```bash
# Project CLAUDE.md
grep -in "<keyword>" CLAUDE.md

# Discipline pack
grep -rn "<keyword>" docs/aida/discipline/

# Memory pack
grep -rn "<keyword>" ~/.claude/projects/$(basename "$(pwd)")/memory/

# Findings
aida findings list | grep -i "<keyword>"
```

If a related rule exists, **edit/extend it** instead of creating a duplicate. Cross-link related rules with `[[name]]` syntax (for memories) or relative paths (for discipline pack).

### Step 4: Write the rule

A good rule has three parts:

1. **The rule** — one line, imperative voice ("Always X" / "Never Y" / "When X, do Y")
2. **Why** — the underlying invariant or the specific failure mode (one sentence)
3. **How to apply** — concrete trigger for next time (one or two lines)

Example (project CLAUDE.md):
```
- **Never use `npm`** — this project uses `bun` exclusively. The npm lockfile drifts from bun's resolution and breaks CI. When you see `package.json`, default to `bun install` / `bun run <script>`.
```

Example (discipline pack):
```
### Verify before declaring complete

Never declare a spec done before:
1. The commit is on `main` (not just locally committed)
2. The PR (if any) is merged or the spec doesn't need one
3. `aida show <SPEC>` shows the expected linkage

The "I ran the tests, looks good" claim isn't substrate-grounded. Check the substrate.
```

Example (feedback memory — frontmatter included):
```markdown
---
name: feedback-prefer-concise-output
description: User prefers concise terminal output; verbose framing is friction
metadata:
  type: feedback
---

**Rule:** Default to concise terminal output. Skip prose framing around tool calls; let the result speak.

**Why:** User runs many parallel sessions; verbose padding compounds. Confirmed 2026-05-28 ("stop adding wellness commentary").

**How to apply:** When in doubt, ship the answer and stop. If context is needed, one short sentence beats a paragraph.
```

### Step 5: Capture the trace

Always include a session trace back so future-you can find where the rule came from:

- For CLAUDE.md / discipline pack rules: append `_(filed 2026-MM-DD from <SPEC-ID-if-relevant> / <one-line context>)_`
- For feedback memories: include the date and a one-line context in the body
- For findings: the substrate captures `from-advisor:<origin>` automatically

### Step 6: Commit the rule (project-scoped only)

For CLAUDE.md and `docs/aida/discipline/` changes — these are project artifacts that should ride to the team:

```bash
git add CLAUDE.md docs/aida/discipline/<file>.md
git commit -m "[AI:claude] docs: capture rule — <one-line rule> (TASK-N)"
```

For memory pack files — these are user-personal, do NOT commit to the project repo. They land in `~/.claude/projects/<slug>/memory/` automatically (gitignored).

For findings — `aida findings add` writes to the substrate; no manual commit needed.

### Step 7: Confirm with the operator

Print a one-line summary of where the rule landed:

```
✓ Rule captured: <one-line rule>
  → <path or finding id>
```

So the operator can verify it landed in the right place.

## Anti-patterns to avoid

- **Don't dump everything in CLAUDE.md.** Project-wide rules only. Recurring user preferences belong in feedback memories. General AIDA patterns belong in the discipline pack.
- **Don't create one-off memories.** A memory file with one rule that won't generalize is noise. Either roll it into an existing memory or capture as a finding awaiting recurrence.
- **Don't restate rules already in the discipline pack or CLAUDE.md.** Search first.
- **Don't write rules in the voice of explanation.** "It would be nice if Claude did X" is not a rule; "Always do X" is.
- **Don't capture transient incident notes as rules.** "The CI was flaky on 2026-05-28" is not a rule. "When CI is red, push an empty commit before retry — `gh run rerun` doesn't pick up new commits" IS a rule.

## See also

- `/aida-capture` — end-of-session sweep for requirements (specs, not rules)
- `aida findings add` — capture a pattern observation awaiting recurrence
- `docs/aida/discipline/README.md` — the discipline pack you're updating
