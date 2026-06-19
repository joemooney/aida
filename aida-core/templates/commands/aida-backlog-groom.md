---
description: "Guided burndown-prep: one advisor pass from approvable drafts and Approved-but-not-queued specs to a blessed, drain-ready queue."
---
# Groom the Backlog

Guided burndown-prep: one advisor pass from approvable drafts and Approved-but-not-queued specs to a blessed, drain-ready queue.

## Instructions

Follow the workflow in `.claude/skills/aida-backlog-groom.md`:

Burndown-prep surfacing + approval (start here for a full prep):

- A. Surface BOTH buckets: `aida list draft` (approvable drafts) and `aida burndown plan --candidates` + `aida backlog list` (approved-not-queued).
- B. Approve/select per item — NEVER auto-approve. Promote a clear draft with `aida edit <ID> --status approved`; route a draft that needs a human decision to `aida questions sweep --apply` / `aida questions ask` / `aida questions clarify` (it stays parked, NOT approved).

Then groom the chosen Approved set onto the queue (reuse, don't reimplement):

1. `aida backlog list --json` (filter knobs: `--risk`, `--type`, `--tag`, `--tag-prefix`, `--priority`, `--limit`)
2. Spot the cluster shape — low-risk tasks/docs cluster well; structural specs do not
3. `aida backlog analyze --specs <ids> --json` over the short-list (safe-parallel vs serialize vs unknown)
4. Render a short table summarizing the proposal; use `AskUserQuestion` to multi-select + pick a `batch:NAME`
5. `aida backlog groom --specs <csv> [--batch NAME] [--dry-run]` — refusal is the whole list, no half-applied state
6. `aida queue list --batch NAME` to confirm what's drain-ready, then NAME the exact drain command for the blessed set (`aida queue work --batch NAME --auto-complete`, or `/aida-burndown`).

Use to prep before a burndown, when drafts have piled up, or when the Approved pile has grown past what's easy to scan.

ARGUMENTS: $ARGUMENTS
