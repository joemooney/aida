---
name: aida-rebase
description: Detect whether the current branch has drifted from its upstream and classify the rebase (clean / ahead-only / behind-only / diverged-safe / diverged-risky) before deciding whether to rebase. Thin wrapper around the `aida rebase` CLI. Use before committing, before opening a PR, when picking up a session, or whenever the user mentions rebase/pull/push/"is this up to date?".
allowed-tools:
  - Bash
  - Read
---

# AIDA Rebase Skill

## Purpose

Surface branch drift and rebase safety *before* the user acts on it —
turning "fetch before you commit" discipline into a reflex.

## When to Use — proactive-invocation playbook (TASK-105)

`/aida-rebase` being available is necessary but not sufficient: the
agent needs clear heuristics for when to fire it *without being asked*.
Run the `--dry-run` probe (zero side effects) on any of these triggers
and surface the result:

| Trigger | Action |
|---|---|
| About to commit and the session has been open >15 min | `aida rebase --dry-run --json`; if behind, surface before committing |
| User says any of: "rebase", "pull", "push", "is this up to date?", "let's ship", "ready to merge" | Run the dry-run probe inline, surface the classification |
| About to open a PR (`/aida-pr`) | Dry-run probe — catch staleness before review, not after |
| Long session resumes after 30+ min idle | Optional re-check — catches drift over breaks |
| A long build/test just finished | Defer the surface until the critical-path task completes, *then* suggest |

### What does NOT trigger /aida-rebase

- Read-only operations (`aida show`, `aida list`) — nothing to rebase for.
- Detached HEAD or a branch with no upstream — a different problem; the
  CLI reports `clean` with a "no upstream" note and exits 0.
- Per-keystroke or per-message triggers — too noisy.
- Repeated suggestions after the user already declined this session, or
  said "don't rebase" / "I'll handle git" — **suppress for the rest of
  the session.** Respect the explicit opt-out.

## Workflow

### Step 1: Probe (always side-effect-free)

```bash
aida rebase --dry-run --json
```

This fetches the upstream, computes ahead/behind + file-path overlap,
classifies, and exits 0 **without touching the working tree**. Parse
the JSON: `classification`, `ahead`, `behind`, `overlap`,
`working_tree_clean`, `followups`.

### Step 2: Surface the classification in natural language

- **clean** / **ahead-only** — "Already in sync (or only ahead). No
  rebase needed." Stop here.
- **behind-only** — "Behind by N commits, no local commits to replay.
  Safe rebase — proceed?"
- **diverged-safe** — "Both sides advanced (M ahead, N behind), no file
  overlap. Safe rebase — proceed?"
- **diverged-risky** — "Both sides advanced (M ahead, N behind), overlap
  on: [files]. Inspect those files before proceeding."

### Step 3: Execute or defer

- Safe classes, user/agent approves → `aida rebase --auto`.
- Risky class → let the user inspect the overlap; only run `aida rebase`
  (which re-prompts) once they decide.
- A dirty working tree is auto-stashed and popped around the rebase;
  pass `--no-stash` to refuse instead.
- On conflict the CLI aborts and restores the tree, then reports the
  conflicted paths — fall back to a manual `git rebase`.

## CLI Reference

```bash
aida rebase --dry-run --json     # probe: classify, no side effects
aida rebase --auto               # execute safe rebases without a prompt
aida rebase                      # interactive: confirm before executing
aida rebase --no-fetch           # classify against cached refs (offline)
aida rebase --no-stash           # refuse on a dirty tree instead of stashing
aida rebase --branch <ref>       # rebase onto an explicit ref, not @{u}
```

## Related skills / commands

- `/aida-commit` — fire `/aida-rebase --dry-run` from its pre-commit
  check when the session is stale.
- `/aida-pickup` — verify a fresh base when picking up a queued item.
- `/aida-pr` — verify before `gh pr create` so review starts on a
  current base.
