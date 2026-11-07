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
| Resume a parked work item when a human answers | Yes | Resume or relaunch according to vendor capability; not an exact pending tool continuation. |
| Bounded AIDA tool responses with artifact references | Yes | Applies to AIDA-owned outputs, not every vendor tool result. |
| Explicit child task packets and reviewer context separation | Yes | Harness-native context inheritance remains vendor-owned. |
| Rewrite tool-call/result pairs in conversation history | No general adapter capability | Requires harness support or AIDA's own runtime. |
| Fork background compaction and apply safe history cutoffs | No general adapter capability | Requires ownership of conversation history. |
| Exit while retaining an exact pending model/tool continuation | No general adapter capability | Ordinary session resume is a different guarantee. |

## 2. Current vendor capabilities

The checked-in descriptor table in
[`aida-core/src/vendors/mod.rs`](../../aida-core/src/vendors/mod.rs) distinguishes
these capabilities. Descriptors describe adapters; they do not grant authority,
containment credit, or admission to a particular launch surface.

| Vendor | Mail/wake surface | Resume surface | Consequence for this track |
|---|---|---|---|
| Codex | Exact-thread queue adapter, subject to configured support and version checks | Recorded thread | Improve delivery accounting and recovery around the adapter. A queue receipt alone does not prove model consumption. |
| Claude | Per-turn hooks | Recorded session | Supply pending context at supported hook/turn boundaries; do not claim arbitrary idle-session wake from a per-turn hook. |
| AGY | Live mail wake unsupported | Resume unsupported in the descriptor | Keep mail durable for the next supported interaction; do not advertise Codex-equivalent wake or resume. |

See [Codex mail wake](../advanced/codex-mail-wake.md) and
[session communication](../advanced/session-communication.md). Any future
adapter capability needs its own evidence and admission; this proposal does
not upgrade a vendor by analogy.

## 3. Improvements worth evaluating independently

### 3.1 Reliable event delivery and explicit interruption

Infinity's `system/defer.rs` and `system/thread.rs::filter_deferrable` distinguish
peer/subscription notifications from user interruptions while a tool is active.
Borrow that semantic distinction for AIDA's delivery contract.

For AIDA-owned events, retain stable event identity, destination identity,
source/provenance, attempted delivery, acceptance, uncertainty, and eventual
acknowledgement. Reconcile ambiguous delivery rather than automatically sending
again. Preserve queued events across restarts. Inspect existing mail-wake
ledgers before adding another queue or deduplication mechanism.

Where a vendor exposes a supported queued/non-interrupting delivery primitive,
use it and test its observable behavior. Where it does not, leave the event
pending for a supported boundary. Do not infer that an agent is idle from a
stale status field, and do not treat synthetic peer input as a human command.
Explicit human interruption remains a separate operation.

This is an improvement to AIDA's adapter guarantees, not a port of Infinity's
internal tool-call state machine into an opaque vendor session.

### 3.2 Filter and coalesce before waking a model

Infinity's subscription-event children protect the parent conversation from
raw event noise. AIDA can adopt the goal without implementing that runtime.

Start with deterministic filtering: scope events to the relevant work item,
PR/MR head, and subscription; collapse repeated equivalent observations; and
wake on actionable transitions. Preserve Unknown/transient observations and
refusals rather than silently interpreting them as success or irrelevant noise.
Keep UI-only progress out of model notifications when it carries no action.

A bounded read-only triage invocation using an existing supported harness is
an optional second step for ambiguous events. It needs a small explicit input
packet, authority limits, a budget, and retained source evidence. It consumes
model tokens and may add latency. Low confidence or triage failure must remain
visible for escalation. Measure total cost and missed actions, not just reduced
parent-context growth.

### 3.3 Human questions as durable workflow waits

AIDA already has question parking, human inboxes, and resolution conditions.
The opportunity is to associate a parked work item with an explicit wake target
and an answer event, then resume or relaunch through the vendor's supported
adapter after checking current authority and worktree state.

Do not store a vendor-internal tool-call ID on a question and claim exact
continuation without a supported vendor contract. Answering a question does
not automatically clear a separate approval gate or restore an expired grant.

### 3.4 Bounded context packets and AIDA-owned tool output

Infinity's overflow recovery motivates proactive bounds on what AIDA sends to
agents: compact launch context, relevant mail excerpts, bounded query results,
and artifact references for large diagnostics. Preserve full evidence outside
the prompt and distinguish truncation from an empty result.

This reduces overflow risk from AIDA-owned inputs. It cannot repair a vendor's
already-overflowed transcript or intercept all native tools. Likewise, fresh
reviewer sessions and self-contained task packets reduce inherited bias, but
AIDA cannot impose Infinity's exact ancestor cutoff semantics on vendor-native
subagents.

### 3.5 Deterministic adapter tests

Borrow Infinity's mock-provider and deterministic-ID approach at the boundary
AIDA actually owns: fake harness executables, recorded adapter responses,
injected clocks/IDs through existing project abstractions, and event traces.
Verify duplicate notifications, ambiguous acceptance, process restarts, stale
heads, recipient changes, and unsupported capabilities without live models.
Use focused live conformance checks only for claims mocks cannot establish,
such as a vendor's real delivery behavior.

## 4. Suggested order and success measures

| Order | Bounded investigation/change | Evidence of benefit |
|---|---|---|
| 1 | Audit existing wake receipts and vendor capability reporting | Each vendor reports supported, unsupported, pending, or uncertain delivery accurately; existing guarantees are preserved. |
| 2 | Add missing deterministic delivery/restart coverage | No acknowledged event is silently lost and no uncertain delivery is reported as confirmed consumption. |
| 3 | Improve deterministic watch filtering/coalescing where a measured gap exists | Fewer unnecessary model wakes with actionable transitions still surfaced. |
| 4 | Connect question resolution to supported resume/relaunch | The right work item receives the answer, and expired authority prevents execution. |
| 5 | Bound AIDA context/output and evaluate optional triage | Reduced prompt volume and total cost without lost evidence or missed actions. |

These are candidate investigations, not claims that the current code lacks
each mechanism. Baseline current behavior before opening implementation work.
None requires a new model provider, custom conversation engine, RAP adoption,
or replacement coding agent.

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
