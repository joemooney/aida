---
name: aida-pr
description: Wrap up the current batch of commits and open a pull request with linked specs and a test plan. Walks `git log <base>..HEAD` to derive REQ-IDs, confirms they're all Done (or Completed), pushes, drafts the PR body in the established batch format, and runs `gh pr create` after user sign-off.
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

**Every shipped commit links back to a finished requirement.** STORY-86
split the post-implementation lifecycle into two states:

- `Done` — work finished on a branch. This is the expected state for
  every REQ-ID in a fresh PR. `aida queue done` flips here.
- `Completed` — merged to the default branch. `aida pull` auto-bumps
  `Done → Completed` once the referencing commit lands.

A PR composed of commits whose REQ-IDs are still `In Progress` /
`Approved` is a sign the batch isn't actually done — pause and surface
that to the user rather than open a misleading PR. `Done` and
`Completed` are both green-light states for opening the PR.

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
- `Completed` — green check (already merged on a previous PR; can ship)
- `Done` — green check (STORY-86: work finished on this branch; expected
  state for fresh batches; will auto-bump to Completed once the PR
  merges and `aida pull` runs)
- `In Progress` / `Approved` — yellow warning, this batch isn't actually done
- `Rejected` — red error, this commit shouldn't be in the batch
- not found — red error, commit references a deleted/typo'd ID

If any non-`Done` / non-`Completed` items exist, STOP and report them to the user with the matching commit SHAs. Ask: "ship anyway?" — `--force` (or explicit user confirmation) bypasses; default is to refuse.

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

For each `Done` / `Completed` REQ-ID derived in step 2, run:

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

**BUG-88: never claim a push "extends" a PR without verifying state.** Before reporting that a push went to PR-N, confirm PR-N is still open:

```bash
gh pr list --head <branch> --state open --json number
```

If the branch's previous PR has already merged, `aida push` warns and prompts before continuing — the commit would land on `origin/<branch>` but never reach `main`. Don't say "Pushed `<sha>` to PR-N" if PR-N is merged; the right action is a follow-up PR (`gh pr create --base main --head <branch>`) or cherry-picking onto a fresh branch off `origin/main`.

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

### 10. Auto-queue the review for the reviewer role — trace:STORY-90 BUG-86

Right after `gh pr create` returns the URL (and BEFORE step 11's URL output), file the reviewer story:

```bash
aida pr auto-queue-review
```

This invokes the same logic `aida session end` runs as a backup, but at the moment the agent's context is freshest — PR is just opened, branch is current, commits are in working memory. The command:

- Detects the PR via `gh pr list --head <current-branch>`
- Files a `Review PR-<n>: <title>` story routed to the `reviewer` role
- Adds `implements` relationships from the story to each spec referenced in the commit range
- Is idempotent — re-runs print `ⓘ already exists`, never duplicate-file

**Surface the outcome explicitly — never bury a failure under casual "PR opened" prose.** trace:BUG-86

The command prints one of four shapes. Each MUST be relayed verbatim, with a clear glyph header so the user can tell at a glance whether the reviewer queue actually got an entry:

*Success (✓ filed):*

```
✓ filed STORY-N (covers SPEC-1, SPEC-2, ...) → reviewer queue (PR #<n>)
```

Quote the line verbatim. Step 11's "Next steps" template renders the success path.

*Idempotent re-fire (ⓘ already exists):*

```
ⓘ PR #<n> already has a `Review PR-<n>` story queued — skipping
```

Quote verbatim. Treat the same as success for downstream steps; the reviewer queue is populated.

*By-design skip (ⓘ dim — typically "no PR yet" or "reviewer session shape"):*

```
ⓘ auto-queue: no open PR for branch `<branch>` — reviewer queue not filed
  Re-run manually: `aida pr auto-queue-review --branch <branch>`
```

This is non-fatal but the reviewer queue is empty. Tell the user explicitly: "the auto-queue stepped aside (reason: <quoted>). Re-run with `aida pr auto-queue-review --branch <branch>` after the PR is open / from outside a review session." Don't let this dilute into a vague "fine, moving on" — the user needs to know the reviewer queue is NOT populated.

*Needs-attention failure (⚠ yellow):*

```
⚠ auto-queue: `gh pr list` failed for branch `<branch>` (...) — no reviewer story filed
  Re-run manually: `aida pr auto-queue-review --branch <branch>`
```

The exit code is non-zero on this path. STOP — do not pretend the hand-off succeeded:

1. Tell the user explicitly that step 10 FAILED and the reviewer queue is empty
2. Quote the exact error line + the re-run command
3. The most common causes are `gh` unauthenticated (`gh auth status`), `gh` not on PATH, or a network blip — suggest the user run `gh auth status` first
4. The session-end backup will retry later as a fail-safe, but the user shouldn't depend on that — fixing it now keeps the implementer→reviewer hand-off tight

Step 11's "Next steps" template branches on whether step 10 succeeded — the *auto-queue skipped/failed* variant is for the by-design and needs-attention paths.

### 11. Output the URL + Next steps — trace:TASK-87 trace:TASK-110

Print the URL `gh` returned. Then surface a structured
`Next steps (recommended order):` block so the implementer→reviewer hand-off
is explicit rather than improvised. Don't auto-execute — the user picks.

**Ordering rationale (TASK-110 + TASK-111):** end-implementer comes BEFORE
start-reviewer. The implementer's lease owns the PR/STORY scope; a reviewer
session on the same scope while the implementer lease is held would
conflict (or require `--steal`, which is for stuck-lease recovery, not
normal handoffs). Since TASK-111 shipped, `aida session end` now probes
the PR's CI state and prompts (or waits with `--wait-ci`, skips with
`--skip-ci`) before releasing the lease, so the user no longer has to
sequence `gh run watch` manually — the right move is now just **End
implementer (CI-aware) → Start reviewer**. If CI is red, the End session
refuses by default so fixup commits land in the implementer session
without a lease re-claim.

**Detect state first:**

```bash
gh run list --branch <pr-branch> --limit 1 --json status,conclusion 2>/dev/null
aida session show 2>/dev/null | awk '/^Session /{print $2; exit}'   # session-id prefix
```

Combine with step 10's auto-queue outcome (✓ filed / ⓘ already exists /
⚠ skipped).

**Glyph convention** (consistent across `/aida-pickup`, `/aida-pr`,
`/aida-review`): `▶` = primary recommended action, `⏵` = alternative path,
`🚪` = stop/exit. Recommendations must be CONCRETE — name the PR, the
review story, the session ID. In this block the three rows are sequential
steps to perform in order (▶ first, then ⏵, then 🚪), not alternatives —
"recommended order" is literal.

**Templates:**

*Auto-queue succeeded (✓ filed or ⓘ already exists):*

```
PR-<N> opened: <url>
<STORY-X> filed as review story; reviewer queue has it at head.

Next steps (recommended order):
  1. ▶ End implementer session (CI-aware) → Ctrl+D + `aida session end <session-id>` from parent shell — auto-probes CI; refuses if red so you can push fixups without re-claiming the lease; pass `--wait-ci` to block until green, `--skip-ci` to release immediately
  2. ⏵ Start review session → from parent shell: `aida queue work <STORY-X>` (or `aida queue work PR-<N>`)
```

*Auto-queue skipped/failed (⚠ outcome from step 10):*

```
PR-<N> opened: <url>
⚠ Auto-queue review didn't fire (gh unauthenticated or PATH-broken).

Next steps (recommended order):
  1. ▶ End implementer session (CI-aware) → Ctrl+D + `aida session end <session-id>` from parent shell (probes CI; pass `--wait-ci`/`--skip-ci` as needed)
  2. ⏵ Open reviewer manually (or merge inline) → from parent shell: `eval "$(aida role enter reviewer --owns PR-<N>)"` + `/aida-review --pr <N>`, or just `gh pr merge <N> --squash` if you're the sole reviewer
```

Print exactly one block — don't dump both templates.

## Composes With

- `/aida-commit` — commit first, then PR. Skill chain: commit → pr.
- `/aida-code-review` — sister skill on the reviewer side; opens automatically once `aida pr auto-queue-review` (step 10) fires.
- STORY-66 / STORY-90 (auto-queue PR for reviewer) — primary trigger is step 10 here; `aida session end` re-fires the same logic as an idempotent backup so a forgotten /aida-pr (or a raw `gh pr create`) still ends up routed to the reviewer.
- BUG-74 — gh detection uses an explicit PATH walk + absolute-path fallback so the auto-queue isn't fooled by a stripped child-process PATH. `AIDA_DEBUG_GH=1` prints the search trace when gh ends up not found.

## Common Failure Modes

- **No base divergence**: `git log <base>..HEAD` is empty. Either you're on `main` or no commits have landed yet — report and exit.
- **Stale local branch**: remote has commits we don't. Surface a `git pull --rebase` prompt before pushing.
- **Half-shipped batch**: one of the REQ-IDs is still `In Progress`. Report which commit references it and ask the user to either `aida edit <id> --status completed` first or drop the commit. If the spec needs another round of work (reviewer found gaps, CI red, etc.), `aida queue rework <id> --work --resume` (TASK-218) is the one-verb recovery — flips status back to InProgress, re-queues for implementer, and chains the session relaunch.
- **REQ-ID typo**: `aida show` returns "not found". Report the commit SHA and the bad ID; ask the user to amend the commit or file the missing requirement.
- **`cargo fmt --all --check` drift (TASK-61)**: refuse the PR and walk the user through `cargo fmt --all` + commit. Don't push the drifted code so CI doesn't have to catch it.
