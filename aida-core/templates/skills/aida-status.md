---
name: aida-status
description: One-shot project status — requirement breakdown, cache freshness, sync state, recent activity, and (when working on AIDA itself) build/release context. Read-only; surfaces what `/aida-onboard` would dig deeper on.
allowed-tools:
  - Bash
---

# AIDA Status Skill

## Purpose

Show the user where the project stands in one screen: how many
requirements exist by status, whether the cache is fresh, whether the
orphan store is in sync with origin, and what was touched recently.
Equivalent to opening a dashboard before starting a work session.

## When to use

- The user asks "how's the project?" or "what's the state?" and wants
  a snapshot without drilling in
- Start of a session when the user hasn't said where to focus yet
- After a long break, to surface what changed since last session
- Before cutting a release, to confirm sync + cache + recent commits

## Skip if

- The user already knows what they want to do — go straight to that
  task, status output is just noise
- The user is asking about a single requirement (use `aida show <id>`
  or `/aida-search`)
- The user is asking about queued work (use `/aida-queue`)

## Snapshot

!`aida status 2>/dev/null || echo "(not in an AIDA project — nothing to report)"`

## Workflow

### Step 1: Read the status sections

`aida status` returns several sections; surface the ones that matter
for the user's current situation rather than reciting the whole dump:

- **Project**: name, mode (centralized vs distributed), store path
- **Requirements**: total + breakdown by status. Big numbers in `Draft`
  or `Approved` mean unstarted backlog; big `InProgress` means work
  in flight; big `Done` means work finished on branches but not yet
  merged to main (STORY-86 — `aida pull` auto-bumps Done→Completed
  on merge); big `Rejected` means historical noise (usually fine).
- **Cache**: `FRESH` is good. `STALE`/`MISSING` means
  `aida cache rebuild` is overdue; offer to run it.
- **Sync**: "N ahead of origin" means a `git push origin aida-store`
  is pending. "N behind" means `aida db sync --pull` is pending.
- **Recent activity**: last few completed/in-progress items. Useful for
  "where did I leave off?"
- **AIDA development context** (only when in this repo): the running
  binary version, workspace version, ahead-of-tag count, template
  symlink health, release-readiness verdict.

### Step 2: Highlight anything that needs action

If any of these are true, raise them explicitly — the user opened
`/aida-status` to spot exactly this kind of drift:

- Cache is stale → suggest `aida cache rebuild`
- Sync is behind → suggest `aida db sync --pull`
- Sync is ahead → suggest `git push origin aida-store`
- Template symlinks broken (AIDA dev only) → suggest
  `make sync-templates`
- Many `Draft` items piling up → suggest `/aida-evaluate` or
  `aida list --status draft` for triage

### Step 3: Offer follow-ups

After the snapshot, offer the natural next moves without forcing one:

- `/aida-onboard` — full project orientation (deeper than status)
- `/aida-search <topic>` — drill into a specific area
- `/aida-pickup` — grab the next queued item
- `/aida-capture` — review the conversation for unfiled requirements
- `aida list --status approved --priority high` — what's ready to start

## Empty / no-AIDA case

If the status command says "not in an AIDA project" or similar:

> No AIDA store here. Run `aida init` to start, or `cd` to a project
> that already has one.

## Notes

- `aida status` is read-only; this skill never mutates state. Any
  suggested commands (cache rebuild, push, sync) are surfaced as
  recommendations for the user, not auto-run.
- The output is multi-section and verbose by design — don't copy it
  verbatim into the response, summarize what's notable.
