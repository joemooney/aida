# Tech-Debt Scan

Run a read-only end-of-session sweep for duplication and code debt, then surface findings with a recommendation and filing verb each.

## Instructions

Follow the workflow in `.claude/skills/aida-techdebt.md`:

1. Scope the scan to the session's diff first (`git diff --name-only main...HEAD`), not the whole repo, unless the user asks for a full sweep
2. Run the five scans: duplicated code blocks, copy-pasted trace comments, dead trace paths (`trace:` → Rejected spec), spec-graph duplicates, orphan files
3. Present one consolidated report, severity-ordered — dead trace paths and code duplication first, orphan files last (noisiest)
4. Give every finding a one-line recommendation
5. Compose with capture: `aida findings add` for observations to triage later, `aida add --type task` for endorsed cleanup, `aida edit <dupe> --status rejected` to retire a spec-graph duplicate

Read-only by default — find and recommend; never delete code or flip specs unprompted. Use at session end, or when the user says "techdebt", "find duplication", or "clean up".
