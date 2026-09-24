# Chapter 3 — Work & autonomy

This is the chapter where AIDA stops being a spec database and starts *doing work*. It's also the chapter with the most overlapping commands — `queue`, `backlog`, `burndown`, `drain`, plus the presence and escalation verbs — so it opens with a **decision tree** instead of an alphabetical list. Find your situation, follow the arrow, then read that command's entry.

> Manual contract: rationale, not flag tables. `aida <cmd> --help` owns the exact flags.

## The autonomy ladder — start here

The whole chapter answers one question: **"I have approved work; how does it get done?"** The answer depends on two things — *how much* (one spec vs many) and *who's driving* (you at the keyboard vs agents unattended).

```
Do you have approved, queued work to execute?
│
├─ ONE spec, you want to drive it interactively
│ → aida queue work <SPEC> (pick it up, launch a session)
│
├─ SEVERAL queued specs, you're AT THE KEYBOARD watching
│ → aida queue work --auto-complete nextN (sequential drain, you supervise)
│ …or the parallel fan-out: /aida-burndown (Claude Code skill)
│
├─ SEVERAL specs, you're AWAY / want it unattended overnight
│ → aida away (set presence) then aida queue work --auto-complete
│ …presence makes it default to headless; integrity gates still apply
│
├─ You don't know WHAT'S READY to drain yet
│ → aida burndown plan (read-only: ready vs parked, the pickability gate)
│
├─ You have APPROVED work but it's not queued yet
│ → aida backlog list → aida backlog groom (tee it up — the advisor sign-off)
│
└─ A drain is RUNNING and you want to know what it's doing
 → aida drain status
```

Three cross-cutting truths the tree assumes:
- **Only *queued* work drains.** Queueing a spec is the advisor sign-off; nothing autonomous touches a spec you didn't queue. That's why `backlog groom` (approved → queued) is a step.
- **The pickability gate** decides what's *safe* to fan out: bounded (not an epic), unblocked, decision-free, not parking-tagged. `burndown plan` shows you that gate's verdict without running anything.
- **When an agent hits a decision it can't make, it *punts*** (parks the spec) rather than guessing — and that punt surfaces to *you* via `findings` / `questions` / `human`. The escalation cascade is implementer → advisor → human, and the human is its permanent terminus.

---

### `aida queue`

**One line** — your personal work queue: what's teed up for you (or your role) to pick up.

**Mental model.** The queue is the *blessed, ordered* list of work — distinct from the *backlog* (approved but not yet teed up) and from a raw `list` (everything). An approved spec isn't "work in progress" until it's queued; `queue add` is that act, and because it's authority-gated, **the queue is the record of what's been signed off for execution**. `queue work` is the verb that actually picks an item up — leases it, spins a worktree, launches a session.

**Reach for it when** — you want to see what's teed up (`queue list`), add an approved spec to it (`queue add`), or start working the head (`queue work`). `queue work --auto-complete` is the autonomous drain — it drives the full lifecycle (implement → CI → review → merge → pull) per item.

**Don't reach for it when** — the spec isn't approved yet (approval comes first; the queue is post-approval); or you want the *unqueued* approved work (that's `backlog`); or you're trying to see who's *actively working* something (that's `aida session leases`, not `queue list`).

**Key options / subcommands (rationale only).**
- `queue work <SPEC> --auto-complete` — the drain. Add `nextN` (e.g. `next5`) to drain several; `--batch NAME` to drain a tagged cluster; `--no-human=both` to run it fully headless.
- `queue work --auto-complete --require-head` (also `burndown run --require-head`) — a wave pins the binary that launched it, so when you drain the AIDA repo itself on a dev build, launch checks that build against the default branch HEAD. A build that is behind (or has diverged) prints a one-line warning and the wave still runs it, which is how you deliberately pin an older binary to bisect an orchestration regression. `--require-head`, or `[drain] require_head = true` in `.aida/config.toml`, turns that warning into a refusal naming the fix (`make build-fast`, then rerun). A build *ahead* of HEAD counts as current; released binaries and downstream projects are never checked; `.aida/drain.lock` records `launched_stale` when a wave launched on a stale build.
- `queue list` — merges your local (per-project) queue with your active role's *global* queue, tagging cross-project entries with `[origin:<project>]`. The `--global` / `--local` flags scope it when the merge is noise.
- `queue advance` — walk the whole queue and push each item to its *next* step: autonomous items drain, human-required ones (review / `--zen` / decision) get dispatched interactively. Where `queue work` picks up *one* head, `advance` processes the queue to a resolution and never silently hides work it can't auto-handle.
- `queue done` — mark work finished on a branch (→ **Done**). The precise verb the lifecycle wants (vs the newcomer `aida done`).
- `queue rework` — send a spec back (also reachable as the top-level `aida rework`, see [Ch4](04-git-lifecycle.md#aida-rework)).
- `integrate` — the read-side of the producer/consumer split: parallel implementers finish work, flip a spec Done, and leave an open PR but never merge; bare `aida integrate` shows the focus-scoped queue, live throughput, and active fan-out so an integrator can see whether main is moving. The handoff is the substrate itself — no message bus.
- `queue recover` — an interactive wizard for a spec stuck after a **failed phase-1 implementer session** (a provider 529, commit-and-exit without a PR, an external crash, partial work). It inspects the spec's git/lease/PR state, recommends a recovery path, and steps through it — a front-end over the same lease/PR probes the orchestrator uses, not new mechanism.

**Gotchas.** The queue's identity is your **shell user** (`$USER` / `$AIDA_USER`), *not* your AIDA node or role. If `queue list` is unexpectedly empty, check `echo $USER` and `echo $AIDA_USER` first — the queue is keyed off whichever the shell sees.

**Chains with** — `backlog groom` fills it; `queue work` empties it into sessions; `drain status` watches an `--auto-complete` run draining it.

---

### `aida backlog`

**One line** — curate Approved-but-not-queued work into the queue.

**Mental model.** The backlog is the holding area between *approved* and *queued*: specs blessed to exist but not yet teed up for execution. `backlog` is the advisor's grooming surface — see what's waiting (`list`, with advisory risk chips), check which candidates touch the same files (`analyze`), and move chosen ones onto the queue (`groom`). **Grooming is the sign-off that feeds the drain.**

**Reach for it when** — you're the advisor deciding *what to work next*: pull the ready, low-risk, non-overlapping approved specs onto the queue so the drain (or an implementer) can pick them up.

**Don't reach for it when** — the work isn't approved yet (backlog is approved-only; approval is the prior gate); or you want to *auto-approve and queue in one shot* — that conflates approval (judgment) with queueing, and approval must stay deliberate.

**Key options / subcommands (rationale only).**
- `backlog groom` — move selected specs (`--specs` CSV or `--from-stdin`) onto the queue; `--batch NAME` tags them so `queue work --batch NAME` drains them as one cluster. `--dry-run` first.
- `backlog analyze` — pairwise file-overlap between candidates (from trace markers + plan Critical-Files). Use it to *avoid* fanning out two specs that edit the same file in parallel.

**Gotchas.** The risk chips are *advisory* — they don't gate anything; they're a hint for your grooming judgment. `backlog list` shows **work** types only: an accepted ADR sits at `Approved` exactly like blessed work does, but a decision, a vision, a glossary term and a principle are *authored*, not implemented, so they are never offered as groomable candidates. Name one with `--type decision` (or `vision` / `term` / `principle`) if you want to see it anyway.

**Chains with** — `burndown plan --candidates` shows what's groomable; `backlog groom` queues it; `queue work` / `/aida-burndown` drains it.

---

### `aida burndown`

**One line** — the read-only foundation for an autonomous backlog burn-down.

**Mental model.** `burndown plan` resolves which queued specs are **ready to fan out** vs **parked**, applying the *pickability gate* (bounded + unblocked + decision-free + not parking-tagged). It changes nothing — it's the deterministic input the actual run consumes. The **run itself is the `/aida-burndown` skill in Claude Code**, not a CLI subcommand, because the parallel fan-out (one worktree-isolated agent per spec) is a harness capability, not something a plain process does. `burndown status` is the read-side companion to a *running* drain: is one live (the global drain lock — pid, start, launching command — corroborated against a PID-liveness probe, so a crashed run shows as a **stale lock** rather than a phantom), what worktrees are leased in-flight, and a pointer to the live event log (`.aida/burndown/<drain-id>.jsonl`) to tail.

**Reach for it when** — before kicking off a drain, to see what *will* fan out and what's blocked and *why* (`plan`); or to find groomable candidates (`--candidates` shows approved + pickable + not-yet-queued — what you could bless next); or, *while* a drain is running, to ask "is one live, what's it working, where's the log?" (`status`).

**Don't reach for it when** — you expect `plan` to *run* the burn-down (it doesn't — that's the skill); or you want to drain one specific spec interactively (`queue work <SPEC>`).

**Gotchas.** `burndown plan`'s default set is the *queued* pickable specs — queue membership is the gate, so an approved-but-unqueued spec won't appear until you groom it. It also considers **work** types only: the knowledge-class records (decision, vision, term, principle) are authored, not implemented, so they never appear as ready or as parked-with-a-reason, even though an accepted ADR sits at `Approved` just like blessed work. Pass `--type decision` (or any other type) to narrow the plan to exactly that type — that's also how you opt a knowledge-class record back into the view. The parked entries each carry a reason; `aida why <spec>` explains any single one. `burndown run --verbose` streams live progress and tees JSONL to `.aida/burndown/<drain-id>.jsonl`; set `[burndown] verbose = true` in `.aida/config.toml` or `~/.aida/config.toml` to make that the default, and pass `--quiet` for the legacy buffered launch. `burndown status` reads the headless `burndown run` lock; the in-process `queue work --auto-complete` orchestrator has its own richer window at `aida drain status` (member-by-member phases). **While a drain is live**, both `burndown plan` and `aida queue list` lead with a `⚡ a drain is running (pid N)` banner and mark the specs it owns — `▶ in-flight` (an implementer is leased on it now) vs `◷ scheduled` (claimed, not yet picked up) — so a running drain doesn't read as a fresh, idle ready set. The signal is the same `.aida/drain.lock` the drain writes (no parallel liveness probe), so the marking and `burndown status` never disagree.

**Ordering.** The ready set is fanned **highest priority first** (high > medium > low), with **queue-insertion order** (each spec's queue-entry `added_at`) as the tiebreak — a high-priority bug queued after a low-priority chore still drains first, and equal-priority specs drain in the order you queued them. Ordering never changes eligibility: a blocked spec still waits regardless of priority, and `serialize:<group>` members are still sequenced one per wave. Set `[burndown] order = "queue"` — or pass `--order queue` to `burndown plan` / `burndown run` for one invocation — to drain in **strict queue-insertion order**: oldest-queued first, no priority reordering. Insertion time is a fact about the queue, not about spec-id spelling: under `queue`, a spec whose id sorts *later* alphabetically drains first if it was queued first.

```toml
[burndown]
verbose = true
order = "priority"   # or "queue" for strict queue-insertion order
```

**Chains with** — `backlog groom` (fill the queue) → `burndown plan` (verify the ready set) → `/aida-burndown` (run it) → `burndown status` / `drain status` (watch).

---

### `aida solo`

**One line** — run your project solo: one leave-it-running command that grooms → implements → integrates the safe backlog end-to-end.

**Mental model.** Solo is the single-operator drain. With no team and no concurrent advisor, *you* are the advisor and the integrator both — `aida solo run` takes that role for you: it works the **safe backlog** on a cadence (garden → assess/queue → implement → integrate → repeat) with maximum discretion, while **parking keystone / architecture-class work** for you to look at rather than shipping it unattended. Bare `aida solo` (no action) just sets the visible work-state flag — a timestamped `~/.aida/solo.toml` with a 24h safety TTL that the statusline surfaces — without starting the loop; `aida solo run` starts the loop; `aida solo stop` ends it; `aida solo status` reports whether either is active.

**Reach for it when** — you're working alone and want the backlog drained without babysitting each spec: kick off `aida solo run` and let it cycle. Use bare `aida solo` when you only want to *mark* yourself as the solo operator (lighting the statusline marker) so other surfaces know maximum-discretion mode is in effect.

**Don't reach for it when** — you have a concurrent advisor or want a parallel worktree-isolated fan-out (that's `/aida-burndown`); or the work is keystone/architecture-class (solo deliberately parks it for you).

**Gotchas.** The solo flag carries a 24h TTL by default so it can't silently linger forever — override with `--ttl 8h` (etc.). `aida solo run --dry-run` runs ONE tick that *prints* the cycle it would execute, then exits — verify the loop without a live drain. `--interval <SECS>` tunes the cadence between cycles (default 300). Ctrl-C, `aida solo stop`, or the TTL all stop a running loop.

**Chains with** — bare `aida solo` lights the statusline marker → `aida solo run` works the safe backlog → `aida solo status` checks it → `aida solo stop` ends it; the parked-keystone items surface via `aida human`.

<!-- doc-intent: TASK-879 TASK-880 -->

---

### `aida drain`

**One line** — inspect the `--auto-complete` drain that's currently running.

**Mental model.** A drain is a long-lived process walking the queue through the full lifecycle. `drain status` is the window into it: what command launched it, the batch members and each one's lifecycle state, the current phase, and — crucially — **a prediction of what happens to the queue when the current session exits**.

**Reach for it when** — a drain is running and you want to know "where is it, and is it safe to close this terminal?" Prints `No drain in progress.` (exit 0) when none is running, so it's safe to poll.

**Don't reach for it when** — you want to *start* a drain (`queue work --auto-complete`) or see the *ready set* before one starts (`burndown plan`). `drain` is observation of an in-flight run only.

**A spec won't drain?** If the queue says it needs a guided/operator session, run `aida derisk <ID>` to launch the advisor de-risking workflow for that spec.

**Chains with** — the observation counterpart to `queue work --auto-complete`; pairs with `aida findings` to triage anything the drain shelved.

---

### `aida away` · `aida home` · `aida presence`

**One line** — tell AIDA whether you're at the keyboard, so autonomy can adjust.

**Mental model.** Presence is a machine-global, daemon-free state (a timestamped `~/.aida/presence.toml`). It's **advisory input to mode selection**: while you're `away`, an `aida queue work --auto-complete` with no explicit `--no-human` flag *defaults* to a headless drain — because there's no one to answer a prompt. Explicit flags always win, and integrity gates (scope-ack, CI, merge-on-green) *always* apply regardless. `home` clears it (and an interactive command auto-flips you back).

**Reach for it when** — `away` before you walk away from an unattended overnight drain (so it doesn't sit waiting on a prompt nobody will answer); `home` when you return; `presence` to check the current effective state + TTL.

**Don't reach for it when** — you want to *force* headless vs interactive regardless of presence — pass the explicit `--no-human` / `--escalate-*` flags instead; presence is the *default*, not an override.

**Gotchas.** `away` has a TTL (default 8h) — it lapses on its own so you don't get stuck headless forever. Tune the away-drain behavior under `[presence]` in `.aida/config.toml`:

```toml
[presence]
consumers  = "on"              # master switch (default on) | "off"
away_drain = "headless-both"   # default | "headless-escalate-defaults" | "headless-park"
escalation = "park"            # punt handling, its OWN knob — "defaults" ships the defensible default, "park" shelves NeedsAttention. UNSET = derive from away_drain (headless-park → park, else defaults).
home_offer = "surface"         # home-side (default surface) | "dont-block"
active_within = "15m"          # last-human-input oracle: gap below this reads "active"
stale_after   = "2h"           # gap at/above this reads "stale"; "idle" between. Accepts "15m"/"2h" or integer seconds.
```

`escalation` is decoupled from `away_drain`: you can run a max-throughput `headless-both` drain but still `park` punts for triage (or the reverse) — previously the punt-handling default rode the `away_drain` rung and could not be picked independently. Leaving `escalation` unset reproduces the historical coupled behavior exactly. (`aida human away/home/presence` are the same verbs under the `human` role vector — same state, different front door.)

**Last-human-input oracle.** Separate from the explicit `home`/`away` intent above, AIDA passively observes when the operator last typed a prompt. The per-turn `aida awaiting --notice` hook (wired on `UserPromptSubmit`) stamps a per-session last-prompt timestamp under `~/.aida/turn-clock/<session-id>.toml`, and prepends a line to every turn — `Current date/time: … . Timing: first prompt of this session | continuation (Xm since last prompt).` — so the agent always has fresh time + cadence context (this replaced the trial `~/.claude/hooks/inject-time.sh`). The most-recent stamp across sessions is the machine-wide oracle: `aida human presence` and `aida ps` report `operator last seen <Nm/Nh> ago — active/idle/stale`, with the Active/Idle/Stale bands tuned by `active_within` / `stale_after`. This gives the escalation cascade a signal for whether an interactive ask is answerable (operator active) or should park / go headless (stale).

**Chains with** — `away` → `queue work --auto-complete` (now defaults headless) → `home` → `aida status` surfaces what's waiting for you.

---

### `aida human`

**One line** — the front door to "what needs a person?"

**Mental model.** The human is the **permanent terminus of the escalation cascade** (implementer → advisor → human). `aida human` is that role's first-class home, symmetric with the agent roles: bare `aida human` shows the bottleneck view — every spec a human must decide, review, or triage — grouped by *why*. It's the same set as `aida list human`, but organized around the question "what's stuck on me?"

**Reach for it when** — you come back to the keyboard and want the one view that answers "what's waiting specifically for me?" `aida human unblock` emits a paste-ready advisor prompt that grooms the open items keeping *themselves* out of the burn-down.

**Don't reach for it when** — you want everything open (that's `list`), or the async decision questions specifically (that's `questions`). `human` is the *grouped-by-why bottleneck*, a superset view.

**Chains with** — `human` → `questions answer` (decisions) / `review` (reviews) / `triage` (drafts) / `findings` (shelved drains).

---

### `aida triage`

**One line** — the disposition lease: "one disposing advisor per scope."

**Mental model.** Triage is *intake disposition* — deciding draft specs' fates (approve / reject / park). The authority gate decides *who* may dispose; the **triage lease** decides *how many* — exactly one live advisor per scope, so two advisors don't dispose the same inbox concurrently. `triage acquire` takes the lease (refused, naming the holder, if someone holds it); per-scope, so non-overlapping subsystem advisors can dispose in parallel.

**Reach for it when** — you're about to clear the draft inbox and want to claim the scope so a sibling advisor doesn't double-dispose it. (The clearing itself is the `/aida-triage` skill + `edit --status approved`; this is the *lock* around it.)

**Don't reach for it when** — you're a solo operator with no concurrent advisor (the lease is collision-avoidance; alone, you don't need it). And note it's distinct from `findings`/`questions` triage — this is the *draft-disposition* gate specifically.

**Chains with** — `triage acquire` → dispose drafts (`/aida-triage`) → `triage release`.

---

### `aida questions`

**One line** — the async decision inbox you answer outside any agent.

**Mental model.** A parallel surface to code review: code review gates *implementations*, the questions inbox gates *decisions* — the design forks that block a spec. Two ways one gets created: the **ask-ahead** path (`questions ask`, often seeded by `questions sweep`) is where the *advisor* pre-distills a vague `needs-human` spec into a structured question + enumerated choices, so you later drain it with a **pure pick — no agent, no LLM session**; the **live** path (`questions clarify <spec>`) spins an agent to interrogate you and generate options *now*, reserved for specs too under-specified to pre-enumerate. The intended rhythm is **ask-ahead, answer-async**. Answering a question **applies** its resolution (binds acceptance / clears a gate / rejects) and **auto-queues** the now-decision-free spec onto the burndown ready set — so the decision inbox and the work drain are symmetric: `burndown run` drains the decision-*free* set (needs no human), `questions answer` drains the decision inbox (needs *only* the human).

**Reach for it when** — you're back at the keyboard and want to clear the decisions agents escalated to you — the cheapest possible human-in-the-loop (no session to spin up, just answer).

**Don't reach for it when** — the thing isn't a *distilled question* but a *shelved failure* (that's `findings`) or a *parked design-fork on a specific spec* (that's `punt`/`NeedsAttention`). Questions are the curated, answer-this-and-move-on inbox.

**Chains with** — populated by autonomous drains hitting forks; answering one can unpark a spec back into the ready set.

---

### `aida findings`

**One line** — triage the findings filed by drain phases (and your own observations).

**Mental model.** When a headless drain phase shelves a spec (CI red, RequestChanges, build fail), it files a **finding** — a triageable record of "this needs a human look." `findings` is that queue. You can also `findings add` your *own* observations — a pattern spotted in a live session, captured before the context decays, to promote/dismiss/recur later.

**Reach for it when** — after an autonomous drain, to see what got shelved and why (`findings list`); or to capture an advisor observation you don't want to lose (`findings add`).

**Don't reach for it when** — the item is a *decision to make* (that's `questions`) rather than a *failure to triage*. Findings are about outcomes that went sideways; questions are about choices not yet made.

**Counting defect classes.** `findings classes` answers "how often does this kind of defect recur?" from recorded review verdicts. A reviewer can tag each finding with a class from a small fixed vocabulary when recording a verdict (incomplete-fix, fail-open, absent-evidence-reads-as-good, untested-path, contract-drift, race, perf, portability, stale-base, scope-creep); this report counts them across every verdict, archived rounds included, each act of review once. Limit it to a recent window with `--since` (`30d`, `12h`, or a timestamp) and get machine output with `--json`. Findings recorded before classes existed stay unclassified and are counted as such; free text is never classified after the fact, because a keyword match cannot tell an approval from a refusal. `aida review classes` prints the same report.

**Gotchas.** A finding is the right home for a not-yet-confirmed pattern; promote it at recurrence (the counter survives in a `recurrence:N` tag).

**Chains with** — filed by `queue work --auto-complete` shelving; triaged into rework (`aida rework`) or dismissal.

---

### `aida punt`

**One line** — pause a spec at a design-fork with a structured reason.

**Mental model.** `punt` is the **safety net an autonomous drain reaches for when it hits a decision it cannot safely make**. Instead of guessing past a fork, it moves the spec to **NeedsAttention** with a `--category` and `--reason` — a structured park, not a silent stall. (This is also *the* way to set NeedsAttention; `edit --status` won't do it, because a punt without a reason is useless.)

**Reach for it when** — you (or an agent) hit a genuine fork mid-implementation and the right move is "stop and flag," not "decide blindly." The spec must currently be In Progress.

**Don't reach for it when** — you actually know the answer (just proceed) or the spec is fine and you want a note (`comment add`). Punt is for *real* "I cannot safely continue" forks.

**Chains with** — punt → the spec surfaces in `findings` / `human` → a human resolves it → `aida rework` or unpark back to ready.

---

### `aida autonomy` · `aida sandbox` · `aida goal`

These three support the autonomy machinery rather than driving work directly.

**`aida autonomy`** — calibration + maturity views. `autonomy calibration mismatches` surfaces where the complexity-calibration predicted wrong; `autonomy report` is the human-intervention maturity report (how often drains had to stop and ask). **Reach for it when** you want to *measure* how autonomous the drains actually are, and where they keep needing you. Not part of the daily loop — a periodic health lens.

**Sandboxed trial runs** — use a throwaway, discardable git-canonical store under a temp dir when you want to drain-test or scenario-play *without touching your real store* — the safe place to try `aida queue work --auto-complete` or seed test specs. Don't use a disposable store for real work — by definition it's discardable.

**`aida goal`** — derive a *machine-checkable* completion condition from spec metadata, ready to paste into `/goal` or `/schedule`. Each flag is one clause (`aida goal --batch`, `aida goal --epic`, `aida goal --queue-empty`, …); flags compose with AND; every clause carries an explicit verification command. **Reach for it when** you want a loop/drain to stop on a *deterministic* condition rather than a vague "make it pass." **Don't** pick a clause whose mechanism your drain bypasses (e.g. an `aida goal --queue-empty` condition met trivially because autonomous-merge skipped that queue) — the clause must match how the work actually routes.

### `aida schedule` (alias `aida cron`)

The per-seat **job registry**: which periodic jobs each seat runs, whether one is overdue, and who last ran it. One command, one registry, two kinds of job:

<!-- trace:TASK-176 | ai:codex -->
- **substrate** — `command = "session reap"` (an allow-listed `aida` subcommand: `cache verify`, `session reap`, `queue gc`, `notify check`, `doctor`, `doctor check performance --fail-on-findings`, `doctor check remote-drift --fail-on-findings`, `doctor check stale-remote-branches --fail-on-findings`, `doctor check disk-headroom --fail-on-findings`, `doctor check runaway-seats --fail-on-findings`, `fetch --code-only`, `store compact`). Needs no LLM. `aida schedule tick` runs the due ones (the per-turn hook calls `aida schedule tick --hook`, which skips network-touching jobs) or paste `aida schedule emit-cron` into a real crontab. Idempotent, quiet when idle. The `aida doctor check <category> --fail-on-findings` shapes are **gates**, not reports: a non-zero exit emits `CronJobFailed`, which a paired `on = ["CronJobFailed"]` seat job turns into a due item — see `hub-drift-guard`, `stranded-branches-guard` and `disk-headroom-guard` in the scaffolded `[schedule]` section (commented by default; this repo enables `hub-drift-guard` and `disk-headroom-guard`, and ships `stranded-branches-guard` disabled until its remote-branch backlog is healed) for the worked pattern. <!-- trace:STORY-1367 | ai:claude -->

  **Runaway-seat watchdog** (`watchdog` job, `aida doctor check runaway-seats`): a zero-token substrate check over this project's Claude Code transcripts (`~/.claude/projects/<slug>/`, for the project root and every worktree) and its headless logs (`.aida/headless-logs/`, `.aida/burndown/`). Each run reads only the bytes appended since the last one into per-session per-minute aggregates kept in `.aida/watchdog/`. It trips per session on the runaway shape (too many wakes, one injected prompt repeated or firing faster than its declared cron, mostly no-op replies, a model-side mailbox poll), on token rate and compactions far above an ordinary busy seat, and on the project's 24h spend nearing `daily_token_budget`. Each finding names the session, its seat and pid when the agent registry knows them, the rule, the measured value, the threshold and the window; a repeated prompt is named by a short hash, never its text. Each rule reports once per crossing and again only after it clears. A run cut short by its byte/time budget is shown as degraded evidence and resumes next run; no readable evidence is shown as unknown; neither makes the gated job exit non-zero or pages a seat. Thresholds live under `[watchdog]` in `.aida/config.toml`: `window_minutes` (60), `max_wakes_per_hour` (120), `max_tokens_per_hour` (500000000), `daily_token_budget` (6000000000), `budget_alert_pct` (90), `max_repeated_prompts_per_hour` (60), `max_idle_ratio_pct` (80), `idle_min_replies` (20), `noop_reply_max_chars` (60), `restart_context_tokens` (0 = off), `max_compactions_per_day` (4), `max_bytes_per_file`, `max_total_bytes`, `time_budget_ms` (10000). <!-- trace:STORY-1462 | ai:claude -->

A registered job only ever runs when something invokes `aida schedule tick` — nothing does that by default. `aida schedule install-cron` installs a marker-keyed, idempotent crontab entry (Linux/macOS) that ticks every 15 minutes, appended without touching the rest of your crontab; `aida schedule uninstall-cron` removes exactly this repo's entry. `aida init`, at a TTY, offers to install it (default **no** — writing to your crontab is a real side effect — and it shows the exact line first); non-interactive init never prompts, and `aida init --no-schedule` skips the offer entirely on a TTY too. `aida doctor` (category `scheduler-driver`) flags a repo with enabled substrate jobs but no installed driver, and separately flags a substrate job overdue by more than 2x its interval (the symptom even when an entry IS installed but broken — wrong PATH, wrong binary, cron disabled). <!-- trace:STORY-1463 | ai:claude -->

**Output contract.** `schedule tick` has no JSON projection — it only ever prints plain human/TOON lines (or nothing, when idle), same as `schedule run`. It never accepts `--format json` / `--json`; passing either is refused before the tick runs at all, not silently downgraded. If you need machine-readable scheduler state, poll `aida schedule status --json` / `aida schedule list --json` / `aida schedule due --json` instead — those leaves do carry a dedicated JSON projection. The installed cron entry logs `schedule tick`'s stdout/stderr to `~/.aida/schedule-tick.log` (plain text, human-readable); that file is self-bounding — a tick call past a few hundred KB truncates its own oldest content rather than growing without limit, so one misconfigured or persistently-failing entry cannot flood it forever. Every `schedule tick` invocation also records `exit_code`/`duration_ms` and, in local usage telemetry (`~/.aida/usage.jsonl`), which driver ran it — `"hook"` (the per-turn hook's `--hook`), `"cron"` (the installed crontab entry, tagged via `AIDA_SCHEDULE_INVOKER=cron`), or `"manual"` (run by hand). <!-- trace:BUG-1600 | ai:claude -->

**Migration note.** A crontab entry installed by an older `aida` build invoked `schedule tick --format json`, which this command has never actually supported — every tick from such an entry failed before doing any work. The next `aida schedule install-cron` (or `aida init`'s TTY offer) detects and rewrites a stale entry in place; nothing to do by hand beyond re-running one of those. <!-- trace:BUG-1600 | ai:claude -->
- **seat** — `prompt = "Triage the mailbox…"`, `seats = ["advisor"]`. Needs the seat's judgment, so the scheduler **never executes it**: it becomes *due* and is delivered as text to whoever holds the seat — the `aida awaiting --notice` per-turn line (`⏰ 1 due seat job — mailbox-triage (every 30m, last 47m ago) → …`), a `DUE JOBS (seat: advisor)` block leading the pickup prompt, and a `## Due Jobs` section in `aida agent new`'s launch context. Identical under every vendor; unattended delivery waits in the shell/event substrate and never creates an in-session recurring model poll. The seat reports back with `aida schedule done <job> [--note "…"]`. <!-- trace:BUG-1589 | ai:codex -->

A job's **schedule** is any of three, on the same entry (`every` + `on` combine — event fast path, interval fallback):

```toml
[[schedule.jobs]]
name = "mailbox-triage"
seats = ["advisor"]
every = "30m"                          # interval heartbeat
on = ["MailReceived"]                  # events already written to .aida/events.jsonl
prompt = "Triage the mailbox: `aida mailbox inbox`."
enabled = true

[[schedule.jobs]]
name = "mailbox-latency"
command = "notify check"
when = "mail.oldest_unread_age > 15m"  # predicate; fires once per episode
enabled = true
```

`when` is a small typed grammar: `field op value`, joined by `&&` / `||` (`&&` binds tighter; parentheses allowed), durations like `90s` / `15m` / `2h`. A condition job fires **once when its predicate turns true and not again until it has been false** (once-until-cleared; the ledger records the episode). The field set is fixed: `mail.unread`, `mail.oldest_unread_age`, `drain.lock_free`, `queue.drain_mode_ready`, `queue.depth`, `sessions.finished_unreaped`, `ci.red_prs` (0 without network), `findings.open`, `escalations.open`. `on` accepts the event names the drain emits (`PrMerged`, `SpecShelved`, `QueueDrained`, `AdvisorEscalated`, `PuntFiled`, `CiTerminal`, `UnshippedWorkDetected`, `MailReceived`, …); a typo in either is refused at config-load time.

**Registry** = the project's `[schedule]` section in `.aida/config.toml` plus the machine-global `~/.aida/schedule.toml` (same shape, or bare `[[jobs]]`), merged by name with the project entry winning. The original `[[schedule.tasks]]` / `interval` spelling still works. **Ledger** = one store object per job on the `aida-store` branch (`schedule/<job>.yaml`: `last_run`, `last_by` seat/session/vendor, `result`, `due_since`, `episode`), so "who last triaged the mailbox and when" has one answer on every clone; writes are debounced (a run clock moving <60 s with the same result is not committed) and merge last-writer-wins.

Verbs: `aida schedule list [--seat advisor] [--json]` (the merged registry, with source layer and last reporter), `aida schedule status [--json]` (every job with status / last run / next due; legacy `.aida/schedules.toml` cadence entries appear as kind `fires_task`, deprecated), `aida schedule due [--seat advisor] [--json]` (what is due now), `aida schedule run [<job>]` (force a substrate job; for a seat job print its prompt), `aida schedule done <job> [--note …]`, `aida schedule tick [--hook]`, `aida schedule emit-cron`, `aida schedule install-cron`, `aida schedule uninstall-cron`.

**Reach for it when** a seat has recurring duties that keep living in one session's memory (a hand-made in-session cron dies with the session), or when a substrate chore (reap, gc, doctor) should run whether or not any agent is alive. **Don't** put a merge or a drain launch in a seat job — a seat job is advice to the seat, never an automatic action; the tick and the gates own those. Cold-booting a headless seat for an overdue seat job belongs to the scheduler tick (a separate story), which consults this registry.

<!-- doc-intent: shaped by STORY-1226 -->

---

### The OS sandbox — `[contained] os_wrap`

<!-- trace:TASK-867 -->

`os_wrap` is the master switch for AIDA's **own** OS-level agent sandbox, built on [bubblewrap](https://github.com/containers/bubblewrap) (`bwrap`). It is distinct from `[contained] enable` (Claude Code's native settings sandbox): `os_wrap` is an OS boundary AIDA itself wraps around the agent process.

```toml
# .aida/config.toml
[contained]
os_wrap = true
```

When `os_wrap = true`, a headless `claude` launch is spawned as `bwrap <confinement-flags> claude …` with:

- ro-bind `/ /` — the whole host filesystem mounted **read-only**, so the agent can read but never write outside its allowed set;
- read-**write** binds for only the code worktree, its sibling `.aida-store`, and the build/auth caches the toolchain needs (`$CARGO_HOME`/`~/.cargo`, `~/.npm`, `~/.claude`, `~/.claude.json`);
- a fresh `/dev`, `/proc`, and a `tmpfs` `/tmp`; die-with-parent semantics so a killed drain leaves nothing behind;
- **shared network** — `os_wrap` is a *write*-confinement boundary, not a network jail (egress is governed separately by `allowed_hosts` / `managed_domains_only`).

It is **strictly opt-in (default OFF)** and **fail-closed**: if `bwrap` is not on `PATH`, or it is installed but the host blocks unprivileged user namespaces, the launch **errors with remediation** rather than silently running the agent unconfined.

> **Current scope: headless drains only.** Today `os_wrap` wraps the **headless** drain paths (`aida queue work --auto-complete --no-human`, the `claude -p` launches). The interactive `aida agent new` launch is **not** yet wrapped — that's tracked separately (deferred until userns confinement is reliable on the dev host). So enabling `os_wrap` confines unattended drains, not your keyboard-driven sessions.

> **Host requirement: unprivileged user namespaces.** `bwrap` needs the kernel to allow unprivileged user namespaces. On recent Ubuntu (23.10+/24.04) AppArmor blocks this by default, so even with `bwrap` installed the sandbox fails-closed until you run `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` (persist it under `/etc/sysctl.d/`). **Discovery path:** `aida doctor` reports whether `bwrap` is available and confinement-capable on this host, and `aida config show` renders the resolved `[contained]` posture (including `os_wrap`) — start there before enabling it.

The full mechanism, the bind list, and the host setup are documented in [`../agents/claude-bubblewrap-sandbox.md`](../agents/claude-bubblewrap-sandbox.md).

### Contained-mode network egress — `[contained] allowed_hosts`

When the **contained** posture is on (`[agents] contained`), an agent's Bash runs inside Claude Code's sandbox (bubblewrap on Linux). By default that sandbox prompts the first time a command reaches a new network domain. To pre-restrict egress to a known allowlist, set a project config:

```toml
# .aida/config.toml
[contained]
allowed_hosts = ["github.com", "api.anthropic.com", "static.crates.io", "registry.npmjs.org"]
```

This injects `sandbox.network.allowedDomains` into the contained settings (the proxy default-denies egress except to these hosts; wildcards like `*.crates.io` work). **It is strictly opt-in:** with `allowed_hosts` unset, the contained settings are byte-unchanged — no network restriction is applied. **Reach for it when** you run unattended drains and want to bound where they can reach.

> **`allowed_hosts = []` (or omitted) means *no restriction*, not "deny all".** An empty list reads like a lockdown but is the unrestricted default — full egress, current behavior. You only restrict egress when the list is **non-empty**, in which case **only** those hosts are allowed.

**Slice-1 limitation:** a non-allowlisted domain *prompts* for approval — fine interactively, but a **headless** `claude -p` drain can't answer the prompt. True block-without-prompt for headless drains needs `network.allowManagedDomainsOnly` via *managed* settings — see the next section.

### Headless hard default-deny egress — `[contained] managed_domains_only`

`allowed_hosts` alone only *prompts* on a non-allowlisted domain, which a headless `aida queue work --no-human` drain can't answer. To get a **hard** default-deny (block without prompt) on the headless path, opt in:

```toml
# .aida/config.toml
[contained]
os_wrap = true              # the bwrap OS-wrapper this rides on
managed_domains_only = true
allowed_hosts = ["github.com", "api.anthropic.com", "static.crates.io", "registry.npmjs.org"]
```

Claude Code only enforces `sandbox.network.allowManagedDomainsOnly` (deny-without-prompt) when it arrives via the **managed-settings** tier, not the project settings file. When `managed_domains_only` is on, the os_wrap launcher generates that managed-settings document (mirroring your `allowed_hosts` into the managed `allowedDomains`, since managed-only honors *only* the managed allowlist) and bind-mounts it read-only over the wrapped process's `/etc/claude-code/managed-settings.json` **inside the bwrap namespace** — so the host `/etc` is never touched and the policy can't be overridden from inside the sandbox. It is **strictly opt-in** and **fail-closed**: with the flag unset the launch is byte-unchanged; with it set, if the bind can't be established the launch errors rather than running egress un-hard-blocked. Requires `os_wrap = true` (it's delivered through the bwrap wrapper).

### Strict read-confinement — `[contained] read_allowlist`

By default the os_wrap bwrap wrapper makes the **whole filesystem readable** (read-only) — write-confinement only. So a rogue drain can't *write* outside its tree but can still *read* host secrets (`~/.ssh`, `~/.aws`, browser cookies) and exfiltrate within the egress allowlist. To tighten reads to a default-**absent** filesystem, opt in with an allowlist of extra readable paths:

```toml
# .aida/config.toml
[contained]
os_wrap = true
read_allowlist = ["/data/shared", "/home/me/.config/some-tool"]
```

When `read_allowlist` is **non-empty**, the wrapper replaces the broad `--ro-bind / /` with an enumerated set: the essential system/toolchain paths (`/usr`, `/lib*`, `/etc`, `/nix`, `/opt`, `/run`, `/var`, …) needed for `claude`/`node`/`cargo` to run, **plus** your allowlist, **plus** the worktree (still read-write). Everything else — including host credential dirs — is simply **not present** in the sandbox. **Strictly opt-in:** with the key absent (the default), behavior is byte-for-byte unchanged. Listed paths use a try-bind, so a listed-but-absent path is skipped rather than aborting the launch. **Stronger but more fragile** — if a toolchain needs a path you didn't enumerate it will break, so tune it live on your deploy distro. Requires `os_wrap = true`.

---

## Where to go next

- **[Chapter 4 — Git & lifecycle](04-git-lifecycle.md)**: what `queue work` produces — branches, PRs, review, and the merge that earns Completed.
- **[Chapter 6 — Roles & sessions](06-roles-sessions.md)**: the seats (advisor/implementer) and the leases that keep parallel drains from colliding.
- **[Chapter 8 — Reporting](08-reporting.md)**: `aida status` and the lenses that show you the autonomy you just ran.
- [`docs/review-process.md`](../review-process.md) — **who reviews in each mode** (manual brief → advisor by hand; `--auto-complete` → reviewer phase; `--no-human` → headless reviewer + cold-boot advisor tier), the fasttrack-vs-review tag split, and the completion handoff loop. The authoritative source for this chapter's review/escalation lore.
- `docs/autonomous-drain.md` is the full practical guide to the drain modes this chapter summarizes.
