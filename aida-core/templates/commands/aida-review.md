---
description: "Drive a PR review to completion — checklist, verdicts, fix-forward, merge, mark-complete."
---
# AIDA Review

Drive a PR review to completion — checklist, verdicts, fix-forward, merge, mark-complete.

## Usage

```
/aida-review              From inside a reviewer session whose scope is PR-N, auto-detect
/aida-review --pr 7       Explicit PR number
/aida-review --merge-only Skip the per-spec walk, just gate on CI and merge if green
/aida-review --delegated  Opt-in to delegated review via @claude review once trigger
```

## Instructions

Follow the workflow in `.claude/skills/aida-review.md`:

1. Resolve the PR number from the active session lease's scope, or accept `--pr N`
2. `aida review prompt --pr N --write .aida/review-prompt-pr-N.md` to generate the per-spec checklist (STORY-67)
3. Walk each spec: read the diff against acceptance criteria, run the test plan, post a ✅ PASS / ⚠️ PARTIAL / ❌ FAIL verdict with evidence
4. Fix-forward mechanical issues (fmt drift, cfg-gated tests, typos) as small commits; never fix-forward semantic gaps
5. Gate on CI green (`gh run watch` if a run is in-flight)
6. **Write the verdict file FIRST** — before the PR comment, before any merge, before exit.
   This is the orchestrator's phase-3 → phase-4 handshake; a review that skips it reads as
   "the review did not complete" no matter how good the prose verdict was, and the PR comment
   is only its human-facing projection. Write `.aida/review-verdicts/PR-N.json`:

   ```json
   {"verdict": "Approved", "summary": "one line of why", "mode": "orchestrator-phase-3"}
   ```

   `verdict` is one of `Approved` / `RequestChanges` / `Rejected` (case- and
   hyphen-tolerant). Under a headless drain, add `"merge": "escalated-to-human"` instead of
   merging yourself when the merge needs a human (irreversible migration, uncertain provenance).
7. Post a consolidated review comment with the verdict table
8. Interactive sessions: pause for explicit user confirmation before `gh pr merge N --squash`.
   Headless (`--no-human`) sessions: never prompt — act on the verdict-file default
9. After merge: `aida edit X --status completed` for every spec that PASSed; leave partials/fails In Progress
10. If a STORY-66 auto-queued `Review PR-N` story exists, mark it Completed too via `aida queue done`
11. Hand off — the user runs `aida session end` themselves from outside the worktree

Pairs with `/aida-pr` (implementer side) and `/aida-code-review` (orthogonal exhaustive audit — NOT a substitute).
