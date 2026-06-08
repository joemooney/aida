# Autonomously Burn Down the Backlog

Fan out worktree-isolated implementer subagents over the ready set, integrate
their PRs, and loop until drained — the empirically-working autonomous drain,
encoded so the "never stop to ask" rules are structural.

## Instructions

Follow the workflow in `.claude/skills/aida-burndown.md`:

1. Resolve the selector from `$ARGUMENTS` (default `--status approved`; also
   `--tag <T>` / `--batch <B>` / `--max <N>`).
2. Get the ready set: `aida burndown plan <selector> --json` → `{ready, parked}`.
   Report the parked count + reasons once; act on `ready` only. Empty → stop.
3. Fan out one worktree-isolated implementer subagent per ready spec (bounded
   wave); each takes ONE spec end-to-end to a PR.
4. Integrate (you do NOT implement): merge green+clean PRs, reconcile to
   Completed, pull. HOLD any `review:draft-only` spec as a draft for the operator.
5. Punt-and-continue: a blocker parks ONE spec, never halts the wave. Never stop
   to ask; never down tools.
6. Re-run `aida burndown plan`, launch the next wave, loop until `ready` empty;
   then `PushNotification` an honest summary.

The pickability gate in `aida burndown plan` guarantees only bounded + unblocked
+ decision-free specs are fanned out — that is what makes step 5 safe.
