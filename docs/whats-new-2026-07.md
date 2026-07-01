# What's new in AIDA — July 2026

The self-improvement loop kept going. Where [June](whats-new-2026-06.md) was about the *first-touch* feeling better, July is about the **workflow around a spec** getting out of your way: one command to a ready worktree, one line that surfaces every handoff waiting on you, a `zen` that knows what it shouldn't touch, and a substrate that heals itself instead of dead-ending your next command.

Same rule as last month — this page is organized by **who benefits**, not by what shipped. Read the section that's you.

- [For the human at the keyboard](#for-the-human-at-the-keyboard) — one command to a worktree, one line to every handoff
- [For the agent on the other end](#for-the-agent-on-the-other-end) — a swappable agent binary, a status line that leads with the ask
- [For the cockpit](#for-the-cockpit-aida-tui) — the drive verb now explains what it won't do
- [Self-healing and sub-second](#self-healing-and-sub-second) — the reliability and speed work

---

## For the human at the keyboard

### One command to a ready worktree — for a single spec, not just an epic

You asked for this directly. Standing up an isolated worktree to work a spec by hand used to be a multi-step dance — create the branch, add the worktree, `cd` in, take the lease. Now `aida worktree enter` accepts a **single spec** and does the whole thing:

```
$ aida worktree enter TASK-42
▸ worktree ready: .aida-worktrees/TASK-42  (branch: task-42)
▸ lease taken — TASK-42 → In Progress
  (cd'd in; start working)
```

- `aida worktree enter <EPIC>` — a scoping-only worktree (auto-focus), no lease, for orchestrating a cluster.
- `aida worktree enter <SPEC>` — the same *plus* it takes the implementer lease and flips the spec to In Progress, so you can start editing immediately. **No agent is launched** — this is you, at the keyboard.
- `aida worktree add <…>` — create and print the path without `cd`'ing, for scripts.

Re-entering is idempotent. **Why it's better:** the gap between "I want to work this spec" and "I'm in a clean tree working it" is now one line, and the lease bookkeeping happens for you so `aida ps` tells the truth about what you're doing.

### One line tells you every handoff waiting on *you*

The coordination surfaces — mergeable PRs, unacked briefs, findings, reviewer verdicts, escalations, **and unread mail** — used to each be their own command you had to remember to check. They're now folded into a single inbox:

```
$ aida awaiting
Awaiting you (3):
  ▸ PR #1290 mergeable — STORY-743 CI green
  ▸ brief from advisor — TASK-42 (unacked)
  ▸ mail (1 unread) — from codex re: BUG-17
```

And the per-turn signal is now impossible to miss: a compact `aida awaiting --notice` line is injected once per prompt (cache-backed, **no network** — it never stalls your prompt), and it stays silent when nothing awaits you. Mail that used to sit unseen in an inbox is now surfaced in the same "Awaiting you" report that leads `aida status`.

**Why it's better:** there is now exactly *one* place that answers "is anything blocked on me?" — and it comes to you, instead of you remembering to go ask.

### `zen` knows what it shouldn't touch

`aida zen <spec>` drives a spec autonomously to merge. But some specs — anything tagged keystone / architecture / security / needs-design — *shouldn't* be driven unattended; they want a human's judgment on the forks. `zen`'s suitability gate now **holds** those specs instead of driving them, and points you at the supervised path:

```
$ aida zen ADR-7
✗ ADR-7 needs design judgment — not a safe autonomous drive.
  This is a keystone/needs-design spec. Drive it supervised with the
  guided dialog instead:
    aida queue work ADR-7 --guided
  (or clear the hold with --force if you're sure.)
```

**Why it's better:** the autonomy front door is now safe by default. You can point `zen` at anything and trust it to refuse the ones that would go wrong unattended — routing you to `--guided` (the interactive keystone dialog) rather than silently driving into a wall.

### The completion moment, the empty-queue signpost, the stale-binary nudge

The small human touches from June are all still here and a few got sharper: an empty list now *teaches* you the `aida add` that fills it, `queue done` warns before you skip a lifecycle phase, and every agent-mode error names the exact override flag on its first line so you're never hunting for the escape hatch.

---

## For the agent on the other end

### Swap the agent binary out from under a drain — `AIDA_AGENT_CMD`

The orchestrator's headless phases spawn a vendor agent (`claude -p`, etc.). You can now override *which* binary that is:

```
$ AIDA_AGENT_CMD=./my-mock-agent aida queue work --batch smoke --auto-complete --no-human
```

This is what makes the autonomy machinery **testable end-to-end without burning a real agent**: point it at a mock that replays canned phase outputs and you can exercise the full implement → CI → review → merge lifecycle deterministically. Interactive launches are deliberately left alone — the override only touches the headless path. Paired with the new declarative **scenario library** (a `ScenarioDriver` that feeds per-phase sequences through the real orchestrator), the drain engine now has a self-test harness that never touches the network.

**Why it's better:** the highest-stakes, hardest-to-test part of AIDA — the unattended drive — can now be driven by a scripted stand-in, so its branches get real regression coverage instead of "we ran it overnight and it seemed fine."

### `aida status` leads with the ask, in agent mode

In agent mode, `aida status` now opens with the **actionable count** — how many things await you — instead of a queue-depth number that didn't line up with what you could act on. An agent reading the first line gets the signal that matters: *is there something for me to do right now?*

---

## For the cockpit (`aida tui`)

### The drive verb explains what it won't do

The cockpit's `drive` verb (the TUI face of `aida zen`) now surfaces the same suitability gate as the CLI: aim it at a keystone / needs-design spec and it shows the **hold** inline, with the clarify-or-force remedy right there in the surface — rather than either driving into a wall or greying out with no reason.

**Why it's better:** the cockpit stays honest about the autonomy boundary. You see *why* a spec is held and exactly how to proceed (route to guided, or force), without leaving the TUI.

---

## Self-healing and sub-second

The reliability and speed work — the kind you feel as "it just recovered" or "that used to hang."

- **A corrupt cache heals itself.** `.aida/cache.db` is a rebuildable read-projection, but a truncated or corrupt file used to make *every* read dead-end with a SQLite error. Now AIDA detects genuine corruption (`NotADatabase` / `Corrupt` — and deliberately *not* transient `Busy`/`Locked`, which should retry), deletes the bad cache, and rebuilds it from the git-canonical store before running your query. Your command succeeds instead of failing.
- **The per-turn coordination hook can never block your prompt.** The once-per-prompt notice check now bails **instantly** on cache-lock contention (a short fast-fail ladder, no network) instead of riding the normal multi-second retry ladder — so a busy store can never make your prompt hang. (This was the `UserPromptSubmit hook timed out` papercut — gone.)
- **`aida ps` is sub-500ms.** The global running-work table used to do a full store load plus a serial `/proc` walk (~1.3s). Both were collapsed — batched process probe, cache-fast lease scan — so the cockpit's liveness glyph and `aida ps` itself now return in well under half a second. The TUI's liveness poll also stopped thrashing (longer TTL, polls only when visible, single-flight).
- **`events.jsonl` can't grow without bound.** The run event log now rotates/truncates at run-started when it crosses a size ceiling (tunable via `AIDA_EVENTS_MAX_BYTES`), so a long-lived project's event log stays bounded.
- **The integrator is a first-class seat.** The merge-authority role now has its own role file, a statusline seat, and `queue --role integrator` routing — so the person (or agent) draining merges has a real home in the coordination model, not a borrowed one. Read-only throughput view: `aida integrate`.

---

## Where to go next

- The autonomy front door: [`aida-power-features.md`](aida-power-features.md) — `aida zen` / `aida ship` / `aida burndown` / `aida integrate --watch`.
- Working a spec by hand in isolation: `aida worktree enter <SPEC>`.
- The coordination inbox: `aida awaiting` (and the silent `--notice` that rides every prompt).
- Last month's delights: [`whats-new-2026-06.md`](whats-new-2026-06.md).
