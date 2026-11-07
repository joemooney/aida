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
and debugging a new execution engine. In Infinity (`e1a3e4b36e5`),
`crates/infinity-agent-core/src` totals 12,930 lines—of which 6,620 lines are
production runtime code (including 2,000 production lines in
`event_processor.rs:1-2000`) and 6,310 lines are inline `#[cfg(test)]` suites
(including 3,208 test lines in `event_processor.rs:2001-5209` and 2,652 lines in
`system/tests.rs` + `test_support.rs`)—before counting `infinity-daemon`
(7,490 lines), provider adapters, RAP/MCP bridges, CLI surfaces, and sandboxing.

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

### Hibernation depends on the embedding, execution mode, and a passive-tool check

Infinity exposes two distinct execution modes in `crates/infinity-agent-core/src/system/builder.rs`:
1. **Step mode (`AgentSystem::step`, `builder.rs:452-492`):** Loads `Thread::load(...)` from the stores, runs one `step_no_defer`, and drops the per-thread `Thread` handle (and its in-memory `HistoryManager`) when the step finishes. Note that dropping `Thread` is still distinct from exiting the host worker process: `SystemInner` (shared stores, model/config sources, input channels) and any spawned background tasks remain in the embedding process unless the worker process itself exits.
2. **Resident local driver (`LocalAgentSystem` / `drive_thread`, `system/local/driver.rs:235-282`):** Calls `check_before_idle` (`driver.rs:331-364`), which returns `IdleDecision::AwaitInput` whenever `thread.awaiting_tool_result()` is true, parking the driver task and its `Thread` in memory rather than exiting.

Moreover, inspecting `Thread::awaiting_tool_result` (`system/thread.rs:235-238`) alongside `Thread::pending_active_tool_call` (`system/thread.rs:216-226`) reveals an important discrepancy in Infinity's local driver:
* `pending_active_tool_call()` checks `t.is_passive()`, so passive sleep tools (`sleep_until_event_or_input`, `sleep`, and `sleep_until` in `tools/sleep.rs`, which return `is_passive() -> true`) do not block event deferral.
* `awaiting_tool_result()` only checks `name != "close_thread"` and **does not check `!is_passive()`**:
  * For `sleep_until_event_or_input` (`tools/sleep.rs:11-49`), which is recorded as a `ToolCall` at the tail of history and whose `execute()` is a no-op that never schedules a `ToolResult`, `awaiting_tool_result()` stays `true` indefinitely and **the resident local driver never exits on its own (`IdleDecision::AwaitInput`)** until external input arrives.
  * For timed `sleep` and `sleep_until` (`tools/sleep.rs:54-228`), `execute()` spawns an in-process `tokio::spawn` timer task (`sleep.rs:98`, `sleep.rs:204`) and `awaiting_tool_result()` remains `true` **for the entire sleep duration**, keeping the driver resident until the timer delivers its `ToolResult` and the subsequent step finishes without a tool call.

For AIDA, releasing worker memory across long tool, timer, or human waits requires exiting the worker process (not just parking a local driver or dropping a step handle in a long-lived process) and persisting timer/callback routing across restarts.

### Commit-before-dispatch trade-offs and crash boundaries

In `Thread::step_no_defer` (`system/thread.rs:564-587`), `self.history.sync().await?` commits the assistant `ToolCall` before `event_processor::execute_action_with_error_result` dispatches the HTTP request.
* **Why commit-before-dispatch is required for async callbacks:** Because `HistoryManager::handle_content` matches an incoming `ToolResult` against `history.last()`, dispatching before committing would allow a fast callback to arrive before its `ToolCall` is in history, dropping the result as an orphan. If the process stays alive and the HTTP POST fails synchronously, `execute_action_with_error_result` injects a synthetic error `ToolResult` so the thread recovers on the next step.
* **Where the crash window lies:**
  1. If the process crashes after `self.history.sync().await?` (`thread.rs:566`) but before `execute_action_with_error_result` (`thread.rs:574`), the `ToolCall` is durably persisted at the history tail with no HTTP request ever sent. While plain user text can still interrupt it (`TOOL_CALL_INTERRUPTED_TEXT`), `pending_active_tool_call()` remains `Some(call_id)`, causing `filter_deferrable` to defer `SubscriptionEvent`, `ThreadReport`, and `ParentMessage` inputs indefinitely. Recording a durable pre-dispatch outbox receipt (analogous to `inflight: Option<Receipt>` in AIDA's [`mailbox_wake_run.rs`](../../aida-cli-lib/src/mailbox_wake_run.rs)) makes unfinished or timed-out dispatch **detectable and explicitly uncertain** rather than silently wedged; safe automatic retry still requires tool idempotency or explicit reconciliation.
  2. In `HistoryManager::sync` (`event_processor.rs:645-670`), `conversation_store.append_messages` and `let _ = self.state_store.add_processed_message_ids(...)` run as two separate calls with errors from the second call explicitly discarded (`let _ =`), allowing a crash or state-store failure to leave appended messages unrecorded in `processed_message_ids`.
  3. The local daemon's `crates/infinity-daemon/src/memory_store.rs` writes JSON snapshots and discards several write errors; its durability properties must not be generalized to all `ConversationStore`/`StateStore` implementations or imported as sufficient for AIDA.

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
