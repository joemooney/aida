# AIDA's control plane against durable-execution engines and merge queues

<!-- trace:TASK-1525 | ai:claude -->

**Audience:** contributors, reviewers of architecture sketches, and whoever
picks what the control plane is benchmarked against. This is an architecture
comparison, not a market comparison: AIDA is not sold as a workflow engine or a
merge queue, and no `docs/positioning/vs-*` page should be derived from it.

**What is being compared.** AIDA's *control plane* is the layer that decides
whether work may advance and reports the state of work in flight — the queue,
the drain, the orchestrator and its phases, seats, leases, worktree
assignment, review verdicts, merge-holds, `BlockedBy` gating, required-check
rollups, the event feed, and the surfaces that report on all of it (`aida ps`,
`aida drain status`, `aida status --awaiting`, `aida doctor`). The substrate defines it
as `TERM-5` (`aida show TERM-5`), amended on 2026-09-26 to name it the
**corpus-integrity layer**: it exists to keep the intent store true while
unreliable agent workers change the code. The store and its intent are the
product (`VIS-2`, `aida show VIS-2`); the control plane is the means. It takes
about half of AIDA's engineering (50.9% of attributed changed lines over the 30
days measured by the [2026-09-24 positioning
deep-dive](../positioning/2026-09-24-spike-86-positioning-from-engineering.md)),
and it has never been set beside the two families it most resembles:

1. **Durable-execution engines** — Temporal, Cadence, Azure Durable Functions,
   Restate, Inngest, and the workflow schedulers that share their vocabulary
   (Airflow, Argo, Prefect): an append-only event history per workflow,
   deterministic replay to rebuild state after a crash, checkpointing at each
   step, activity retries with backoff, and timers that survive process death.
2. **Merge queues** — Bors, Zuul, Mergify, Aviator, and the GitHub merge queue:
   an admission controller in front of the default branch that tests each
   candidate on top of the branch it will actually land on, serialises or
   batches merges, and refuses to land anything whose checks are not green on
   the current head.

The one fact that shapes every row below: **AIDA's workers are
non-deterministic LLM agents.** A Temporal activity re-run against the same
inputs is expected to produce the same side-effect; an implementer session
re-run against the same spec produces a different branch. The control plane
therefore borrows the *integrity* properties of both families and deliberately
declines the *replay* properties that assume deterministic workers.

The rest of this document is two tables — one per family — each with three
columns: what AIDA already has (and which command or module provides it), what
it lacks, and what it deliberately does not want. A closing section answers the
question the round-2 moat document left open: does a durable-execution property
close gap **P1, resumable orchestrator checkpointing**?

---

## 1. Durable-execution engines

The unit of comparison is one spec's lifecycle: `implement → CI → review →
merge → pull → build`, run by the single per-spec engine
`auto_complete::orchestrate_with_resume` over the `Phase` enum
([one-orchestration-engine.md](one-orchestration-engine.md)). A Temporal
workflow is the closest analogue: a long-running function whose steps are
activities with retries, whose progress is an event history, and whose state is
rebuilt by replay.

### 1.1 What AIDA already has

| Durable-execution property | AIDA equivalent | Where it lives |
|---|---|---|
| **Append-only event history** per workflow | `.aida/events.jsonl`, one structured line per drain state change: `PhaseEntered` (with its own `attempt` number and seat), `SpecRetried`, shelves, punts, outcomes. Consumers key on values, not adjacency (BUG-1290). | `aida-cli-lib/src/events.rs`; the drain and the shift block on predicates over the feed (`event_wait.rs`: `wait_for_actionable`, `wait_for_queue_drained`) instead of polling |
| **Checkpoint per step** | `.aida/drain-state.json` records the launching command, batch membership, the current spec, phase and attempt, and PR/run ids. It is *removed* on a clean exit; its presence means live or crashed. | `drain_state.rs`; `aida drain status` |
| **Crash resume at the last completed step** | `aida queue work --resume-drain` reads the drain-state file, probes reality (git, PR, spec status, PID liveness) and re-enters the phase loop at the right phase rather than restarting phase 1 (STORY-491/492). `--resume-dry-run` previews without acting (BUG-1517). | `drain_resume.rs` (pure resumability + re-entry-phase decision); `drain_cmd.rs` (the `--resume-drain` entry point) |
| **Activity retries with a bounded budget** | Transient phase failures (`watchdog`, `no-verdict`, `no-pr`, `tool-exit`, `cache-locked`) are retried once by default (`[drain] retry_transient`, capped at 3); a retry re-enters only the failed phase. Non-transient verdicts are never retried. | `docs/autonomous-drain.md` → *Transient self-retries*; STORY-975 |
| **Retry-with-backoff around unreliable I/O** | `gh`/`git` subprocess failures in phases 3–6 are classified transient vs permanent by a configurable stderr allow-list and retried with backoff; every attempt is surfaced to the user and the drain-state file. `ETXTBSY` spawn races are retried at the process layer. | `network_retry.rs` (BUG-286); `process_retry.rs` |
| **Failure is a typed outcome, not an exception** | A failed phase *shelves* the spec: `NeedsAttention` + a structured `FailureReason` (`phase`, `kind`, `detail`, `recovery_hint`), dependents are skipped via `BlockedBy` pickability, and the batch continues (EPIC-28). Exit 2 + `aida findings list` for triage. | `docs/autonomous-drain.md` → *Shelving on failure*; `requeue.rs` for the `NeedsAttention → back-in-flight` transition |
| **Durable timers / watchdogs** | The phase watchdog fails a phase that made no commit or file change for the no-progress window and records `watchdog` as the failure kind; `AIDA_WORKER_SPEC_TIMEOUT` bounds a whole spec in the worker loop; integration waits belong to the launcher, not to an agent turn (TASK-1169). | `auto_complete.rs` (`Watchdog` failure kind); `docs/autonomous-drain.md` → *Watchdog* |
| **Automatic re-drive of transient parks with backoff** | The re-drive supervisor scans `NeedsAttention` parks, re-drives only the typed *transient* causes on a capped, backed-off loop, and leaves genuine needs-human parks for escalation (STORY-1051, ADR-26). | `supervisor.rs` |
| **Single-writer / mutual exclusion per workflow** | One drain per repo (`.aida/drain.lock`, BUG-538); one worktree per spec via leases (`worktree_lease.rs`, `worktree_lock.rs`); one merge at a time (`merge_lock.rs`). | `drain_lock.rs`, `worktree_lease.rs`, `merge_lock.rs` |
| **Durable "what happened" after the process is gone** | `.aida/last-drain.json` carries the finished-drain tally so `aida status` can answer "I walked away — what happened?" (STORY-730). | `last_drain.rs` |
| **A recovery wizard for the one step that cannot be replayed** | `aida queue recover <id>` turns the state after a failed implementer session (529, commit-without-PR, unpushed commits, dirty worktree) into a deterministic recommended action (STORY-384). | `queue_recover.rs` |

### 1.2 What AIDA lacks

| Durable-execution property | Status in AIDA | Would it help? |
|---|---|---|
| **Deterministic replay of the workflow function** from its event history (Temporal's core mechanism: the workflow code re-executes and the history answers every `await`) | Absent. `--resume-drain` *probes present reality* (git, PR, spec status) and picks a re-entry phase; it does not replay the history. | Not for phase 1 (see §1.3). For phases 2–6 the probe-and-re-enter design already gives the property replay would give, because those phases are idempotent over forge state (a PR that is merged stays merged). |
| **Workflow versioning / patching** (change the workflow code while histories from the old version are still in flight) | Absent. A drain launched by an older binary keeps running the older phase loop; the substrate's defence is `merge_hold.rs` (fail-closed against a stale-binary merger), not history migration. | Low value while one drain per repo is the rule. Becomes relevant if long-lived multi-day drains outlast releases. |
| **Per-activity heartbeats and heartbeat timeouts** | Partial. Session liveness (`session_liveness.rs`, `session_reap.rs`) is PID- and substrate-state-driven, not a heartbeat the worker emits. A hung-but-alive agent is caught only by the phase watchdog. | Yes, modestly: a worker-emitted heartbeat would distinguish "thinking" from "hung" earlier than the watchdog does. Tracked under the dispatch-health surface (`dispatch_health_ps.rs`) rather than as a workflow primitive. |
| **A queryable history of *every* workflow ever run** (Temporal's visibility store) | Partial. The event feed is per repo and rotates; the spec's own history is in the orphan-branch git log (`aida history events`). There is no cross-drain index of runs. | Marginal. The spec graph, not the run log, is the record that matters (`VIS-2`); run-level analytics are the `/aida-insights` skill's territory. |
| **Child workflows and signals** as first-class primitives | Partial. Batches are a list walked by one loop; the advisor escalation tier and the punt ledger act as signals (`resume_after_advisor`), and directives (`.aida/worker.cmd`) act as external signals to the worker loop. There is no general "signal a running phase" API. | Not wanted as a general API (see §1.3); the specific signals AIDA needs are the sentinel files and the mailbox. |
| **Exactly-once side-effects** across a crash boundary | Absent as a guarantee. Phases 3–6 are made idempotent by construction (create-if-needed PR, `merge_hold` fail-closed, `pr_ship` re-checking CI on the current head). Phase 1 has no idempotence story: two implementer runs produce two branches. | Only for phases 3–6, where it is already approximated. |

### 1.3 What AIDA deliberately does not want

- **Replaying the implementer.** Temporal's replay assumes that re-running the
  workflow function against the same history is a no-op. Re-running an
  implementer session against the same spec is *not* a no-op: it is a second,
  different attempt. AIDA therefore checkpoints *outcomes* (branch, head sha,
  PR number, verdict on that sha) rather than *decisions*, and on resume it asks
  the forge and git what is true instead of asking a history what was decided.
  This is the corpus-integrity stance of `PRIN-6` (present evidence is not
  necessarily current evidence) applied to the engine itself.
- **Unbounded automatic retries.** A durable-execution engine will retry an
  activity for days if the policy says so. An LLM worker retried for days burns
  tokens producing different wrong answers. The retry budget is small (default
  1, cap 3), the retried causes are an allow-list of *mechanical* failures, and
  a reviewer's `request-changes` or a red CI on the agent's own code is never
  retried by the machine: it is shelved for a human or the advisor tier.
- **A general-purpose workflow DSL.** ADR-7 fixes *one* per-spec engine with a
  fixed phase order; ADR-9/ADR-10 make that a CI gate
  ([one-engine-invariant.md](one-engine-invariant.md)). The value of the
  control plane is that every spec goes through the same, inspectable gates;
  a second engine or a user-defined graph of activities would reintroduce the
  "which loop is driving this?" confusion the invariant exists to remove.
- **Hiding the history from the operator.** Temporal's history is for the
  engine; AIDA's is for the human first (`aida drain status`, `aida ps`,
  `aida findings list`, `.aida/last-drain.json`). Every checkpoint is plain
  JSON/JSONL under `.aida/` precisely so a person can read it when the machine
  is wrong.

---

## 2. Merge queues

The unit of comparison is one PR's admission to the default branch. Bors,
Zuul, Mergify, Aviator and the GitHub merge queue all answer the same
question — *may this change land on the branch as it is now?* — and enforce
the answer with a gate the contributor cannot bypass.

### 2.1 What AIDA already has

| Merge-queue property | AIDA equivalent | Where it lives |
|---|---|---|
| **A queue of merge candidates with positions** | Bare `aida integrate` lists Done-with-PR work as a merge queue with positions; `aida integrate --run` drains it through one serialised rebase/review/merge/pull loop so finished PRs do not stale-base race each other (STORY-520 producer/consumer split). | `integrate.rs` (pure readiness decision), `integrate_view.rs`, `integrate_checkout.rs` |
| **Test on the branch it will actually land on** | The integrator rebases each candidate onto current `main` before its checks are consulted (`aida pr rebase`, `pr_rebase.rs`), then waits for CI on the rebased head. Batched integration (`/aida-integrate`, the `batch N` commits on `main`) lands several rebased branches in one PR. | `pr_rebase.rs`, `pr_ship.rs`, the integrate skill |
| **Serialised merges, one at a time** | `merge_lock.rs` plus the single integrator loop; `drain_lock.rs` forbids a second integrating drain on the same repo. | `merge_lock.rs`, `drain_lock.rs` |
| **Required checks must be green on the current head** | Checks collapse to one pass/fail with a *required set* read from branch protection (`gh pr checks --json`); informational checks are re-classified only when not required *and* on the `[ci]` allow-list. A required red is always a real failure. | `docs/autonomous-drain.md` → *Which red checks gate a merge*; `pr_ship.rs` |
| **A second, independent admission predicate beyond CI** | Merge-readiness is the conjunction of two independently-falsifiable predicates — required checks pass *and* a review verdict names the current head — and surfaces report them separately (`PRIN-7`). Verdicts are first-class JSON per spec, so a stale approval on an older sha does not admit a newer one. | `review_verdict.rs`; `aida queue done` gate |
| **Holds that a contributor cannot bypass** | `.aida/merge-holds/PR-<n>` (ADR-37 layer 1): every AIDA merge path funnels through `forge::merge_change`, which refuses fail-closed while the marker exists; layer 2 is a GitHub required status check on the equivalent label, catching even a raw `gh pr merge`. Cleared only by `aida merge-hold clear <n>`. | `merge_hold.rs`; the `merge-hold-gate` workflow |
| **Dependency-aware ordering** | `BlockedBy` pickability: a candidate whose blocker is not Completed is skipped, not merged out of order (STORY-333). Typed relationships in the store, not a queue-local annotation, define the dependency. | `burndown.rs` (`classify` → `Pickability`) |
| **Refuse to assert green when the evidence is absent** | `PRIN-5`: a surface must distinguish absent evidence from good evidence. `aida pr ship` aborts on unknown check state rather than treating "no rows" as pass; pure-git forges keep the coarse verdict rather than inventing one. | `pr_ship.rs`, forge adapters |
| **Post-merge verification** | The `pull` and `build` phases (5 and 6) confirm `main` builds after the merge, and the trailing `(SPEC-ID)` on the merge commit drives spec completion, so a merge that lands but does not build is caught by the same loop. | `auto_complete.rs` phases 5–6; the commit-trailer convention |

### 2.2 What AIDA lacks

| Merge-queue property | Status in AIDA | Would it help? |
|---|---|---|
| **Speculative / optimistic batching** (Zuul, GitHub merge queue: test N candidates stacked, bisect on failure) | Absent. `/aida-integrate` batches *after* per-branch review and rebase, testing the combined result once; a red batch is unpicked by hand. | Yes, once integration throughput is the bottleneck. Today reviewer capacity and build capacity are, so the single serial loop is the right size. |
| **Server-side enforcement of the whole gate** | Partial. The merge-hold has a server half (required status check). The verdict-names-current-head predicate is enforced client-side by every AIDA merge path but not by a branch-protection rule, so a raw `gh pr merge` without a hold can still land un-verdicted code. | Yes: a `verdict-gate` required check reading `.aida/review-verdicts/<SPEC>.json` (or a label mirror) would make `PRIN-7` server-enforced the way ADR-37 layer 2 made the hold. |
| **Priority lanes and queue jumping** | Partial. Priority orders the drain's ready set and `aida integrate` positions, but there is no "hotfix lane" that pre-empts a running integration. | Low. Hotfixes are rare enough to `aida pr ship` by hand. |
| **Automatic rebase-and-retest on every new `main`** | Partial. The integrator rebases when it reaches a candidate; nothing re-tests a waiting PR each time `main` moves. `aida pr rebase` is on demand. | Marginal; the serial loop rebases immediately before merge, which is the only point at which staleness matters. |
| **Flake handling by re-run with a budget** | Partial. The transient-retry budget covers phase failures, and `[ci] informational_*` covers known-noisy workflows; there is no per-check "re-run this job up to N times" policy. | Modest. Worth adding as a `[ci]` policy if a flaky required check appears; not worth a general mechanism first. |

### 2.3 What AIDA deliberately does not want

- **Merging on green alone.** Every merge queue in the list admits on checks.
  AIDA refuses to: because the change was written by an LLM, "the tests pass"
  is necessary but not sufficient, and the second predicate (a verdict on this
  head, from a seat that did not write the code) is the corpus-integrity rule
  `PRIN-7`. A queue that could be satisfied by CI alone would let an agent
  approve its own work by making the tests pass.
- **Queue state that lives only in the forge.** GitHub's merge queue state is
  invisible once the PR is merged or removed. AIDA's admission state is in the
  repo (`.aida/review-verdicts/`, `.aida/merge-holds/`, `.aida/events.jsonl`)
  and the spec graph, so the *reason* a change was admitted survives the PR and
  can be read back by `aida why` and `aida history events`. This is the
  survival half of `VIS-2` applied to the gate.
- **A queue that reports a state it has not verified.** The characteristic
  defect of both admission and reconciliation is a surface asserting an untrue
  state — false-green, false-empty, stale-as-current (`TERM-5`). Bors's
  "approved" or GitHub's "queued" is a reassurance; AIDA's surfaces are
  required by `PRIN-4`–`PRIN-7` to say *which* predicate holds, on *which*
  head, and whether the evidence is absent rather than negative. A merge queue
  that collapsed those into one badge would be a regression, not a feature.

---

## 3. Gap P1: resumable orchestrator checkpointing

The [round-2 moat document](../archive/competitive-analysis/2026-05-31-round2-moat-gaps-moves.md)
(2026-05-31) named **P1 — Resumable orchestrator checkpointing** as the first
gap a technical evaluator hits: *"A crashed drain isn't step-resumable today;
formalize phase transitions into a replayable execution log keyed to
spec+worktree. Edge: git-canonical AND crash-resumable."*

**Where it stands.** The step-resumable half is closed. STORY-491/492 shipped
`aida queue work --resume-drain`: the drain-state file is the checkpoint, the
pure decision in `drain_resume.rs` decides whether a crashed drain is *safely*
resumable and at which phase, and the entry point re-enters the phase loop
there. `.aida/events.jsonl` is the execution log the gap asked for, keyed by
spec, phase and attempt. What did not ship, and should not, is the *replayable*
half in the Temporal sense.

**Would a durable-execution property close it?** Only the properties AIDA
already borrowed do:

- *Checkpoint per step* — adopted (`drain_state.rs`). This is what makes resume
  possible.
- *Append-only history* — adopted (`events.rs`). This is what makes the resume
  auditable and lets a watcher wait on a predicate instead of polling.
- *Deterministic replay* — declined, for the reason in §1.3: the only phase
  worth replaying is the one that cannot be, because its worker is an LLM.
  Replay would rebuild the engine's in-memory state, but the engine's state is
  already recoverable from the forge and git, which are the ground truth the
  corpus-integrity layer is required to consult anyway (`PRIN-6`).
- *Workflow versioning* — declined for now; the fail-closed merge-hold covers
  the stale-binary case that versioning would otherwise address.

**What remains of P1** is therefore narrower than the moat document phrased
it: the resume is keyed to the drain-state file, not to "spec + worktree" in
the store. A drain whose `.aida/drain-state.json` is lost (a deleted checkout,
a second machine) cannot resume, though `aida queue recover <id>` can still
reconstruct the recommended next action from git and forge reality. Moving the
checkpoint into the orphan store would make the resume git-canonical as the
gap wanted; it is a real design choice with a sync-conflict cost, and it needs
a sketch on its own spec before implementation.

---

## Related

- `TERM-5` — control plane, amended as the corpus-integrity layer (`aida show TERM-5`).
- `VIS-2` — the vision this layer serves (`aida show VIS-2`).
- `PRIN-4` … `PRIN-7` — the integrity rules the control plane enforces.
- [one-orchestration-engine.md](one-orchestration-engine.md) and
  [one-engine-invariant.md](one-engine-invariant.md) — the single per-spec
  engine and its CI gate.
- [autonomy-and-escalation.md](autonomy-and-escalation.md) — the escalation
  tiers above the engine.
- [`docs/autonomous-drain.md`](../autonomous-drain.md) — the operator's guide
  to retries, shelving, CI gating and recovery.
- [2026-09-24 positioning deep-dive](../positioning/2026-09-24-spike-86-positioning-from-engineering.md)
  §3 and §6 — the layer model and the finding that this comparison was missing.
