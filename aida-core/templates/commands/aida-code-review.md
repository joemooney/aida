# Code Review

Exhaustive code quality review with a structured report and before/after diffs.

## Instructions

Follow the workflow in `.claude/skills/aida-code-review.md`:

1. Determine review scope (current branch diff, a specific path, or recent commits)
2. Check for sloppy code, untested paths, missing trace comments, excessive complexity, inconsistent style
3. Produce a structured report grouped by severity, with file:line references
4. Provide concrete before/after diffs for each finding
5. Offer to open requirements for issues that warrant follow-up work

Use when the user asks for a quality review of changed code or a specific area.
