# Autonomous backlog burn-down

The pattern for draining a backlog of ready work **without a human in the loop** — and, just as important, the discipline that keeps it from stalling. This is the *why and the rules*; the encoded command that runs it is `/aida-burndown` (see "The command" below).

## The problem it solves

The failure mode is not "the agent can't do the work" — it's that the agent *stops*: every few items it hits a fork and asks a question, or declares it can't proceed, and the drain dies waiting for a human who isn't there. Looping a `/goal` prompt, or begging the agent to keep going, does **not** fix this. The fix is operational discipline plus the right machinery — encode the rules into the runner so "don't stop" is structural, not a thing the agent must remember (see [`substrate-as-bouncer.md`](substrate-as-bouncer.md)).

## The pattern that works

1. **Front-load decisions once.** Disposition the backlog so the ready set is decision-free; tag decided-and-buildable specs. A spec with an unresolved design fork is *not* ready.
2. **Fan out implementer subagents in parallel**, each isolated in its own git worktree, each taking **one** bounded ready spec end-to-end to a PR (read spec → implement to acceptance → trace markers → build + test + fmt → commit → push → open PR). Worktree isolation means parallel agents never collide.
3. **The main session is the integrator** — it does *not* implement. It polls the PRs, merges the green + clean ones, reconciles them to Completed, pulls, and launches the next wave.
4. **Loop it event-driven.** Wait on the wave with the harness `Monitor` tool over the drain's wake feed — `Monitor(command: "aida watch --emit-wakes", persistent: true)` — which wakes the session **only** on an actionable verb (a PR shipped/merged, a CI verdict, a punt, a shelve, the queue drained) and stays silent through the benign phase churn, so the supervisor burns **zero tokens** between events: supervision cost is O(actionable-events), not O(time-elapsed). Keep a **long-interval** scheduled wake-up (30–60 min) as the degenerate fallback so a wedged or event-less watcher still resurfaces the loop.

## The non-negotiable rules

- **Pickability gate on every spec.** Only fan out work that is **ready + unblocked + bounded** (no unresolved design fork). A spec that fails the gate is skipped or parked — never dragged in. This is what makes "never stop to ask" *safe*: the runner only dispatches work that can go end-to-end without a human.
- **Punt-and-continue.** A blocker parks **one** spec (tag it + leave a note) and the pipeline rolls on. One spec's blocker must **never** halt the pipeline.
- **Never stop to ask / never down tools.** "I can't make further progress" is almost always false — there are other ready specs; go work one. For a fork: make the defensible call, or park that one spec and move on.
- **CI gates `main`.** A bad change parks (CI red → that PR doesn't merge); it never reaches `main`. This is what lets the integrator merge greens without re-reviewing each by hand.
- **Don't fan out coupled work — route it to a batch drain mode.** The parallel fan-out is for INDEPENDENT specs. A set that shares files or must land in order goes through the sequential batch drain instead of N colliding worktrees, and instead of hand-driving a `git reset --hard origin/main` between members: tag the members `batch:NAME`, then `aida queue work --batch NAME --auto-complete --sequential` (ordered, each member its OWN PR off freshly-pulled main, one at a time) or `--single-branch` (all members accumulate on ONE branch → ONE cluster PR). The failure rule differs by mode: **`--sequential` shelves the failed member and continues; `--single-branch` halts** (later increments build on earlier commits, so it stops rather than build on broken code).
- **Keep at the keyboard, not the drain:** releases/tags and changes to the autonomy machinery itself (the orchestrator, the runner) — those ship supervised, because a fix riding through a broken drain gets caught in the breakage.

## The command

`/aida-burndown` encodes this loop so it runs the same way every time, rather than depending on an agent to remember the rules. It takes a **flexible target**, all funnelled through the one pickability gate:

```
/aida-burndown --batch <name>      # a cluster
/aida-burndown --tag <tag>         # by tag
/aida-burndown --status approved   # the ready backlog (default)
/aida-burndown --queue             # the active role's queue
/aida-burndown "<description>"      # ad-hoc → resolved to a filter
```

`aida` resolves the ready+bounded set; the skill drives the worktree-isolated fan-out + integrator loop with punt-and-continue. Default target: the ready backlog for the active role.

## Harness scope: this fan-out is Claude-only

The parallel fan-out engine is **Claude-Code-harness-only.** Step 2's wave is the harness's native subagent fan-out — `Agent(subagent_type: …, isolation: "worktree")` — a primitive only the Claude Code harness provides. A non-Claude vendor (Codex, Cursor, Amp, a bare `claude -p` script) has no equivalent, so it **cannot** run this loop.

Those vendors drain the same ready set the **serial** way instead: `aida queue work --auto-complete` one spec at a time (add `--batch NAME` to walk a cluster member-by-member). Same lifecycle — implement → CI → review → merge → pull → build — but sequential rather than a parallel wave, and vendor-agnostic because the **orchestrator**, not the harness, owns the drive. The trade is the usual one: lower throughput, universal reach. (SPIKE-74 tracks the agent-agnostic drain backend — a drain engine behind a trait — that would let non-Claude vendors fan out too; until it lands, fan-out is Claude-only and serial is the fallback.)

## Relationship to the orchestrator drain

This is the **recommended** autonomous-drain path. It deliberately uses the harness's native subagent fan-out rather than `aida queue work --auto-complete` (the orchestrator-spawns-agent path). The two are **not** competitors: the orchestrator drain is hardened in parallel; `/aida-burndown` is the path to reach for now. Don't run both against the same set and wonder which to trust — pick `/aida-burndown` for hands-off backlog draining, and use the orchestrator drain where its single-spec lifecycle is what you want.

See also: [`workflow-patterns.md`](workflow-patterns.md) (when a fix must ship at the keyboard, not the drain), [`backlog-grooming.md`](backlog-grooming.md) (getting the ready set decision-free first), [`substrate-as-bouncer.md`](substrate-as-bouncer.md) (why the rules are encoded, not documented-and-hoped).
