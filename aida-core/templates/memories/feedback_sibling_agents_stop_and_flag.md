---
name: feedback_sibling_agents_stop_and_flag
description: "When a sibling agent finishes its bounded work but the worktree has unrelated dirty state from other agents/sessions, the correct discipline is STOP and flag — do not co-mingle bounded work with accumulated noise in a single PR. The flag IS the deliverable; isolation into a worktree is the master's (or another session's) responsibility."
type: feedback
propagation: scaffolding-pack
originSessionId: 1bf450af-f9e9-49d0-a098-79696b14cefc
---
Multi-agent AIDA projects accumulate working-tree state from multiple sessions: untracked plans, sibling agents' uncommitted edits, the master's notes-in-progress, scratch files. When a sibling agent finishes its bounded work (e.g., a clean test cleanup, a small bug fix), the question of "what PR should this be?" is non-trivial — committing all of git's dirty state would conflate unrelated changes, while ignoring the dirty state risks losing other work.

**The correct discipline:** when a sibling agent finishes its bounded work AND finds the worktree contains unrelated dirty changes it didn't author, **stop, flag the state, and let the master (or another session) decide how to isolate**. Do NOT:

- Open a PR with all the dirty state co-mingled (conflates unrelated work; obscures what the agent actually shipped)
- Silently discard the dirty state (loses other agents' / sessions' work-in-progress)
- Use \`git checkout -- .\` or similar destructive cleanup (same as above)

The flag IS the deliverable. The agent surfaces:

- What it changed (the bounded work it intended to ship)
- What's dirty that it didn't author (the un-authored noise it noticed)
- Recommended next step (typically: isolate via worktree, ship the bounded work, leave the other dirty state for its rightful owner)

## Why this matters

A multi-agent project accumulates state faster than a single-agent project. The default of "commit and ship what's dirty" works fine when one agent owns everything. With sibling agents, that default conflates ownership. The stop-and-flag discipline:

- Preserves separation of authorship
- Lets each agent (or human) own their own work
- Makes PRs reviewable (you see ONLY the change at hand, not unrelated drift)
- Forces explicit decisions on dirty state owned by others ("revert" vs "ship separately" vs "this is mine, I forgot")

## Empirical example

2026-05-22: Codex finished SPEC-398's test cleanup. The main worktree carried:

- Codex's intended changes (tests/test_mcp_stdio.{py,sh})
- Master's earlier untracked docs (cross-agent-onboarding.md, codex docs)
- An unaccounted-for modification to aida-cli/src/headless_tail.rs (mysterious; neither Codex nor master authored)
- 5 planning files from Codex's earlier strategic work
- A dedup script from days ago
- Antigravity setup docs in progress

Codex correctly stopped and reported: *"I have not opened a PR because the current checkout contains unrelated dirty changes ... Next clean step is to isolate the STORY-398 docs + SPEC-398 test cleanup into a PR branch/worktree."*

This is the exact behavior to reinforce. The master then created the worktree, applied Codex's changes via patch, opened the PR from isolation, and left the rest of main's dirty state untouched for its rightful owners to decide.

## How to apply

When you (sibling agent or master) finish bounded work:

1. Inspect \`git status --short\` to see what's dirty in the worktree.
2. If everything dirty is yours → commit + PR per the usual pattern.
3. If anything dirty isn't yours → STOP. Report:
   - "I finished <bounded work>. The following files are dirty but not mine: <list>. Recommended next step: isolate via worktree."
   - Mark your in-AIDA status (queue done, comment, etc.) without committing.
   - Let the master or a separate session handle the isolation.

When briefing a sibling agent at session start, include this expectation explicitly — *"if you find someone else's dirty state in the worktree when you finish, stop and flag; don't co-mingle."*

## Composes with

- [[feedback_one_master_advisor_until_subsystems]] — the master's role includes resolving multi-agent state-merge decisions; sibling agents flag, master decides.
- [[feedback_capture_over_concentration]] — flagging the state IS capture; isolation is a separate action.
- TASK-458 (\`aida pr ship\`) — eventually wraps the isolation pattern in a verb; until then, the worktree-add + patch-apply sequence is the manual form.
