---
name: Fetch before committing on long-running sessions
description: When a session has been open a while, fetch origin before staging anything — silent commit on stale main creates divergent-branch grief at push time
type: feedback
propagation: scaffolding-pack
originSessionId: 1bf450af-f9e9-49d0-a098-79696b14cefc
---
Before committing during a session that's been open more than a few minutes (or after the user has done anything in another shell/machine), fetch origin first and check ahead/behind. The action verb is now **`/aida-rebase`** (TASK-103/104/105 — landed 2026-05-15):

```bash
aida rebase --dry-run --json    # fetches, classifies (clean/ahead-only/behind-only/diverged-safe/diverged-risky), no side effects
aida rebase --auto              # execute the rebase when the class is safe
```

The proactive-invocation playbook (when to fire it unprompted) lives in the `/aida-rebase` skill's "When to Use" section — `aida-core/templates/skills/aida-rebase.md`. Manual equivalent if `aida` isn't on PATH:

```bash
git fetch origin "$(git rev-parse --abbrev-ref HEAD)"
git rev-list --left-right --count HEAD...@{u}
# "0 N"  → behind by N: pull --rebase before committing
# "M 0"  → ahead by M: safe to commit
# "M N"  → diverged: rebase before commit if M is mine + cheap to replay
```

**Why:** 2026-05-13 incident — I committed the STORY-86 plan file locally (`b18c1d45`) without fetching first. Meanwhile PR #19 had just merged to origin/main. User's next `git pull` failed with divergent-branches error, requiring a manual `git pull --rebase` recovery. The commit was new-file-only and couldn't conflict, so the eventual rebase was trivial — but the divergence surprise itself was avoidable. Long sessions accumulate stale-HEAD risk; the agent should treat session-age as a signal to refresh before staging.

**How to apply:** Specifically when:

- Session has been running > ~15 minutes since last `git pull` or `aida pull`
- User has been working in another shell, on another machine, or via web UI (PR merges, GitHub Actions, etc.)
- About to `git add` + `git commit` on a long-lived branch like `main`
- After a long `aida ...` chain that may have changed orphan-store state but not code state

Skip when:

- Working in a fresh worktree just spawned by `aida queue work` (already pulled)
- The commit is on a feature branch nobody else touches
- About to push immediately after committing AND push will catch the issue with its own divergence check (TASK-54 wired this for `aida push`)

**Related**: `/aida-rebase` (TASK-104) now automates the detect/classify step; TASK-97 (`aida pull --autorebase`) and TASK-98 (`/aida-commit` precheck) will delegate to the `aida-core::rebase` detection module rather than re-deriving ahead/behind.

**Long-form recovery recipe**: `docs/recipes/divergent-branches.md` in the AIDA repo.
