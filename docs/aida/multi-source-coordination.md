# Multi-source coordination: seeing remote agent work in `aida status`

When you coordinate work across more than one machine or session — Codex on
this box, a sibling agent on another machine, a cloud `/ultraplan` session in
the browser — AIDA's local agent registry (`.aida/agents/*.toml`, STORY-431)
and session leases can only see the processes running on **this** machine.
Cloud and cross-machine agents execute work and push commits but never appear
locally, which leaves the operator as the manual aggregator of everyone's
activity.

STORY-452 closes the cheapest slice of that gap.

## What you get

`aida status` shows a **Recent remote activity (inferred)** section. It reads
the `[AI:<tool>]` provenance trailer on the tip commit of each remote-tracking
branch (`origin/*`) and surfaces the agent-attributed branches that have **no
local session lease** — i.e. work that is not already visible in the local
"Active agents" section.

Example:

```
─── Recent remote activity (inferred) ───
  codex       BUG-250        [AI:codex] fix(orchestrator): held outcome 3h ago
    branch: bug-250
  antigravity STORY-431      [AI:antigravity] feat(agents): registry      1d ago
    branch: story-431
  (inferred from commit trailers on lease-less branches — local agents shown above)
```

The section is silent when there is no remote signal (no agent-attributed
lease-less branches), so it never adds noise to a single-machine workflow.

## How the inference works

- One `git for-each-ref refs/remotes/origin` call reads each remote branch's
  tip subject + committer date — no per-branch git invocations.
- The `[AI:<tool>]` trailer is parsed for the agent type (`codex`,
  `antigravity`, `claude`, `other`). A confidence suffix (`[AI:codex:med]`) and
  mixed authorship (`[AI:codex+claude]`, first tool wins) are tolerated.
- Branches with a live **local** session lease are excluded — that is local
  work, already shown above.
- One row per branch (the most recent commit), sorted newest-first, capped at a
  handful of rows.

Freshness is only as current as your last `git fetch` — the section reads
remote-tracking refs, it does not fetch on its own.

## Honest limitations

This is **Option A (inference)** from STORY-452's mechanism menu — the cheapest
slice, deliberately. It is lossy:

- **Post-hoc only.** A remote agent appears here only after it has *pushed a
  commit* with an `[AI:...]` trailer. Live, in-progress remote work (an agent
  mid-session that hasn't pushed yet) is invisible.
- **Trailer-dependent.** A commit without the `[AI:<tool>]` convention is
  treated as human work and not shown. Agents that don't follow AIDA's commit
  format won't be attributed.
- **Stale as your fetch.** Rows reflect the last `git fetch`, not real time.
- **No live freshness / no busy-idle.** Unlike local agents (which have a
  heartbeat via STORY-435), inferred remote rows carry only the commit time.

## What's deferred

The richer mechanisms from STORY-452 are intentionally **not** built here:

- **Option B — explicit remote registration** (`aida agent register --remote`):
  precise, but manual.
- **Option C — branch-presence + lease-absence with heartbeat freshness**:
  composes with STORY-435 when shipped.
- **Option D — multi-node registry sync via the `aida-store` branch**: full
  cross-machine visibility, but heavier (store writes per agent activity).

These sequence with the EPIC-31 agent-lifecycle keystone. The inference here is
the zero-cost floor that needs no protocol and no new verbs.

trace:STORY-452
