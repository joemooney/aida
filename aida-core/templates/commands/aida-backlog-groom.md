# Groom the Backlog

Curate Approved-but-not-queued work onto the queue with risk + file-overlap heuristics.

## Instructions

Follow the workflow in `.claude/skills/aida-backlog-groom.md`:

1. `aida backlog list --json` (filter knobs: `--risk`, `--type`, `--tag`, `--tag-prefix`, `--priority`, `--limit`)
2. Spot the cluster shape — low-risk tasks/docs cluster well; structural specs do not
3. `aida backlog analyze --specs <ids> --json` over the short-list (safe-parallel vs serialize vs unknown)
4. Render a short table summarizing the proposal; use `AskUserQuestion` to multi-select + pick a `batch:NAME`
5. `aida backlog groom --specs <csv> [--batch NAME] [--dry-run]` — refusal is the whole list, no half-applied state
6. `aida queue list --batch NAME` to confirm what's drain-ready

Use when the Approved pile has grown past what's easy to scan, or you want to feed `aida queue work --batch NAME --auto-complete`.

ARGUMENTS: $ARGUMENTS
