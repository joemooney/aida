# Infinity lessons for AIDA's existing Codex, Claude, and AGY support

Status: documentation draft; proposed improvements, not implementation approval.
Date: 2026-11-07. Tracking: TASK-3387.
<!-- trace:TASK-3387 | ai:codex -->

This document asks: **What can AIDA improve while Codex, Claude Code, and
Antigravity (AGY) remain the agents doing the work?** It replaces the mixed
roadmap in the original Infinity deep dive. The separate
[AIDA-owned agent exploration](2026-11-07-aida-owned-agent-runtime-exploration.md)
asks whether AIDA should also own a model loop. Neither track depends on
approval or completion of the other.

The original research inspected AIDA `02790568d5e` and Infinity
`e1a3e4b36e5`. This split also checks AIDA `122fea66d30`. Infinity source paths
below are relative to the separate `/home/joe/ai/infinity` checkout.

## 1. Ownership boundary

AIDA owns requirements, queue routing, worktree and lease coordination,
authorized launch paths, mail delivery, human questions, watches, review
coordination, and completion gates. Each vendor harness owns its model calls,
conversation representation, tool scheduling, compaction, and internal subagents.

AIDA can improve the inputs it supplies and the lifecycle around a harness. It
cannot promise an exact tool-boundary wake, rewrite a harness transcript, or
hibernate a harness's pending tool execution unless that harness exposes and
supports the necessary operation.

| Improvement | Benefits existing harnesses? | Limit |
|---|---|---|
| Durable wake delivery and recovery | Yes | Queue acceptance, observed delivery, and acknowledgement remain separate facts. |
| Filter/coalesce watch events before waking agents | Yes | Preserve important transitions and evidence of failed observations. |
| Resume a parked work item when a human answers | Yes | Process exit (or `claude stop` for `--bg` sessions) plus `--resume` (`claude --resume`, `codex resume`) can release vendor process memory across human waits; not an in-flight tool continuation. |
| Turn-boundary session fork for helper/triage tasks | Yes (candidate to test) | `claude --resume <id> --fork-session`, `codex fork`, and AGY `invoke_subagent` exist, but whether a hook/idle boundary guarantees a clean, flushed transcript without concurrent input or child work requires lifecycle testing. |
| Pre-compaction handoff and threshold tuning | Yes (where vendor exposes hooks/flags) | `claude --autocompact <tokens>` and `PreCompact` hooks can trigger AIDA handoff snapshots before vendor compaction runs; AIDA still does not own the internal summary algorithm. |
| Bounded AIDA tool responses with artifact references | Yes | Applies to AIDA-owned outputs, not every vendor tool result. |
| Explicit child task packets and reviewer context separation | Yes | Harness-native context inheritance remains vendor-owned. |
| Rewrite tool-call/result pairs in conversation history | No general adapter capability | Requires harness support or AIDA's own runtime. |
| Exit while retaining an exact pending model/tool continuation | No general adapter capability | Ordinary session resume restarts at a turn boundary, not mid-tool-call. |

## 2. Current AIDA adapter descriptors vs. available vendor CLI surfaces

This section distinguishes **what AIDA's checked-in [`VendorProfile`](../../aida-core/src/vendors/mod.rs) table currently wires** from **what the underlying vendor CLIs/runtimes expose for investigation**. Descriptors in [`aida-core/src/vendors/mod.rs`](../../aida-core/src/vendors/mod.rs) describe admitted AIDA adapters today; they do not grant authority or containment credit, nor do they represent the ceiling of what each vendor binary supports.

| Vendor | Wired in `VendorProfile` today (`aida-core/src/vendors/mod.rs`) | Verified vendor CLI / runtime capabilities available to test | Consequence for this track |
|---|---|---|---|
| Codex | `MailWake::ExactThreadQueue` (`codex queue`, ` >= 0.164.0`), `Resume::RecordedThread`, `Background::HostDetached`, `.codex/hooks.json` | `codex queue`, `codex resume`, `codex fork` (session fork), `PreToolUse` / `PostToolUse` / `SessionStart` / `UserPromptSubmit` / `SubagentStart` / `SubagentStop` / `Stop` hooks | Extend [`mailbox_wake_run.rs`](../../aida-cli-lib/src/mailbox_wake_run.rs) with turn-state awareness; test `codex fork` flush behavior at turn boundaries and `codex resume` after parked waits. |
| Claude | `MailWake::PerTurnHooks`, `Resume::RecordedSession`, `Background::DaemonHandoff`, `.claude/settings.json` | `--resume <id> --fork-session`, `--autocompact <auto\|tokens>`, background-session `stop\|kill <id>` / `respawn <id>` (`--bg`), `PreCompact` hook plus existing tool/session/stop hooks | Wire `PreCompact` / `--autocompact` to capture AIDA handoff state before compaction; test `--bg` `stop` + `--resume` (or foreground exit + `--resume`) across human waits; test `--fork-session` flush behavior at turn boundaries. |
| AGY | `MailWake::Unsupported`, `Resume::Unsupported`, `Background::Unsupported`, `.agents/hooks.json` | Native in-session `invoke_subagent` (`self`, `research`, `define_subagent`), `send_message`, `schedule` (one-shot timers with `TimerCondition` and cron wakeups), and `.agents/hooks.json` | Keep host-side mail durable for the next hook/turn boundary, while evaluating AGY's native `invoke_subagent` and `schedule` primitives inside AGY skills/workflows where admitted. |

See [Codex mail wake](../advanced/codex-mail-wake.md) and
[session communication](../advanced/session-communication.md). Any newly wired
vendor flag or hook surface still requires its own deterministic adapter tests
and launch-policy admission.

## 3. Baseline in AIDA today and improvements worth evaluating

### 3.1 Reliable event delivery, turn-state deferral, and explicit interruption

**What AIDA already implements:**
* [`aida-cli-lib/src/mailbox_wake_run.rs`](../../aida-cli-lib/src/mailbox_wake_run.rs) (`Ledger`, `Receipt`, `Scan`) already maintains per-thread durable notification ledgers with `accepted`, `pending`, exponential `backoff`, a 24 KB `NOTICE_LIMIT`, and `inflight: Option<Receipt>` (which pauses wakes on a timed-out or uncertain `codex queue` submission until explicit `--recover <thread> --accepted|--retry`).
* [`aida-cli-lib/src/mail_wake.rs`](../../aida-cli-lib/src/mail_wake.rs) (`WakeAdapter`, `launch_wake`, `MIN_CODEX_QUEUE`) already gates Codex wake on version `>= 0.164.0` and arms the row only after `SessionStart` records the exact thread ID.

**What Infinity's design adds (`system/defer.rs`, `system/thread.rs::filter_deferrable`):**
* Infinity distinguishes **deferrable synthetic events** (`SubscriptionEvent`, `ThreadReport`, `ParentMessage`) from **human interruptions** (`UserContent::Text`) while work is active, and formats synthetic events with explicit provenance (`receive_event__injected`) so the model does not mistake a background event for a user command.
* **Remaining delta for AIDA:**
  1. Track turn/park state from existing `PreToolUse` / `PostToolUse` / `Stop` hooks (or explicit `aida questions park` state) so [`mailbox_wake_run.rs`](../../aida-cli-lib/src/mailbox_wake_run.rs) can hold non-urgent peer/watch notices until a turn boundary or coalesce them while a session is busy, while reserving immediate injection for explicit human interruptions.
  2. Format injected notices in [`mailbox_wake_run.rs`](../../aida-cli-lib/src/mailbox_wake_run.rs) and [`review_watch_notice.rs`](../../aida-cli-lib/src/review_watch_notice.rs) with structured provenance metadata (`source`, `event_id`, `head_sha`, `trust_class`, `action_expected`) so untrusted forge text (PR comments, CI failure excerpts) is clearly separated from AIDA coordinator instructions.

### 3.2 Filter and coalesce before waking a model

**What AIDA already implements:**
* [`aida-cli-lib/src/review_watch_notice.rs`](../../aida-cli-lib/src/review_watch_notice.rs) (`send`, `findings_due`) already prunes findings from superseded heads (`entry.findings.retain(|k, _| k.starts_with(&prefix))`), deduplicates blocking verdicts by `(head, sha256_digest)`, and persists `Notice { message, sent }` receipts before delivery.
* [`aida-cli-lib/src/review_watch.rs`](../../aida-cli-lib/src/review_watch.rs) already supersedes running reviewer processes on head change (`SUPERSEDE_GRACE`) behind `pidfd` identity checks ([`review_watch_signal.rs`](../../aida-cli-lib/src/review_watch_signal.rs)) and throttles readiness probes (`READY_INTERVAL = 120s`) through [`review_watch_ready.rs`](../../aida-cli-lib/src/review_watch_ready.rs) (`trait Evidence`).
* [`aida-cli-lib/src/mandate_drive.rs`](../../aida-cli-lib/src/mandate_drive.rs) (`skip_once`) already deduplicates repeated `(key, kind, reason)` skip/refusal observations across loop ticks.

**What Infinity's design adds and remaining delta for AIDA:**
* Infinity's subscription-event children (`is_for_subscription_event: true` in `event_processor.rs`) protect the parent conversation from raw event noise, but pay an LLM completion per event.
* In AIDA, **deterministic host-side filtering** across [`review_watch_ready.rs`](../../aida-cli-lib/src/review_watch_ready.rs) and [`mailbox_wake_run.rs`](../../aida-cli-lib/src/mailbox_wake_run.rs) should remain primary: collapse equivalent CI/forge observations, preserve `Unknown`/transient refusals distinctly from clean passes, and coalesce rapid head updates before queuing a wake.
* Where an event is genuinely ambiguous (e.g., classifying unstructured log failure output), an optional bounded read-only triage helper can use a fresh session or a turn-boundary fork (`claude --resume <id> --fork-session` / `codex fork`), subject to token budget, authority limits, and measurement against deterministic rules.

### 3.3 Human questions as durable workflow waits (stop-on-ask/park, resume-on-answer)

**What AIDA already implements:**
* [`aida-cli-lib/src/questions.rs`](../../aida-cli-lib/src/questions.rs) provides [`questions_ask`](../../aida-cli-lib/src/questions.rs) (line 1873) to record a `DecisionRequest` on a spec, and [`questions_answer_one`](../../aida-cli-lib/src/questions.rs) (line 1951) → [`finalize_answer`](../../aida-cli-lib/src/questions.rs) (line 866) to apply the chosen resolution token via [`apply_resolution_token`](../../aida-cli-lib/src/questions.rs) (line 506, including `"defer" | "park" | "hold"` triggers from `STORY-2402`), bind design decisions into `## Acceptance`, and auto-queue newly `Pickability::Ready` specs via `maybe_autoqueue` (line 956).

**What Infinity's design adds and remaining delta for AIDA:**
* Infinity's `PendingUserChoice` (`rap-protocol/src/lib.rs`, `system/thread.rs`) binds a blocking question to a specific thread ID in `StateStore` so the answer routes directly back to that thread.
* In AIDA today, [`finalize_answer`](../../aida-cli-lib/src/questions.rs) auto-queues a spec into the general burndown ready set (`maybe_autoqueue`), **but does not record or wake/resume a specific paused session or worktree** that posed the question via [`questions_ask`](../../aida-cli-lib/src/questions.rs):
  1. When an active session poses a blocking question via [`questions_ask`](../../aida-cli-lib/src/questions.rs) (or parks via [`apply_resolution_token`](../../aida-cli-lib/src/questions.rs)), optionally bind a compact resumption packet (spec ID, worktree branch/HEAD, pending verification step, and recorded vendor `session_id` / `thread_id` from the agent registry).
  2. Allow the blocked worker to cleanly exit (or `claude stop <id>` for a `--bg` session), releasing vendor process memory while waiting on the human.
  3. When [`questions_answer_one`](../../aida-cli-lib/src/questions.rs) → [`finalize_answer`](../../aida-cli-lib/src/questions.rs) runs, check current grant/worktree authority and either wake the live thread (`codex queue`) or resume the recorded session (`claude --resume <id>` / `codex resume <thread_id>`) with the answer packet. Answering a question supplies data only; it never bypasses an approval gate or revives an expired grant.

### 3.4 Bounded context packets, pre-compaction handoffs, and turn-boundary forks

* **Proactive output bounds:** Infinity's `recover_from_overflow` (`crates/infinity-agent-core/src/event_processor.rs:1444-1454`) only recovers when a single unvalidated tool result overflows (`history.unvalidated_len() == 1`). AIDA should proactively bound AIDA-owned CLI/tool outputs (launch context, mail excerpts, search/query results) and write oversized diagnostics to artifact files with path + byte-range references.
* **Pre-compaction handoff capture:** Unlike Infinity's background compaction child (`apply_compaction`), vendor compaction in Claude/Codex/AGY is opaque to AIDA once it runs. However, AIDA can wire Claude's `PreCompact` hook (and `--autocompact` threshold) plus turn-count/grant-expiry hooks (`aida-grant-expiry-notice.sh`) to automatically snapshot durable state (active spec, decisions, git diff summary, open questions) into AIDA's store *before* vendor compaction summarizes the transcript.
* **Turn-boundary context forks (testing CLI forks against the `safe_spawn_point` invariant):** Infinity's `HistoryManager::safe_spawn_point` (`crates/infinity-agent-core/src/event_processor.rs:1845`) cuts inherited history immediately before an unanswered tool call so the child never inherits a dangling `tool_use`. Because `claude --resume <id> --fork-session` and `codex fork <id>` operate on vendor-persisted session transcripts, AIDA should test whether invoking them from a `Stop` hook or idle boundary reliably observes a fully flushed transcript with no concurrent user input or in-flight subagent work before relying on CLI forks for helper/triage tasks.

### 3.5 Deterministic adapter tests

Borrow Infinity's deterministic testing discipline (`infinity-provider-protocol/src/mock.rs`, virtual TUI grid snapshots in `infinity-agent-cli/tests/tui_flow_snapshots.rs`) at AIDA's adapter seams:
* Extend AIDA's existing PATH-isolated fake-harness tests (`test_env::run_path_isolated_test`) and injectable evidence traits (such as [`review_watch_ready::Evidence`](../../aida-cli-lib/src/review_watch_ready.rs)) to cover turn-deferred wake coalescing, `inflight` uncertain-delivery recovery across restarts, `questions_ask` → `finalize_answer` session-resume flows, and `PreCompact` handoff triggers without live model calls.

## 4. Suggested order and success measures

| Order | Bounded investigation/change | Target AIDA seam | Evidence of benefit |
|---|---|---|---|
| 1 | Turn-state deferral & structured provenance framing on wakes | [`mailbox_wake_run.rs`](../../aida-cli-lib/src/mailbox_wake_run.rs), [`review_watch_notice.rs`](../../aida-cli-lib/src/review_watch_notice.rs) | Non-urgent peer/watch notices wait for turn/park boundaries; untrusted forge text is tagged with provenance metadata. |
| 2 | Stop-on-ask/park and resume-on-answer around human questions | [`questions.rs`](../../aida-cli-lib/src/questions.rs) (`questions_ask`, `questions_answer_one`, `finalize_answer`, `apply_resolution_token`), [`mail_wake.rs`](../../aida-cli-lib/src/mail_wake.rs) | Blocked sessions can exit (or `claude stop` if `--bg`) to free RAM and resume (`claude --resume` / `codex resume` / `codex queue`) when answered, gated on live grant validity. |
| 3 | Pre-compaction handoff capture and `--autocompact` wiring | [`.claude/settings.json`](../../.claude/settings.json), [`aida-core/src/vendors/mod.rs`](../../aida-core/src/vendors/mod.rs) | Durable handoff state is snapshotted before vendor compaction summarizes the session. |
| 4 | Turn-boundary session forks (`--fork-session` / `codex fork`) vs. deterministic watch filtering | [`review_watch_ready.rs`](../../aida-cli-lib/src/review_watch_ready.rs), [`aida-core/src/vendors/mod.rs`](../../aida-core/src/vendors/mod.rs) | Deterministic filtering handles routine transitions; turn-boundary CLI forks are lifecycle-tested for transcript flush safety and measured on cost/latency before broader use. |
| 5 | Proactive output bounds with artifact pointers & deterministic adapter tests | `aida-cli-lib` query/context formatters, `test_env::run_path_isolated_test` | Large AIDA outputs spill to artifact files with byte-range pointers; restart/coalescing paths are verified without live models. |

Each item builds on AIDA's existing ledgers and evidence traits rather than
duplicating them. None requires a new model provider, custom conversation
engine, RAP adoption, or replacement coding agent.

## 5. Existing AIDA responsibilities remain authoritative

Reuse AIDA's worktree lifecycle, current role/grant rules, review separation,
store writers, forge observation boundary, and admitted containment paths.
Infinity's local sandbox wrapper is not a substitute for those controls.
Distinguish native vendor sandbox posture from AIDA containment, and retain
surface-specific conformance limits. See
[containment evidence](../internal/testing/containment-conformance.md).

The deliverable of this track is better coordination of Codex, Claude, and AGY.
Deciding against an AIDA-owned agent leaves every benefit in this document
available on its own merits.
