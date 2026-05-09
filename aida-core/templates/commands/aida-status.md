# Project Status

One-shot snapshot of the project: requirement breakdown, cache
freshness, sync state, recent activity.

## Snapshot

!`aida status 2>/dev/null || echo "(not in an AIDA project)"`

## Instructions

Follow the workflow in `.claude/skills/aida-status.md`:

1. Read the snapshot above and surface the sections that matter — don't
   recite the whole dump
2. Highlight anything that needs action (stale cache, unpushed orphan
   commits, broken template symlinks)
3. Offer follow-ups: `/aida-onboard` for a deeper orientation,
   `/aida-pickup` to start the next queued item, `/aida-search` to drill
   in
