---
name: aida-pr
description: Wrap up the current batch of commits and open a pull request with linked specs and a test plan. Walks `git log <base>..HEAD` to derive REQ-IDs, confirms they're all Completed, pushes, drafts the PR body in the established batch format, and runs `gh pr create` after user sign-off.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
---

# AIDA PR Skill

## Purpose

Codify the "open the PR for this batch" workflow so the prompt structure isn't re-derived from memory every release. Pairs with `/aida-commit` on the producer side and `/aida-code-review` on the reviewer side.

## When to Use

Use this skill when:
- The current branch has 1+ commits ahead of `main` (or a stacked base) and the user says "open a PR" / "ship this batch" / "wrap it up"
- After `/aida-commit` finishes a final commit and the user wants to send the cluster up for review
- Before invoking `gh pr create` directly — this skill catches half-shipped batches (in-progress REQ-IDs) the manual flow misses

## Core Philosophy

**Every shipped commit links back to a Completed requirement.** A PR composed of commits whose REQ-IDs are still `In Progress` is a sign the batch isn't actually done — pause and surface that to the user rather than open a misleading PR.

## Workflow

### 1. Determine the base branch

- Default: `main`
- Override: `--base <branch>` (for stacked PRs on a previous batch's branch)
- Print the resolved base so the user can correct it before any state moves

### 2. Walk the commit log

```bash
git log <base>..HEAD --oneline
```

For each commit subject, extract the trailing `(REQ-ID)` (e.g. `(STORY-78)`, `(TASK-44)`, `(BUG-67)`). Multiple IDs in one subject (e.g. `(TASK-45/46/47)`) all count. Commits without a REQ-ID are fine for `chore`/`docs` types but should be called out if a `feat`/`fix` is missing one.

### 3. Verify status on each REQ-ID

```bash
aida show <REQ-ID>            # for each derived id
```

Collect a status table:
- `Completed` — green check
- `In Progress` / `Approved` — yellow warning, this batch isn't actually done
- `Rejected` — red error, this commit shouldn't be in the batch
- not found — red error, commit references a deleted/typo'd ID

If any non-`Completed` items exist, STOP and report them to the user with the matching commit SHAs. Ask: "ship anyway?" — `--force` (or explicit user confirmation) bypasses; default is to refuse.

### 4. Push code + orphan store

The orphan-branch store changes have to land before the PR is opened — otherwise reviewers see commits referencing requirements they can't `aida show`.

```bash
aida push                     # one-shot: code + orphan store
```

If `aida push` errors with "branch behind main", surface the rebase prompt rather than carry on. (See TASK-54.)

### 5. Draft the PR title + body

**Title format** (mirrors PR-3 through PR-6):

```
EPIC-N batch M: <one-line summary>
```

Derive `N` from the dominant `@EPIC-N` chip across the batch's requirements; derive `M` from the branch name (`epic-20-batch5` → `batch 5`); summary is a 3–6 word description of the cluster.

**Body sections:**

```markdown
## Summary

<2–3 sentence overview of what the batch achieves end-to-end>

## Per-spec

### <REQ-ID-1>: <title>
<1-paragraph body from the matching commit's full message; trim the trailing Co-Authored-By>

### <REQ-ID-2>: <title>
...

## Test plan

- [x] `cargo test --workspace` — <N>/<N> green
- [ ] Manual: <one item per significant spec>
- [ ] <other smoke tests run during development>
```

### 6. Confirm with the user

Show the title and the Summary paragraph. Ask explicitly: "Open this PR?"  The user can:
- Accept (proceed to step 7)
- Edit the title/summary inline (revise and re-confirm)
- Cancel (no `gh pr create` call)

### 7. Open the PR

```bash
gh pr create --base <base> --head <branch> --title "<title>" --body "$(cat <<'EOF'
<body>
EOF
)"
```

Use HEREDOC for the body so markdown formatting and code fences survive intact.

### 8. Output the URL

Print the URL `gh` returned, and optionally suggest `/aida-code-review` as the next step if the project has a reviewer role configured.

## Composes With

- `/aida-commit` — commit first, then PR. Skill chain: commit → pr.
- `/aida-code-review` — sister skill on the reviewer side; opens automatically when STORY-66's auto-queue fires.
- STORY-66 (auto-queue PR for reviewer) — once a PR exists, `session end` auto-files a queued review item.

## Common Failure Modes

- **No base divergence**: `git log <base>..HEAD` is empty. Either you're on `main` or no commits have landed yet — report and exit.
- **Stale local branch**: remote has commits we don't. Surface a `git pull --rebase` prompt before pushing.
- **Half-shipped batch**: one of the REQ-IDs is still `In Progress`. Report which commit references it and ask the user to either `aida edit <id> --status completed` first or drop the commit.
- **REQ-ID typo**: `aida show` returns "not found". Report the commit SHA and the bad ID; ask the user to amend the commit or file the missing requirement.
