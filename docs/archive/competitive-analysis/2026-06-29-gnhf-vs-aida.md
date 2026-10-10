# gnhf vs AIDA — the autonomy layer, head to head

**Date:** 2026-06-29 · **Type:** dated competitive snapshot (autonomy/orchestration lane) ·
**Trigger:** operator flagged `gnhf` as directly adjacent to AIDA's autonomous-drain layer.
**Subject:** `gnhf` (`github.com/kunchenguid/gnhf`, npm `gnhf`, **~2,631★ / 188 forks**, MIT, TypeScript; created 2026-03-31, last push 2026-06-10).
**Author:** Kun Chen (`kunchenguid`) — the same builder behind the **AXI** ecosystem AIDA already incorporated lessons from (`docs/positioning/vs-axi.md`, `docs/competitive-analysis/2026-06-28-axi-ecosystem.md`). gnhf was listed there as a one-liner; this is the deep dive.

> Living-doc note: star counts and capability claims are point-in-time. Where a claim is read off gnhf's own README/source it's stated as such; where it's our inference it's flagged. Refresh signals at the bottom.

---

## TL;DR

`gnhf` ("Good Night, Have Fun") is a **single-objective overnight loop**: you give it one free-text
prompt, it calls a coding agent over and over — commit on success, `git reset --hard` on failure —
until a stop condition (iteration cap / token cap / natural-language `--stop-when` / 3 consecutive
failures) trips. You wake up to **a branch full of commits and a log**. That is genuinely adjacent to
`aida queue work --auto-complete --no-human` / `aida zen` / `aida burndown` — both "tell your agents
goodnight and they work."

**The hypothesis under test — confirmed.** gnhf is **prompt-driven**: the unit of work is one
human-authored objective per run, and "memory" is an unstructured `notes.md` scratchpad plus the git
log. AIDA is **spec-graph-driven**: the unit of work is a typed requirement with a stable ID, the
*backlog itself* is the source of work, and the run drives it through a lifecycle
(implement → CI → review → merge → pull → auto-bump) with traces, dependency-awareness, and
escalation. They occupy the **same "run agents overnight" layer** but from opposite ends: gnhf is the
*dead-simple loop*; AIDA is the *structured, traceable, self-closing pipeline*.

**The honest split:** gnhf wins decisively on **simplicity, install friction, executor breadth, and
adoption** (one npm command, 7 agents + ACP, 2.6k★). AIDA wins on **what it drives and what state it
leaves behind** (a structured backlog, merged-and-green work, code↔spec traceability, multi-spec
coordination). The most useful takeaway is not "who's better" — it's that gnhf has **packaged AIDA's
own most-buried feature** (the autonomous drain) into a viral one-liner, and AIDA should steal the
framing and a handful of its operational-robustness tricks.

---

## What gnhf is (mechanism, pinned down)

gnhf describes itself as a [ralph](https://ghuntley.com/ralph/)- / [autoresearch](https://github.com/karpathy/autoresearch)-style
orchestrator. The mechanism, read from the README + `src/core/` (`orchestrator.ts`, `run.ts`, `git.ts`,
`config.ts`, `exit-summary.ts`, `agents/`):

**Unit of work:** **one natural-language objective per run.** `gnhf "reduce complexity of the codebase
without changing functionality"`. No IDs, no task list, no queue — a single prompt string (or piped
stdin / a whole PRD via `cat prd.md | gnhf`).

**The loop** (one iteration):
1. Validate a clean git tree; create or reuse a `gnhf/<slug>` branch; write `prompt.md`.
2. Build the iteration prompt, **injecting `notes.md`** (accumulated context from prior iterations).
3. Invoke the configured agent **non-interactively**, single-turn.
4. **Success** → `git commit` (one unsigned commit per iteration) + append to `notes.md`.
   **Failure** → `git reset --hard` (rollback), *except* a commit failure preserves the uncommitted
   work and asks the next iteration to repair it.
5. Check caps / failure counter; repeat.

**Stop conditions:** `--max-iterations`, `--max-tokens` (can abort mid-iteration once reported usage
crosses the cap), `--stop-when "<natural-language condition>"` (ends after an iteration whose agent
output reports the condition met), **3 consecutive failures** (configurable `maxConsecutiveFailures`),
or a permanent agent error (e.g. Claude low-credit) which aborts immediately. A complete no-op
iteration counts as a failure toward the abort limit.

**Robustness details (this is where gnhf is mature):** retryable hard agent errors back off
**exponentially**; agent-reported (soft) failures retry immediately; rollback-on-failure with
commit-failure-preserve-for-repair; **system-sleep prevention** for the duration of the run
(`caffeinate` on macOS, `systemd-inhibit` on Linux, a `SetThreadExecutionState` helper on Windows);
graceful double-Ctrl+C interrupt (first = finish current iteration, second = force stop); a permanent
**exit summary** (elapsed, branch, iterations, tokens, diff stats, log paths, review commands); a live
terminal-title status line; JSONL debug logs at `.gnhf/runs/<runId>/gnhf.log`.

**State / memory:** the **git branch is the durable artifact**; `notes.md` is an unstructured
cross-iteration scratchpad the agent reads and appends each round; run metadata (prompt, notes, stop
condition, commit-message convention) lives under `.gnhf/runs/` and is gitignored. There is **no typed
state, no IDs, no relationships, no lifecycle** — the structure is "a sequence of commits toward one
objective."

**Parallelism:** `--worktree` runs each agent in its own git worktree so you can launch **N independent
gnhf processes** on one repo (`gnhf --worktree "A" & gnhf --worktree "B" &`). Crucially these processes
**do not coordinate** — no shared queue, no leases, no dependency edges, no merge ordering. Fan-out, not
a fleet. `--current-branch --push` runs on the current branch and pushes after each success.

**Review / merge:** **none.** gnhf stops at "a branch of clean commits." You review, merge, or
cherry-pick **in the morning, by hand.** It is deliberately a *producer of reviewable work*, not a
self-closing pipeline.

**Single- or multi-vendor:** **single executor per run, broad menu.** `--agent` selects one of
`claude`, `codex`, `rovodev`, `opencode`, `copilot`, `pi`, or any **ACP** target (via the bundled
`acpx` registry, e.g. `acp:gemini`, or a custom ACP command). Per-agent path/arg overrides in
`~/.gnhf/config.yml`. This is genuinely broad agent-agnosticism — but it swaps the *one* executor a run
uses; it does not coordinate several vendors against shared state.

**Distribution / surface:** `npm install -g gnhf` (zero build), plus a bundled **agent skill**
(`skills/gnhf/SKILL.md`) that teaches a *host* agent to drive gnhf in two modes — **Hands-Off** (one
bounded configured run, intervene only on hard failure) and **Companion** (poll the run, re-tighten the
prompt between rounds, treat reviewer findings as the next acceptance criteria). Telemetry on by default
(`GNHF_TELEMETRY=0` to disable). Adoption is real: ~2.6k★, 188 forks.

---

## AIDA's autonomy layer (the thing being compared)

For context (sources in-repo: `docs/autonomous-drain.md`, `aida-cli/src/main.rs` orchestrator,
`auto_complete.rs`, `OVERVIEW.md`):

- **`aida queue work --auto-complete`** — drives **one spec** through the full
  implement → CI → review → merge → pull → auto-bump lifecycle. The three-mode autonomy ladder
  (default / `--zen` / `--no-human`) decides what gets paused-on vs auto-resolved vs punted.
- **`aida zen`** — the single-spec warm autonomous driver (mechanical prompts auto-resolved, real
  design forks still asked).
- **`aida burndown` / the drain** — fan out worktree-isolated implementers over the **ready set** and
  loop until drained; EPIC-28 resilient drain **shelves** a failed spec (`NeedsAttention`), **skips its
  `BlockedBy` dependents**, continues, and exits **code 2** so scripts triage.
- **`aida integrate`** — the continuous "land finished work" integrator seat (rebase → CI → squash-merge
  → pull → escalate).
- **The spec graph is the source of work** — typed requirements with **stable IDs**, typed
  relationships (parent/blocks/references), **`// trace:SPEC-ID` code↔spec comments**, a lifecycle
  state machine, and per-spec history in the orphan-branch git log. The drain doesn't need a per-run
  prompt; it reads the prioritized backlog.
- **Coordination substrate** — queue / lease / inter-agent mailbox / **escalation cascade**
  (headless implementer punts → headless advisor tier → human), pre-work **authority gate** (an agent
  can't self-bless work into the queue).

---

## Side-by-side

| Dimension | **gnhf** | **AIDA autonomy layer** |
|---|---|---|
| **Unit of work** | One free-text objective per run | A typed spec (stable ID) drawn from the backlog; or a batch/epic/ready-set |
| **Source of work** | Human authors a prompt per run | The **prioritized requirement graph** itself (no per-run prompt) |
| **Loop granularity** | One commit per successful iteration, all toward the *same* objective | One spec → one PR → one merge; many specs per drain |
| **State / memory** | `notes.md` scratchpad + the git commit sequence | Typed graph: IDs, relationships, lifecycle states, trace comments, git-log history |
| **Traceability** | Commits + notes; no durable link to intent, no surviving IDs | `// trace:STORY-x` code↔spec links, stable IDs, history events |
| **Review / merge** | **None** — leaves a branch to review by hand in the morning | Reviewer phase + squash-merge + pull + lifecycle auto-bump (self-closing) |
| **End state at "wake up"** | A branch of clean commits awaiting your review | Merged, CI-green, lifecycle-bumped work (further down the pipe) |
| **Parallelism** | N **uncoordinated** processes, each in a worktree | Dependency-aware drain: shelve-and-continue, `BlockedBy` skip, leases, mailbox |
| **Failure handling** | Rollback / exp-backoff / commit-failure-repair / 3-strikes abort | Shelve `NeedsAttention`, `--max-failures`, exit code 2, escalation cascade |
| **Stop condition** | `--max-iterations` / `--max-tokens` / NL `--stop-when` / failure cap | `aida goal` machine-checkable conditions; queue-empty; lifecycle state |
| **Executors** | **7 agents + ACP** (claude/codex/copilot/pi/rovodev/opencode/gemini…) | Effectively **Claude-first** for the drain; cross-vendor via mailbox, not executor swap |
| **System-sleep prevention** | **Yes** (caffeinate/systemd-inhibit/Win helper) | **No** (not implemented) |
| **Install / friction** | `npm i -g gnhf`, clean git tree, go | Build/install a Rust binary, `aida init` (orphan branch + cache + scaffold) |
| **Adoption** | **~2.6k★ / 188 forks** | Single-project, pre-distribution |
| **Integration model** | One sharp tool, compose-your-own | One integrated system (graph + queue + drain + lifecycle) |

---

## The differentiation verdict

**The hypothesis holds.** gnhf is prompt/objective-driven "run an agent overnight"; AIDA is
spec-graph-driven — it drives the *requirement backlog* with IDs / traces / lifecycle / coordination, so
the work is **structured and traceable** rather than ad-hoc. Concretely:

1. **What drives the run.** gnhf needs a human to author one objective per run and to keep re-authoring
   it (Companion mode is literally "re-tighten the prompt between rounds"). AIDA's drain reads a
   prioritized, typed backlog and works items in dependency order with no per-run prompt. *This is the
   real wedge:* gnhf orchestrates **a prompt**; AIDA orchestrates **a backlog**.

2. **What you wake up to.** gnhf leaves **a branch to review** — the loop stops short of review and
   merge by design. AIDA's drain leaves **merged, CI-green, lifecycle-bumped** work behind a reviewer
   gate, with code↔spec traces intact. AIDA closes more of the loop; gnhf hands the last mile back to
   you.

3. **Coordination vs fan-out.** gnhf's parallelism is N uncoordinated processes; nothing prevents two
   from colliding semantically, nothing orders their merges, nothing skips a dependent when its blocker
   fails. AIDA's drain is a coordinated fleet (leases, dependency-aware shelving, escalation, mailbox).

4. **Durable intent.** gnhf's memory is `notes.md` — useful, but unstructured and thrown away with the
   run. Notably AIDA takes the **opposite stance**: it treats an agent's own scratchpad as *not* ground
   truth (the "substrate-as-bouncer" banner, BUG-378) and forces state through the typed store. gnhf
   trusts the scratchpad; AIDA distrusts it and programmatically gates on the store. That's a real
   philosophical fork, and it's why AIDA can answer "why does this code exist / what shipped this spec"
   months later and gnhf cannot.

**Where gnhf is genuinely better (be fair):**

- **Simplicity / time-to-value.** One command, one sentence, clean tree — done. This *is* the
  "I could do this in 20 lines of bash" surface AIDA's Trojan-horse framing embraces, except gnhf
  shipped it, polished it, and 2.6k people starred it. The onboarding cliff is near-zero.
- **Install friction.** `npm i -g` vs build-a-Rust-binary + orphan-branch init. AIDA loses this badly.
- **Executor breadth.** 7 native agents + ACP out of the box. AIDA's drain is Claude-first.
- **Operational robustness for unattended runs.** Exponential backoff, commit-failure-preserve-for-repair,
  mid-iteration token-cap abort, and especially **OS sleep prevention** — all mature, all directly
  relevant to overnight reliability. AIDA has analogues for some but not sleep-prevention.
- **The framing.** "Before I go to bed, I tell my agents: good night, have fun" is the single best piece
  of marketing in this entire lane — and it markets *exactly AIDA's autonomous-drain feature*, which
  AIDA buries under `queue work --auto-complete --no-human` and a glossary of machinery vocabulary.

**Where AIDA is differentiated vs just more complex.** The complexity is real and not free, but it buys
things gnhf structurally cannot have without becoming AIDA: stable IDs that survive edits, code↔spec
traces, a typed dependency graph that makes the drain skip the right work, an enforced lifecycle that
closes the loop to merged-on-main, and a coordination substrate for multiple concurrent specs. gnhf's
own SKILL warns "stop condition met ≠ user acceptance" and tells the host agent to re-verify and
re-prompt — i.e. gnhf *outsources to a human* exactly the structure (acceptance criteria, review gate,
durable intent) that AIDA encodes. That's the line between *differentiated* and *merely complex*.

**Is either redundant?** No, not as substitutes — they're different layers (loop vs substrate). But at
the narrow "run agents overnight" overlap they **are** near-substitutes, and there the trade is stark:
**gnhf is simpler and broader; AIDA is more complete and more traceable.** A solo dev wanting "improve
this app while I sleep" is better served by gnhf today. A team wanting "drain our prioritized,
traceable backlog overnight and wake up to merged work" is AIDA's case — *if* AIDA closes the
packaging gap.

---

## What AIDA should learn (top 3, then the rest)

1. **Steal the framing and ship a one-sentence overnight entry point.** gnhf packaged AIDA's most
   strategically important and most-buried capability (the autonomous drain) as a viral one-liner.
   AIDA already has `aida zen` and `--auto-complete --no-human`; what it lacks is the **dead-simple,
   emotionally-resonant front door**. The Trojan-horse thesis says the simple surface *is* the
   adoption funnel — gnhf is proof. Action: a `aida goodnight` / `aida zen`-grade single command with
   a "tell your agents goodnight" framing, with the graph/lifecycle depth surfacing only on use.

2. **Adopt gnhf's unattended-run operational robustness.** Concrete, cheap, and directly improves the
   `--no-human` drain's overnight reliability: **system-sleep prevention** (caffeinate / systemd-inhibit
   — AIDA has *none* today, and a laptop sleeping mid-drain is a real overnight-failure mode),
   **exponential backoff on retryable agent errors**, **commit-failure-preserves-work-for-repair**, and
   a **permanent exit summary** (elapsed / iterations / tokens / diff stats / review commands). File
   these as TASKs against the drain.

3. **Broaden the executor menu via ACP.** gnhf's agent-agnosticism (7 agents + the bundled `acpx` ACP
   registry) is broader than AIDA's Claude-first drain. AIDA's cross-vendor story lives in the
   *mailbox* (durable coordination record), which is the right moat — but the *executor* side of the
   drain should learn from gnhf's ACP-target abstraction so a drain phase can run Codex/Gemini/etc.
   without bespoke per-agent plumbing. (Pairs with the existing cross-vendor thesis.)

Secondary / smaller:

- **`notes.md`-style lightweight cross-iteration scratchpad** is worth a deliberate position, not just
  rejection. AIDA distrusts the scratchpad on purpose (BUG-378) — correct for *ground truth* — but a
  bounded, append-only running-notes file *as input context* (clearly non-authoritative) is a cheap way
  to carry "what I tried last iteration" without round-tripping the store. Decide explicitly rather than
  by omission.
- **Natural-language `--stop-when`** is more ergonomic for casual use than `aida goal`'s machine-checkable
  conditions. AIDA's is more rigorous (deterministic, scriptable) — keep it as the default, but a
  NL-condition convenience wrapper lowers the bar for first use.
- **The host-agent SKILL (Hands-Off / Companion)** is a clean pattern: an outer agent *steers* the
  overnight worker and treats review findings as the next acceptance criteria. AIDA's advisor/escalation
  cascade already does the richer version of this; the lesson is the **packaging as a skill** other
  agents load on demand — same zero-install distribution lesson as AXI.

**What stays AIDA's moat (don't dilute):** the typed graph, stable IDs, code↔spec traces, the enforced
lifecycle that closes the loop to merged-on-main, and the coordination substrate. gnhf has none of these
and would have to become AIDA to get them. Borrow gnhf's *surface and operational polish*; keep AIDA's
*substrate*.

---

## Refresh signals to watch

- **gnhf grows structure** — IDs, a task list/queue, dependency edges, or any review/merge step → it
  moves from "loop" toward "pipeline" and the overlap with AIDA widens. Tripwire.
- **gnhf coordinates its parallel worktrees** (shared queue / leases / merge ordering) → it grows a
  fleet; re-evaluate the coordination wedge.
- **Star velocity / the AXI cluster converging** — gnhf + no-mistakes + firstmate + tasks-axi forming
  an integrated suite (see `vs-axi.md`); the cluster gaining a graph is the standing tripwire from the
  AXI note.
- **AIDA ships the "goodnight" front door** and/or sleep-prevention → close the learn items above and
  note here when done.
- **ACP adoption broadens** — if ACP becomes the de-facto cross-vendor executor protocol, AIDA's
  Claude-first drain ages faster.

## Related

- `docs/positioning/vs-axi.md` · `docs/competitive-analysis/2026-06-28-axi-ecosystem.md` — same author, the AXI ecosystem (gnhf is a member).
- `docs/autonomous-drain.md` — AIDA's `--no-human` drain (the compared capability).
- `docs/competitive-analysis/marketplace-roster.md` — section B (orchestration); gnhf row added.
- `docs/competitive-analysis/2026-06-12-beads-gastown-vs-aida.md` — adjacent orchestration-layer comparison.
