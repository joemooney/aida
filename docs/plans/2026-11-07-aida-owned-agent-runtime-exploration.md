# Exploration: should AIDA own an agent runtime?

Status: research/design draft; no runtime implementation or adoption approved.
Date: 2026-11-07. Tracking: TASK-3387 (document split only).
<!-- trace:TASK-3387 | ai:codex -->

This document asks: **Is owning model execution valuable enough for AIDA to
maintain an optional agent runtime?** Improvements to today's Codex, Claude,
and AGY integrations are independently described in
[Infinity lessons for existing harness support](2026-11-07-infinity-agent-architecture-deep-dive.md).
That work does not require this runtime, and this is not its inevitable next phase.

Research basis: Infinity `e1a3e4b36e5`, initially compared with AIDA
`02790568d5e`; the document split also checks AIDA `122fea66d30`. Infinity paths
below are relative to `/home/joe/ai/infinity`. Observations describe that checkout,
not guarantees about future releases.

## 1. The separate investment decision

An AIDA-owned runtime would own model requests, conversation assembly, tool
scheduling, streaming/retry behavior, context budgets, compaction, persisted
execution state, and recovery. Direct provider APIs are a separate integration
from launching a vendor CLI. Authentication, billing/access, supported models,
and provider-specific capabilities require independent validation; existing CLI
credentials or subscriptions must not be assumed reusable.

The potential advantage is exact control of suspension, event admission,
context inheritance, and bounded execution. The ongoing cost includes provider
compatibility, coding-tool quality, credential handling, cancellation, migration,
and debugging a new execution engine. A short core loop does not bound that cost.

AIDA's existing positioning in
[OVERVIEW.md](../../OVERVIEW.md#what-this-means-for-aidas-roadmap) favors composing
with vendor runtimes. This exploration proposes a possible exception that needs
its own evidence and decision. Existing harness support should remain useful
whether this experiment succeeds, fails, or is never funded.

## 2. What Infinity contributes

Infinity separates runtime, tool protocol, deployment, and coding tools across
21 workspace crates. The reusable ideas are execution semantics rather than
its entire daemon or coding product.

| Idea | Source in Infinity | Value if AIDA owns execution |
|---|---|---|
| Step-oriented execution | `infinity-agent-core/src/system/thread.rs` | Load state, prepare inputs, complete, commit, dispatch, and hand control back to the embedding. |
| Three-stage history buffers | `infinity-agent-core/src/event_processor.rs` | Separate unvalidated inputs, pending items, and partially streamed assistant output. |
| Deferral around active tools | `infinity-agent-core/src/system/defer.rs`, `system/thread.rs` | Hold synthetic events until a matching result or deliberate interruption settles the active call. |
| Structured event association | `infinity-agent-core/src/event_processor.rs::find_tool_call` and `prepare_input` | Recover original call context after compaction and construct a well-formed model-facing event representation. |
| Safe context forks | `infinity-agent-core/src/traits.rs`, `tools/thread.rs` | Explicit inherited cutoffs, seeded child instructions, and fresh-context horizons. |
| Background compaction | `infinity-agent-core/src/event_processor.rs::apply_compaction` | Summarize a fixed prefix while preserving turns appended during summarization. |
| Human-choice callbacks | `rap-protocol/src/lib.rs`, `infinity-agent-core/src/system/thread.rs` | Represent a decision wait in persistent state and continue after an answer. |
| Deterministic runtime tests | `infinity-provider-protocol/src/mock.rs`, core snapshots, daemon IDs | Exercise branching, retries, interruption, and context behavior without a live provider. |

Additional reference points: `infinity-provider-protocol/src/remote.rs` provides
a Unix-socket provider transport; `rap-protocol`, `rap-client`, and the RAP/MCP
bridges separate dispatch from callbacks. `system/local/router.rs` drives
thread futures in a `FuturesUnordered` pool. `infinity-agent-cli/tests/tui_flow_snapshots.rs`
uses virtual terminal grids for UI snapshots. These are options to evaluate,
not reasons to adopt every component or protocol.

## 3. Qualifications the original synthesis needs

### Hibernation depends on the embedding and wait type

The step API lets an embedding release execution state between invocations.
However, the local driver's `check_before_idle` returns `AwaitInput` while
`thread.awaiting_tool_result()` is true. It retains that thread while waiting;
subscription state alone does not keep the driver resident. Therefore the
checked-in local runtime does not demonstrate zero resident memory for every
tool wait. External tool processes and the coordinator also have resource costs.

For AIDA, process exit on a long wait would be a proposed guarantee to test,
including who owns any surviving tool and how its callback survives restarts.

### Commit-before-dispatch leaves crash boundaries

`Thread::step_no_defer` commits history before dispatching the asynchronous
action. That prevents a result from preceding its recorded call, but does not
by itself recover a crash between commit and dispatch, or an ambiguous remote
acceptance. `HistoryManager::sync` appends messages and updates processed IDs
through separate calls; the latter error is discarded in this implementation.
Callback deduplication is not exactly-once execution of external effects.

The local daemon's `memory_store.rs` also writes JSON directly and discards
write errors at several persistence sites. Its durability properties must not
be generalized to all Infinity storage implementations, nor imported as
sufficient for AIDA. AIDA's Git-canonical requirement store does not by itself
supply an atomic conversation/inbox/outbox transaction either.

### Context protection is separate from raw evidence retention

Infinity delays persistence of unvalidated conversation inputs until model
output begins. Its automatic overflow placeholder recovery is limited to a
single replaceable unvalidated input; a mixed oversized batch or oversized
committed history needs other handling. Model acceptance is not evidence that
input is trusted or semantically valid.

AIDA should durably retain incoming raw events/artifacts before acknowledging
delivery, then construct bounded model-facing views. Keep partial assistant
streams separate until committed. Bound outputs proactively and retain
references to full evidence. Do not erase the only copy to make a prompt fit.

### Deferral and event formatting need their own guarantees

Infinity's `InMemoryDeferQueue` explicitly loses deferred events on process
failure. `NoDeferral` permits interruption semantics when no queue is supplied.
AIDA would need durable deferral for any claimed restart-safe continuation.

Synthetic tool-call/result pairs can convey provenance and satisfy a provider's
conversation structure. They are not authentication or prompt-injection
protection. Provider-specific message validation still belongs in adapters.
A model-supplied `child_of` guard helps detect role confusion; it cannot replace
host-issued child authority or independent reviewer admission.

## 4. Candidate architecture, subject to a separate approved sketch

Use a host coordinator for event intake, durable delivery/reconciliation, and
current authorization. A bounded worker assembles model context and runs a
slice. Route model requests through a narrowly authorized host provider broker;
provider credentials need not enter tool children or the contained worker.
A socket alone is insufficient: bind requests to a validated session and scope,
limit models/budgets, validate framing, and define cancellation and expiry.

Keep tool effects behind AIDA's authorized execution boundary. Reuse existing
worktree, lease, review, forge, environment-projection, and containment owners.
Do not substitute Infinity's local sandbox or infer admission from a synthetic
probe. New worker launch surfaces require their own integrated evidence.

Prefer exiting a worker on long CI, review, or human waits. Persist the wait
reason and routing identity, then obtain fresh admission on wake. Conversation
continuity does not preserve an expired grant. Bind forge waits to the relevant
head SHA; an old event cannot complete work on a new head.

Use a local transactional runtime store for events, execution transitions,
conversation records, and dispatch intents, with appropriate protected artifact
storage. Select provenance checkpoints for the canonical store; do not commit
every token there. Storage location, retention, migration, and cross-worktree
identity remain design decisions. Conversations must not become an alternative
authority for requirement status, review approval, or completion.

## 5. Minimum durable execution contract

Before real effectful tools, define and test these boundaries:

1. Persist accepted input and its stable source identity before acknowledging it.
2. Serialize or fence execution per logical thread so stale workers cannot commit.
3. Commit state transitions, consumed-input bookkeeping, and dispatch intents
   atomically, or provide an equivalent proven recovery protocol.
4. Dispatch with a stable operation ID; record acceptance, rejection, or unknown
   outcome separately from eventual tool completion.
5. Reconcile unknown outcomes before retrying effects. Tools without an
   idempotency or reconciliation contract must not be blindly replayed.
6. Persist authenticated, correlated callbacks and durable deferred events;
   reject stale, wrong-session, or wrong-generation results.
7. Recheck current authority, cancellation, worktree ownership, and head binding
   before resuming or dispatching further work.

Question answers and peer notifications supply data. They do not confer
publication rights, clear merge holds, or authorize a reviewer to approve its
own work. Compaction summaries and inherited context likewise carry no authority.

## 6. A bounded prototype and a decision gate

The first prototype should exercise one workflow: receive a read-only task,
run one read-only tool, park a human question, exit the worker, and resume after
an answer. Start with a mock provider. Add one real provider only after the
state/recovery contract is testable and its access terms are validated.

Defer general shell/file editing, publication tools, inherited subagents,
background compaction, and model-based event triage. Each adds a distinct failure
surface and is unnecessary to establish whether durable slices are useful.

Acceptance evidence should include process death before and after every
persistence/dispatch boundary; duplicate and late callbacks; restart during a
human wait; an expired grant at wake; cancellation; oversized input; and competing
workers. Demonstrate retained evidence and an explicit uncertain state wherever
an external outcome cannot be determined.

Compare the same bounded workflow with an existing harness path. Measure
resident resources while waiting, total model/tool cost, time to resume, human
recovery steps, and successful authorized completion. Mock tests prove execution
semantics; they do not establish competitive coding quality.

Proceed toward an optional AIDA agent only if the measured benefit warrants
maintaining the runtime and its provider/tool integrations. A decision to stop
this experiment does not block the independent existing-harness improvements.
