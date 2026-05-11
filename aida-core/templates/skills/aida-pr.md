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

### 4. Pre-flight: cargo fmt --check (Rust only) — trace:TASK-61

Catch format drift here, not on CI. The "format-once-then-drift" pattern (TASK-57 → batch7) wastes a review cycle every time it happens.

```bash
# Detect a Rust workspace: Cargo.toml at the repo root.
test -f Cargo.toml || skip_fmt_check
cargo fmt --all -- --check
```

If `cargo fmt --all -- --check` exits non-zero:

- STOP — do not push, do not run `gh pr create`, do not add comments per step 5
- Report the drifted files (the diff output names them); typical fix is one command:
  ```bash
  cargo fmt --all
  git add -u && git commit -m "[AI:claude] style: cargo fmt --all"
  ```
- Re-run `/aida-pr` once the fmt commit lands

Skip silently for non-Rust projects (no `Cargo.toml` at the repo root). This is a Rust-toolchain check; it's not meaningful for pure-doc / pure-frontend repos.

Bypass: `--skip-fmt-check` for the rare case where drift is intentional (e.g. an in-flight rustfmt config change). Default is to refuse.

### 5. Attach an implementation summary comment per spec (STORY-81)

For each `Completed` REQ-ID derived in step 2, run:

```bash
aida comment add <REQ-ID> "$(cat <<'EOF'
Implemented via PR-N (commit <short-sha>):

- <one-line bullet derived from the matching commit's body>
- <second bullet if commit covers multiple files / behaviors>

Test status: <passing-count>/<total> tests green
Follow-up: <any explicit follow-up note from the commit body or chat output, omit if none>
EOF
)"
```

Mechanically derived — no creative writing required:

- **PR-N** is the eventual PR number; if `gh pr create` hasn't run yet, use the branch name and revise after the PR opens
- **commit short-sha** comes from `git log <base>..HEAD --grep="(REQ-ID)" --pretty=%h`
- **bullets** lift from the commit body's bulleted lines, falling back to `git show --stat` line summaries
- **test status** comes from the latest `cargo test --workspace` run in the session — usually surfaced in the agent's final report
- **follow-up** is optional; only include when the commit body explicitly notes one (e.g. "tracked separately as BUG-NN")

Skip the comment for trivial fixes whose entire commit body is one line (typo, doc bump, lint) — the commit subject is the whole context.

Once the PR opens (step 8) and the URL is known, revise the comments to include the actual PR URL via `aida comment edit`. This step is best-effort — if the user cancels before step 8, the comments still survive as useful "implemented via commit <sha>" markers.

### 6. Push code + orphan store

The orphan-branch store changes have to land before the PR is opened — otherwise reviewers see commits referencing requirements they can't `aida show`.

```bash
aida push                     # one-shot: code + orphan store
```

If `aida push` errors with "branch behind main", surface the rebase prompt rather than carry on. (See TASK-54.)

### 7. Draft the PR title + body

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

### 8. Confirm with the user

Show the title and the Summary paragraph. Ask explicitly: "Open this PR?"  The user can:
- Accept (proceed to step 9)
- Edit the title/summary inline (revise and re-confirm)
- Cancel (no `gh pr create` call)

### 9. Open the PR

```bash
gh pr create --base <base> --head <branch> --title "<title>" --body "$(cat <<'EOF'
<body>
EOF
)"
```

Use HEREDOC for the body so markdown formatting and code fences survive intact.

### 10. Output the URL

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
- **`cargo fmt --all --check` drift (TASK-61)**: refuse the PR and walk the user through `cargo fmt --all` + commit. Don't push the drifted code so CI doesn't have to catch it.
