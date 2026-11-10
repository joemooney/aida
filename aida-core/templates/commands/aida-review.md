---
description: Run /aida-review.
---
# AIDA Review

Drive a PR review through checklist and verdict; record waiting for external evidence, or proceed to merge and completion after approval.

## Usage

```
/aida-review              From inside a reviewer session whose scope is PR-N, auto-detect
/aida-review --pr 7       Explicit PR number
/aida-review --merge-only Skip the per-spec walk, just gate on CI and merge if green
/aida-review --delegated  Opt-in to delegated review via @claude review once trigger
```

## Instructions

Follow the workflow in `.claude/skills/aida-review/SKILL.md`:

1. Resolve the PR number from the active session lease's scope, or accept `--pr N`
1a. `aida review claim --pr N` to mark PR-N under review, so a merge surface refuses to land it before your verdict; `aida review record` (step 6) clears it
2. `aida review prompt --pr N --write .aida/review-prompt-pr-N.md` to generate the per-spec checklist (STORY-67)
3. Walk each spec: read the diff against acceptance criteria, run the test plan, post a ✅ PASS / ⚠️ PARTIAL / ❌ FAIL verdict with evidence; use WAITING when only external CI or host evidence is missing
4. Fix-forward mechanical issues (fmt drift, cfg-gated tests, typos) as small commits; never fix-forward semantic gaps
5. Gate on required CI green, applying the merge-hold exception in the review decision fixtures below before selecting a verdict. If required CI or host evidence is unavailable, record `waiting` with dependency, responsible actor and resume condition, omit `--finding`, then stop for a fresh review at the current head.
6. **Record the verdict FIRST** — before the PR comment, before any merge, before exit.
   This is the orchestrator's phase-3 → phase-4 handshake; a review that skips it reads as
   "the review did not complete" no matter how good the prose verdict was, and the PR comment
   is only its human-facing projection. Run:

   ```
   aida review record <SPEC> --pr N --verdict approved|request-changes|rejected|waiting \
     --summary "one line of why" \
     [--finding "concrete issue for the rework implementer"]
   ```

   The verb writes both the spec verdict and the handshake file
   `.aida/review-verdicts/PR-N.json` at the DRIVE ROOT — correctly even if you checked the PR
   out into another directory, which is exactly the case where a hand-written file lands
   somewhere the orchestrator never looks. (Hand-writing the JSON — `{"verdict": "Approved",
   "summary": "…", "mode": "orchestrator-phase-3", "findings": []}`, verdict one of `Approved` /
   `RequestChanges` / `Rejected` / `waiting` — still works, but only from the original repository root.)
   Under a headless drain, when a merge needs a human (irreversible migration, uncertain
   provenance), add `"merge": "escalated-to-human"` to the handshake file instead of merging.
7. Post a consolidated review comment with the verdict table
8. Interactive sessions: pause for explicit user confirmation before `gh pr merge N --squash`.
   Headless (`--no-human`) sessions: never prompt — act on the verdict-file default
9. After merge: `aida edit X --status completed` for every spec that PASSed; leave partials/fails In Progress
10. If a STORY-66 auto-queued `Review PR-N` story exists, mark it Completed too via `aida queue done`
11. Hand off — the user runs `aida session end` themselves from outside the worktree

Pairs with `/aida-pr` (implementer side) and `/aida-code-review` (orthogonal exhaustive audit — NOT a substitute).

<!-- trace:BUG-2500 | ai:codex -->
A review blocked solely on external CI or host evidence records `waiting`
(displayed `WAITING`), with a summary naming the dependency, responsible actor
and resume condition, and no substantive findings. Code defects still require
`request-changes`. Waiting blocks completion and merge without implementer
rework, repeat-findings credit or a Rework hold. Existing holds remain intact.
Only a fresh recorded review at the current head ends the wait; green CI or
a changed head alone does not. There is no automatic resume service.

<!-- trace:BUG-2508 | ai:codex -->
A red merge-hold-gate or supervised human/advisor clearance that this review's own fresh APPROVED verdict can release is not external evidence: record approved when required CI is otherwise green and there are no actionable defects. Approval does not release the hold or authorize merge; leave clearance to the authorized actor. Outstanding independent evidence, such as an operator-only canary, still requires waiting even on a held PR.

Review decision fixtures (same current head, no actionable defects):

| Required CI | Hold release condition | Outstanding independent evidence | Verdict |
| --- | --- | --- | --- |
| Green except merge-hold-gate | This review’s fresh APPROVED verdict, then authorized clearance | None | APPROVED |
| Green except merge-hold-gate | This review’s fresh APPROVED verdict, then authorized clearance | Operator-only canary result | WAITING |
