---
description: Run /aida-fasttrack.
---
# Fasttrack a trivial change

Follow `.claude/skills/aida-fasttrack/SKILL.md`:

1. Read `$ARGUMENTS` and confirm the work is cosmetic, documentation-only, or
   one obvious line; bounded, reversible, low blast radius, and unambiguous.
2. If it is not genuinely trivial, file it normally. Use
   `execution_mode=drain` for full autonomous execution with normal gates.
3. File eligible work with `aida fasttrack "<description>" --type <task|bug>`.
   This creates Approved + queued work tagged `batch:fasttrack` and
   `lifecycle:no-review`.
4. Implement on a fresh branch, run the reduced local gate, and merge only
   after CI succeeds.
5. If the work grows beyond trivial, punt it, file a finding, and remove the
   fasttrack tags.

`--express` is a deprecated one-release alias for Approved + queued intake with
`execution_mode=drain`; prefer normal intake with `--mode drain`.

<!-- trace:TASK-1267 | ai:codex -->

ARGUMENTS: $ARGUMENTS
