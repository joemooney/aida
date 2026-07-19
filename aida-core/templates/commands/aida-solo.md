---
description: "The WARM interactive solo-driver: put THIS session in the solo advisor+integrator seat and run the supervised groom → implement → integrate loop with live judgment until the safe backlog is drained — keystone parked for the human."
---
<!-- trace:STORY-668 -->
# Drive the Safe Backlog Solo (warm, full judgment)

Put the current (warm) Claude session in the solo advisor+integrator seat and run the supervised loop end-to-end WITH live judgment — the better-judgment counterpart to the headless cold-boot `aida solo run`. Keystone work (security, architecture, the autonomy machinery) is PARKED for the human, never shipped unattended.

## Instructions

Follow the workflow in `.claude/skills/aida-solo.md`:

1. ENTER: `aida solo` (marks this session solo — statusline marker + 24h TTL; exit later with `aida solo stop`).
2. LOOP until the safe backlog is drained:
   - GROOM (judgment): cross-spec advisor pass — worth-doing, dedup, dispose; bless + queue the pickable approved set. Mechanism: `aida intake --apply` (or the guided `/aida-backlog-groom`). Route human-decision drafts to `aida questions`; never auto-approve.
   - IMPLEMENT: enumerate the fan-out-able set with `aida burndown plan --status approved --json`, then fan ONE worktree-isolated implementer subagent per `ready` spec → PR. PARK keystone/`supervised` (security/architecture/autonomy machinery) for the human — never ship unattended. See `/aida-burndown` for the fan-out discipline.
   - INTEGRATE (serial merge authority): wait CI terminal → merge `--squash` (no `--delete-branch` — the implementer worktree still holds the branch; deletion happens at worktree pruning) → `aida pull` as its own step (the auto-bump rides `aida pull`; never `&&`-chain it behind the merge). HOLD `review:draft-only`/`supervised` PRs. Rebuild/test combined main between batches (`cargo build -p aida-cli`); HALT on red. Punt-and-continue: a blocker parks ONE spec (NeedsAttention), never halts the loop.
   - Loop.
3. REPORT (PushNotification + advisor mailbox handoff), then `aida solo stop`.

Guardrails: never ship keystone unattended; destructive ops (branch/worktree delete, `aida doctor --heal` destructive tier) need sign-off (STORY-666 — gate by consequence); combined-main green between batches (BUG-496); supervise a cycle or two before trusting it.

Delineate: `/aida-solo` = the WARM full-judgment end-to-end loop; `aida solo run` = the HEADLESS cold-boot engine; `/aida-drain-queue` = implement-only; `/aida-burndown` = fan-only. Runbook: `docs/solo-mode.md`.

ARGUMENTS: $ARGUMENTS
