# AIDA session lifecycle: implementer → reviewer → CI → fixup loop

*Last updated: 2026-05-13*

The AIDA session model coordinates a multi-actor workflow: an implementer writes code, opens a PR, hands off to a reviewer, CI runs in the background, and if anything's wrong the implementer comes back for fixups. Each transition between phases is a coordination point with non-obvious correctness conditions. This doc names them, shows the layered tooling that handles each, and documents what survives across the transitions.

For the *git-mechanics* side (base-freshness, divergence, rebase recovery) see [`git-workflow.md`](git-workflow.md) — sibling doc. This doc focuses on the *session* and *role* side.

---

## The two session layers

AIDA's "session" actually combines two underlying objects that have different lifetimes:

| Layer | Owns | What `aida session end` does | What survives end |
|---|---|---|---|
| **AIDA session** | Lease on a scope (e.g. STORY-86, PR-20), worktree, session manifest, role identity | Releases the lease; may clean up worktree | Manifest in `.aida-store/`, recorded for history. The lease is gone — anyone can claim the scope. |
| **Claude Code session** | Conversation history (every message + tool use), tool-result cache, model state | Untouched | The JSONL file at `~/.claude/projects/<project>/<session-id>.jsonl` stays on disk. Can be resumed via `claude --resume <session-id>`. |

This split is the leverage point for the **resume capability** (TASK-112): even after the AIDA session is ended, the Claude conversation is right there — pointing at it on the next pickup avoids cold-launching with no memory.

---

## The phases

A typical implementer→reviewer→merge run looks like:

```
  ┌──────────────────────────────────────────────────────────────────────┐
  │ 1. PICKUP                                                            │
  │    aida queue work STORY-86                                          │
  │    ├─ aida fetch (planned TASK-107) — refresh remote view            │
  │    ├─ git pull code branch (TASK-99) — start from fresh base         │
  │    ├─ aida session start — claim lease, create worktree              │
  │    ├─ session manifest seeded — may include plan from docs/plans/    │
  │    │   (planned TASK-95)                                             │
  │    └─ launch claude (fresh or --resume — TASK-112)                   │
  │                                                                      │
  │ 2. WORK                                                              │
  │    aida edit STORY-86 --status in-progress                           │
  │    (statusline shows base behind-count when stale, TASK-101)         │
  │    code commits with trace:STORY-86 | ai:claude                      │
  │    /aida-rebase fired proactively (STORY-114) if base drifts         │
  │                                                                      │
  │ 3. WRAP UP & PR                                                      │
  │    aida queue done STORY-86 — detects stale base first (TASK-100),   │
  │      offers rebase before flipping to Done                           │
  │    /aida-pr — opens GitHub PR; auto-queues review (STORY-66/90)      │
  │      records PR-N in session manifest                                │
  │    /aida-pr "Next steps" block (TASK-110): Wait CI → End → Start     │
  │                                                                      │
  │ 4. CI + END                                                          │
  │    aida session end (TASK-111 — CI-aware):                           │
  │      ├─ CI green → ends silently                                     │
  │      ├─ CI in-progress → prompt: Wait / End anyway / Cancel          │
  │      ├─ CI red → warn: keep session for fixups? offer --resume hint  │
  │      └─ No PR → ends silently (today's behavior)                     │
  │    Lease released; worktree may persist for resume                   │
  │                                                                      │
  │ 5. REVIEW                                                            │
  │    aida queue work STORY-<review-spec> — picks up auto-queued        │
  │      reviewer item from /aida-pr's STORY-66/90 hook                  │
  │    /aida-review — spec-walk + adversarial pass (STORY-109)           │
  │    merge or request changes                                          │
  │                                                                      │
  │ 6. FIXUP (if needed)                                                 │
  │    aida queue work STORY-86 --resume (TASK-112) — relaunch the       │
  │      implementer's prior claude session with full context            │
  │    fix → re-PR or push fixup commits to same PR                      │
  │    loop back to phase 4                                              │
  └──────────────────────────────────────────────────────────────────────┘
```

Each transition has been a source of friction at some point. The remainder of this doc walks each.

---

## Transition 1→2: Pickup is base-fresh

The implementer session must start on a *fresh* base — otherwise every commit accumulates rebase debt that surfaces at PR time. AIDA's defenses:

- **TASK-99** — `aida queue work` pulls the code branch (not just the orphan store) before creating the worktree. Closes the start-of-session base-freshness gap.
- **TASK-107** — `aida fetch` provides a cheap refresh primitive that TASK-99 calls.
- **TASK-95** (planned) — when `docs/plans/<spec>*.md` exists, its content seeds the session manifest so the implementer doesn't have to discover the plan separately.

Together: a fresh worktree on a freshly-pulled base, with the plan (if any) already loaded as context.

---

## Transition 2→3: Wrap-up rejects stale base

When the implementer signals "done" via `aida queue done STORY-86`, AIDA inspects the base before flipping status. If the branch has drifted behind origin during the work:

- **TASK-100** — detects stale base, offers rebase before allowing the done transition. Catches it pre-PR rather than post.

Then `/aida-pr` opens the PR and writes the PR number into the session manifest (load-bearing input for TASK-111 below).

---

## Transition 3→4: Next-steps ordering

The biggest historical bug here was *ordering*: the post-PR "Next steps" block in `/aida-pr` listed actions in the wrong order, suggesting "start review session" while the implementer's lease was still held.

- **TASK-110** — fixes the ordering. Conservative path: Wait CI → End implementer → Start reviewer.

Once TASK-111 ships, this collapses further (CI handling moves *into* `aida session end`, so the user sees two steps: End → Start).

---

## Transition 4: Session end IS CI-aware

`aida session end` owns the lease being released — which makes it the right place to know whether the work is actually *finished* (CI confirms) versus *paused mid-flight* (CI red, fixups needed). TASK-111 puts CI awareness here:

| CI state | `aida session end` default |
|---|---|
| No PR associated with session | End silently (today's behavior preserved for non-PR sessions) |
| PR exists, CI not started | Info: PR opened, CI hasn't started. End anyway? |
| PR exists, CI in-progress | Prompt: Wait / End anyway / Cancel |
| PR exists, CI green | Info: ✓ CI green. Proceeds. |
| PR exists, CI red | Warn: ⚠ CI failed. Keep session for fixups? — surfaces `aida queue work STORY-86 --resume` as the recovery |

Flags: `--wait-ci` (block until green), `--skip-ci` (today's behavior), `--force` (end regardless).

The CI-red case is where the **resume capability** earns its keep — the warning links forward to the resume verb.

---

## Transition 4→5→6: Resume preserves Claude context

When CI fails (or reviewer asks for fixups, or you just want to revisit tomorrow), the implementer Claude session's conversation history is the most valuable artifact on disk. TASK-112 makes it reachable:

```
aida queue work STORY-86 --resume        # use most recent recorded claude session for this scope
aida queue work STORY-86 --resume <id>   # use specific session ID
aida queue work STORY-86 --fresh         # today's behavior (cold launch)
aida queue work STORY-86                 # prompts if previous claude session exists
aida queue work --list-sessions STORY-86 # enumerate prior claude sessions for scope
```

Mechanism: AIDA session manifest records the `claude_session_id` at launch; resume looks up the most recent for the scope; `claude --resume <id>` brings back the full conversation while AIDA recreates the worktree (same branch).

This is what makes the **fixup loop cheap.** Without resume, every fixup pays the cold-launch tax (re-read files, re-orient, re-derive decisions). With resume, the model picks up exactly where it left off.

---

## What survives across the lifecycle

| Artifact | Lifetime | How to recover |
|---|---|---|
| Code commits on PR branch | Until branch deleted or force-pushed | `git checkout <branch>` |
| AIDA session lease | Until `aida session end` (or `--steal` from another session) | New lease on same scope replaces |
| AIDA session manifest | Persists in `.aida-store/` | `aida session show <id>` (historical) |
| Claude conversation JSONL | Persists indefinitely on local disk | `claude --resume <id>` — wired via TASK-112 |
| Worktree directory | Configurable; may persist after `aida session end` | Recreated on `aida queue work` (with same branch) |
| Plan file (`docs/plans/`) | Committed to git, permanent | `git log docs/plans/` |
| Trace comments in code | Permanent (committed) | `aida search` / `grep trace:` |
| Doc seeds (req comments) | Permanent in orphan store | `aida show <SPEC>` |
| Followup tasks | Per TASK-96, auto-filed from plan's Followups section on queue done | `aida list --parent <SPEC>` |

The pattern: **everything important is recoverable.** Session end isn't destructive — it's a coordination point. The resume capability is what makes that statement load-bearing rather than aspirational.

---

## Why roles don't share Claude sessions

A natural question when working through the implementer → reviewer → fixup loop: why end the Claude session at each role boundary? Couldn't a single Claude session switch roles in place — `/aida-end-implementer` then `/aida-start-reviewer` — preserving the conversation history across transitions?

**No.** Role boundaries are session boundaries by design. The reason is the same insight that makes [/ultraplan's three-explorer plus one-critic architecture](positioning/vs-ultraplan.md) work: a critic agent that shares context with the agents whose work it's reviewing is doing self-review with extra steps. Anchoring bias propagates through shared context.

Apply that to AIDA: if the same Claude session handles both implementer work and reviewer work, the reviewer agent has the implementer's mental model already in its context window. It's reviewing from inside the implementer's reasoning, not from cold. The separate-session boundary is what *enforces* independent review.

### The boundary is the role, not the session

Same Claude session can span everything within one role's natural work. Different roles require different sessions.

| Transition | Same Claude session OK? |
|---|---|
| implementer doing code → `/aida-pr` (still implementer, opening the PR) | ✓ same role |
| `/aida-pr` → `/aida-commit` fixups → `/aida-pr` re-run | ✓ same role |
| reviewer doing the spec walk → posting verdict comments | ✓ same role |
| implementer → reviewer | ✗ anchoring bias risk |
| reviewer → implementer fixup | ✗ different role |
| original implementer session → resume after review (TASK-112 path) | ✓ same role, just bracketed by a reviewer detour |

So the implementer Claude session naturally spans: pickup → code → commits → PR → (clean review: done; or: end session, resume via TASK-112 → fixup → done). What it cannot do: take a quick reviewer detour mid-session and keep going. The reviewer step must be its own Claude session.

### The TUI orchestration vision

The role-pure session model wants a coordination surface *above* individual sessions. Today that surface is the shell + `aida queue list` + the implementer's intuition. A TUI dashboard is the natural fit:

- **TUI is the home** — shows queued work across all roles, active leases, recent transitions, CI status, plan-file presence
- **Sessions are role-pure work units** — short-lived, scoped to one role
- **Transitions go through the TUI** — end session → see dashboard → pick up next role's work → start new session

This shape matches existing tools:

| Tool | "Home" | "Work" |
|---|---|---|
| Linear / Jira (humans) | Board view | Individual ticket |
| Claude Code on the web | claude.ai/code dashboard | Individual cloud session |
| GitHub | Repo / PR view | Individual checkout |
| **AIDA + TUI** (future) | TUI dashboard | Role-pure Claude session |

The TUI isn't filed as a STORY yet (as of 2026-05-13). Most of its prerequisites exist or are filed:

| Need | Status |
|---|---|
| Queue view across roles | `aida queue list --all` covers it |
| Session inventory | `aida session list` |
| Lease awareness | Session manifest |
| Resume capability for the same-role-after-detour case | TASK-112 (approved) |
| Role-pure session enforcement | Already enforced by AIDA's lease model |
| **Visual dashboard / TUI surface** | **Missing — this is the TUI gap** |

A future STORY will name the TUI directly when its prerequisites have shipped.

### Acknowledging the cost

The cold-restart tax is real. Every role transition pays it: the new role's Claude session opens cold, has to re-orient (read files, search the codebase, re-understand decisions). The session-lifecycle defenses minimize the cost but don't eliminate it:

- **TASK-99** (queue work pulls code) ensures the new session opens on a fresh base
- **TASK-95** (queue work pre-populates manifest from plan) ensures it opens with the plan as context
- **TASK-112** (resume claude session) eliminates the tax for the same-role-after-detour case
- **TUI** (future) would surface the cluster context at the moment of pickup

For genuinely different roles, cold-start is part of the cost of independent review. That's the trade: anchoring-bias protection in exchange for a fresh context each time. The /ultraplan parallel: paying for three explorer agents in parallel is worth it because what you get back is genuine diversity, not paraphrased sameness. Same logic; same trade.

---

## Cross-references

- [`git-workflow.md`](git-workflow.md) — sibling doc on base-freshness and divergence (Pattern A / B)
- TASK-99 — `aida queue work` pulls code at start
- TASK-100 — `aida queue done` detects stale base
- TASK-101 — statusline behind-count for active leases
- TASK-107 — `aida fetch` primitive (used by TASK-99 and others)
- TASK-110 — `/aida-pr` next-steps reorder
- TASK-111 — `aida session end` CI awareness
- TASK-112 — `aida queue work --resume` claude session resume
- STORY-114 — `/aida-rebase` centralized rebase verb
- STORY-115 — git-mirror verb surface (granularity, fetch, dry-run, conventions)
- EPIC-23 — Session orchestration & autonomy (parent of all above)
- STORY-42 — `aida queue work` one-shot session + launch (the verb being augmented)
- STORY-66 / STORY-90 — auto-queue PR for reviewer (the auto-queue hook `/aida-pr` fires)
- STORY-109 — `/aida-review` adversarial pass (review-side depth)
- BUG-74 — `gh` detection PATH-walk (shared helper as `gh` use grows across these flows)
