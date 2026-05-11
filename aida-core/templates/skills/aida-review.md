---
name: aida-review
description: Drive a PR review to completion — walk the per-spec checklist for the active PR, post pass/partial/fail verdicts, optionally fix-forward mechanical issues, gate on green CI, merge if green, and mark every covered spec Completed. Dual of /aida-pr on the reviewer side. trace:STORY-91 | ai:claude
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
---

# AIDA Review Skill

## Purpose

Codify the reviewer's flow that stabilized across PR-3 through PR-7 so the prompt structure isn't re-derived from scratch each cycle. Pairs with `/aida-pr` on the implementer side and `/aida-code-review` for the orthogonal "exhaustive code-quality audit" surface (which this skill does NOT subsume — `/aida-review` is the PR-merge workflow, `/aida-code-review` is the audit).

## When to Use

Use this skill when:
- You're inside a reviewer-role session whose scope is a PR (e.g. `--owns PR-7`)
- A `Review PR-N: ...` queue item just landed via STORY-66's auto-queue and you're picking it up
- The user says "review PR-7" / "check PR-7" / "merge PR-7" after the implementer has opened it
- Before any `gh pr merge` — this skill catches half-shipped batches (partial verdicts, red CI) the manual flow misses

## Core Philosophy

**A PR merges only when every covered spec passes its acceptance criteria.** A verdict isn't a vibe-check — each spec gets a structured pass/partial/fail with evidence (diff hunks, test names, CI output). Partial counts as "not yet"; the implementer iterates, the reviewer re-runs.

## Workflow

### 1. Identify the PR target

Prefer the active session lease when one exists:

```bash
aida session show              # if --owns PR-7, scope shows PR-7
```

If the active session's scope is `PR-N`, use that. Otherwise accept `--pr N` from the slash-command args. Refuse to proceed without a concrete PR number — never guess.

Confirm with the user: "Reviewing PR-N: `<title from gh pr view>`. Proceed?" — gives them a chance to correct before any state moves.

### 2. Generate the per-spec checklist (STORY-67)

```bash
aida review prompt --pr <N> --write .aida/review-prompt-pr-<N>.md
```

`aida review prompt` walks the PR's commit range, extracts every `(REQ-ID)` trailer, loads each spec's acceptance criteria + a diff hint, and emits a structured checklist. Read the generated file; that's the worksheet.

If `aida review prompt` returns "no specs found" — the PR has commits without `(REQ-ID)` trailers. STOP and ask the user how to attribute the diff before continuing.

### 3. Walk each spec — verdict per item

For each spec in the checklist, in order:

1. **Read the diff against acceptance criteria** — does each `- [ ]` line in the spec have matching code?
2. **Run the per-spec test plan** — exact commands depend on the spec, but typically `cargo test -p <crate> <test_name>` or a focused subset. Use `cargo test --workspace` only when you can't narrow down.
3. **Post a verdict** in your running notes:
   - ✅ **PASS** — every acceptance bullet covered, tests green, no obvious regression
   - ⚠️ **PARTIAL** — some bullets covered, others missing; name the gap precisely (file + line + which bullet)
   - ❌ **FAIL** — acceptance not met, tests red, or design diverges from the spec

Evidence required for every verdict: a file:line reference for PASS, the missing bullet for PARTIAL, the failing test name + line of the divergent code for FAIL. "Looks good" without a reference is not a verdict.

### 4. Mechanical fix-forward (small commits on the PR's branch)

If the only blockers are mechanical — `cargo fmt` drift, a `#[cfg(unix)]` test fragility, a typo in a comment, an obvious unwrap that should be `?` — fix them on the PR's branch as small, atomic commits with `[AI:claude] style/fix(...)` messages. Don't fix-forward anything semantic; that's an iteration the implementer should drive.

Examples from PR-7's review cycle:
- `[AI:claude] style: cargo fmt --all` (drift introduced after TASK-57's clean)
- `[AI:claude] test(session): gate USERPROFILE assertion on #[cfg(unix)]` (TASK-62, Windows breakage)

After each mechanical fix, re-run the affected test plan from step 3.

### 5. Verify CI is green

```bash
gh run list --branch <pr-branch> --limit 1 --json status,conclusion,url
# or, to wait:
gh run watch <run-id>
```

Block merge until the latest run is `conclusion: success`. If CI is red:
- Walk the failure log: is it caused by this PR (block) or by an unrelated infra/flake (proceed with explicit user confirmation)?
- If caused by this PR, surface to the user and pause — likely a fix-forward (step 4) is the right move.

### 6. Post a consolidated review comment

One comment on the PR that summarizes the per-spec verdicts:

```markdown
## Review: PR-<N>

| Spec | Verdict | Evidence |
|------|---------|----------|
| BUG-71 | ✅ PASS | `.gitignore:94`, 4 unit tests green |
| TASK-61 | ✅ PASS | `aida-pr.md:57-77`, manual repro of refuse-on-drift |
| BUG-72 | ✅ PASS | `main.rs:11212-11258`, `auto_queue_outcome_constructors_...` green |
| TASK-63 | ⚠️ PARTIAL | acceptance covered, but `parse_session_env` doesn't unquote `'` inside a value when adjacent to `\\` |
| STORY-91 | ✅ PASS | this skill |

**CI**: all green (https://github.com/.../actions/runs/...)
**Recommendation**: merge after the TASK-63 quoting tweak.
```

Post via:

```bash
gh pr comment <N> --body "$(cat <<'EOF'
<body>
EOF
)"
```

### 7. Confirm with the user before merge

Show the verdict table. Ask explicitly: "All green — `gh pr merge <N> --squash`?" The user can:
- **Accept** (proceed to step 8)
- **Request changes** (post a `gh pr review --request-changes` instead; STOP)
- **Cancel** (no merge call)

Never auto-merge — the reviewer's `aida-review` is a workflow accelerant, not a YOLO switch.

### 8. Merge

```bash
gh pr merge <N> --squash --delete-branch=false
```

Keep the branch around so a follow-up `aida session end` on the implementer side still resolves naming cleanly. (Branch deletion is the user's call, not the reviewer's.)

### 9. Mark every covered spec Completed

For each `REQ-ID` from the checklist whose verdict was ✅ PASS:

```bash
aida edit <REQ-ID> --status completed
```

For ⚠️ PARTIAL or ❌ FAIL: leave the spec In Progress (or move it to a follow-up bug). Don't mark partials Completed — the queue is the truth.

If the PR carried a `Review PR-N:` story (from STORY-66's auto-queue), mark THAT story Completed too:

```bash
aida queue done <review-story-id> --yes
```

### 10. Hand off

Print a one-liner the user can act on:

```
✓ PR-N merged, M specs marked Completed. Run `aida session end <session-id>` to close out the reviewer session.
```

(Don't call `aida session end` from inside the skill — the user runs it from outside the worktree so their shell's cwd doesn't go stale.)

## Composes With

- `/aida-pr` (STORY-80) — sister skill on the implementer side; same authoring style
- STORY-67 (`aida review prompt --pr`) — generates the per-spec checklist; `/aida-review` consumes its output
- STORY-66 (auto-queue PR review item) — once `/aida-pr` runs, a `Review PR-N` queue item lands for the reviewer; `/aida-pickup` surfaces it, this skill drives it
- TASK-61 (pre-flight `cargo fmt --check` in `/aida-pr`) — once shipped on the implementer side, fmt drift never reaches review; step 4 has less to fix-forward
- `/aida-code-review` — orthogonal exhaustive audit, NOT a substitute. Run it before a release; run `/aida-review` per PR.

## Modes

- **Default**: walk steps 1–10 in order
- `--pr N`: explicit PR number (override session-lease detection)
- `--merge-only`: skip the per-spec walk (steps 2–3, 6) and jump to CI gate + merge + mark-completed. Use when the user has already reviewed manually and just wants the workflow's bookkeeping.

## Common Failure Modes

- **No active session lease and no `--pr`**: refuse — never guess the PR number from cwd alone.
- **`aida review prompt` returns empty**: PR has no `(REQ-ID)` trailers. STOP and ask the user how to attribute the diff.
- **CI red for an unrelated reason**: don't auto-bypass. Surface to the user; they decide whether to override.
- **Spec's acceptance criteria are vague**: ⚠️ PARTIAL by default and ask the user to either tighten the spec or accept the gap explicitly. Don't let vagueness leak through as PASS.
- **Implementer pushes new commits mid-review**: re-run step 2 (`aida review prompt --pr N` again) so the checklist matches the latest range. Verdicts on stale code are worse than none.
