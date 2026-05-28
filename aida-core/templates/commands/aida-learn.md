# Capture a Rule from a Mistake

Capture a rule, lesson, or convention from a mistake — substrate-aware "update CLAUDE.md so you don't repeat this".

## Instructions

Follow the workflow in `.claude/skills/aida-learn.md`:

1. Surface the specific mistake (what happened, what should have happened)
2. Classify the rule's scope:
   - Project-wide → `CLAUDE.md`
   - General AIDA → `docs/aida/discipline/<topic>.md`
   - User-personal preference → `~/.claude/projects/<slug>/memory/feedback_*.md`
   - Pattern observation awaiting recurrence → `aida findings add`
3. Search for existing related rules — extend rather than duplicate
4. Write the rule in three parts: rule (imperative), why (invariant), how to apply (trigger)
5. Capture a session trace back (date + context)
6. Commit (project-scoped rules) or write to memory dir (personal rules)
7. Confirm to the operator where the rule landed

Use when:
- "Update CLAUDE.md so you don't repeat this"
- "Remember this"
- "I keep having to tell you this"
- Reviewer flags a recurring convention drift
- Claude makes a mistake the operator catches
