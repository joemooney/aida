---
description: Open work for independent review via aida ship
---

# AIDA PR compatibility command

<!-- trace:TASK-1613 | ai:codex -->

Follow `.claude/skills/aida-pr/SKILL.md`. Check `aida orchestrator status`
first: in corroborated `orchestrated` mode the parent owns preflight and PR
creation, so commit and push the branch without opening a PR (`aida ship <SPEC> --no-pr`),
then exit — in headless/zen mode run `[ -n "${AIDA_EXIT_SENTINEL:-}" ] && touch "$AIDA_EXIT_SENTINEL"`
as the absolute last action instead of the publication sequence below. For an
authorized open-for-review handoff outside `orchestrated` mode:

```bash
aida ship <SPEC> --no-merge --dry-run
aida ship <SPEC> --no-merge
aida pr auto-queue-review
```

Only autoqueue after successful PR creation/reuse; `--no-merge` does not queue
review automatically. Report the actual autoqueue outcome, including skips
and failures (and in headless mode, touch `$AIDA_EXIT_SENTINEL` last if set).
Preserve a clear PR description, linked specs, and validation
evidence; creation derives title/body from the latest commit.

Full `aida ship` continues through CI, merge, sync, and cleanup and requires
an authorized finish-and-merge path with independent review satisfied.
`aida pr ship` is its lower-level merge tail, not the review-handoff command.
Do not self-merge by default or mark Completed before merge. Honor draft-only
policy (`--no-merge` does not create a draft), supervised judgment, and managed/
headless orchestrator stop/handoff restrictions. The legacy skill options and
manual lifecycle ceremony are retired; use `aida ship --help` for CLI options.
