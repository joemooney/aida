---
name: Check for a live --auto-complete orchestrator before instructing manual PR steps
description: Before telling the user to review a PR, merge it, or run a queue command on a spec, verify whether that spec is under a live --auto-complete orchestrator — the orchestrator runs review/merge/pull/build itself, so manual steps collide.
type: feedback
propagation: scaffolding-pack
originSessionId: 1bf450af-f9e9-49d0-a098-79696b14cefc
---
When a spec has an open PR, do NOT reflexively instruct "review PR-N", "merge it", or "run `aida queue work PR-N`". First establish the execution context: is the spec being driven by a live `aida queue work … --auto-complete` orchestrator? Its six phases include review → merge → pull → build (phases 3-6) — it does them itself. A manual instruction collides with the orchestrator's plan; and a "run this command" instruction that doesn't say *where* to run it causes worse tangles.

**Why:** 2026-05-19, twice in one session. (1) BUG-244/PR-108 — an implementer worked inside the orchestrator's session; had to untangle "what will the orchestrator do." (2) STORY-332/PR-110 — I told the user "review PR #110 (`aida queue work PR-110`)". STORY-332 was under a live `--auto-complete` orchestrator; the user ran the command *inside the orchestrator's still-open phase-1 session*, pressed Ctrl-C, the orchestrator advanced to phase 2 (ended the session, removed the worktree), and the user panicked. Nothing was damaged — but the instruction was wrong: the orchestrator was itself about to review and merge PR #110.

**How to apply:**
- Before any next-step touching a PR / merge / `queue work`: check `ps` for `aida queue work … --auto-complete` and `aida session leases`. Know whether an orchestrator is live.
- If the spec is under `--auto-complete`, the user's real choices are *let the orchestrator finish phases 3-6* or *deliberately stop it (Ctrl-C the orchestrator process)* — never "go run another command."
- Always state execution context explicitly: "from a fresh shell, not inside any Claude session."
- An orchestrator's phase-1 session is meant to simply **exit** so the orchestrator proceeds — never instruct typing new commands into it.

Composes with [[feedback_three_mode_autonomy_taxonomy]] and [[feedback_precise_lifecycle_vocabulary]].
