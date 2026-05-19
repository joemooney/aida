# Punt A Design-Fork

Pause a spec in Needs Attention when you hit a decision you cannot safely
make — the honest alternative to guessing during an autonomous drain.

## Instructions

Follow the workflow in `.claude/skills/aida-punt.md`:

1. Confirm it is a real fork — re-read the spec, its `## Acceptance`, parent,
   and any owning plan. Punt only a decision you genuinely cannot make.
2. Classify the obstacle by *observable* category: `design-fork`,
   `ambiguous-spec`, `missing-context`, `blocked-dependency`, `other`.
3. Punt it:
   ```bash
   aida punt <SPEC-ID> --category <category> \
     --reason "<the fork and the options>" \
     --lean "<best guess if forced — optional>"
   ```
4. Stop working that spec — do not commit a guess. Control returns; an
   orchestrator advances, and the punt surfaces in `aida findings` for triage.

Use when an autonomous (or interactive) implementer/reviewer hits a
design-fork it cannot resolve from the spec, the codebase, or recorded
project conventions.
