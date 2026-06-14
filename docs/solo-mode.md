# Solo mode — the unattended advisor+integrator loop

**Status:** MVP runbook (EPIC-43 / TASK-815). Composes individually-verified
pieces into one loop; the *end-to-end composition* is new — **run it supervised
for a cycle or two before leaving it truly unattended.**

Solo mode is AIDA working the **safe** backlog end-to-end without per-step human
input — groom → implement → integrate → loop — while **parking keystone work
(security, architecture, the autonomy machinery itself) for the human.** It is
the answer to "work through the night," with the explicit constraint that it
never *ships* keystone unattended; it surfaces it for review.

## The loop

```bash
# 1. GROOM — a cold-boot advisor proposes approve/reject/park/queue per open
#    spec, then executes (propose-by-default; --apply executes; [intake] policy
#    bounds it). This is the advisor seat, autonomous.
aida intake --apply

# 2. IMPLEMENT — drain the advisor-blessed ready set: one worktree-isolated
#    implementer per spec → PR. `supervised` specs are EXCLUDED (keystone stays
#    parked). Use --concurrency 1 if any ready specs share a `serialize:<group>`
#    tag (collision-prone work serializes; independent work still parallelizes).
aida burndown run            # add --no-human for headless; --concurrency 1 to serialize

# 3. INTEGRATE — the single serial merge authority. Polls for Done + open-PR and
#    drives reviewer→CI→merge→pull→build, one at a time.
aida queue integrate --watch
```

Run them as a loop (intake → drain → integrate, repeat). `integrate --watch`
can run continuously alongside; the others re-run per cycle.

## Why it's safe to leave running (the floor)

| Guarantee | Mechanism |
|-----------|-----------|
| Only one drain/integrate drives `main` at a time | `.aida/drain.lock` — both `burndown run` and `queue integrate` acquire it (BUG-538, TASK-812); a crashed holder's lock is stale-reclaimed |
| Keystone is never auto-shipped | `supervised` specs excluded from the drain; `supervised` / `review:draft-only` PRs **parked** by the integrator for human review (TASK-813) |
| Collision-prone specs don't co-fan | `serialize:<group>` tag → drain fans ≤1 per group per wave (TASK-814) |
| Escalations reach the human | reliable inter-agent mailbox (BUG-555) — a punt/fork/conflict lands in the advisor inbox + `aida questions` |
| The operator can see it running | `aida burndown status` reads the lock (pid / started / command) — so a peer session knows a drain/integrate is live (TASK-806) |

## What it PARKS for you (never decides alone)

- **Keystone / security / architecture** — `supervised`-tagged specs, `review:draft-only` PRs, the autonomy machinery itself.
- **Design forks** — a real choice (e.g. type-vs-tag) → filed as an `aida questions` DecisionRequest, not guessed.
- **Unresolvable conflicts** — a semantic merge it can't safely do → parked `NeedsAttention` + escalated, never force-resolved.

You drain those with `aida human` (decisions / reviews / triage), then let the
loop pick the rest back up.

## Caveats (honest)

- The pieces are individually tested; the **full loop is new** — supervise a
  cycle before unattended.
- `intake --apply` blessing the wrong thing is bounded by `[intake]` policy +
  the keystone exclusion, but review its first runs.
- Cross-platform: the drain lock + `pid_is_alive` are validated on Linux (PR CI);
  the nightly cross-platform run covers the rest.

## See also

`docs/autonomous-drain.md` (the drain user guide), `docs/architecture/autonomy-and-escalation.md`
(the escalation cascade), EPIC-43 (the vision + remaining: the typed `aida solo`
command, escalation-on-stuck polish).
