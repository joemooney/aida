# Request dispositions and unresolved stall escalation

Status: proposed; master sign-off required before implementation.
Owner: TASK-3074. <!-- trace:TASK-3074 | ai:codex -->

## Approval boundary

STORY-2476's 2026-11-05 01:55 UTC advisor ruling approved slice 1 and
explicitly deferred acted-on tracking to this child. The parent sketch says
master must approve the record/CLI contract, legacy behavior, dispositions
that pause escalation, and escalation defaults. No approval of those choices
is recorded on TASK-3074. This proposal requests that approval under
AGENTS.md's Sketch-First Protocol; it does not introduce runtime behavior.

## Proposed command and meaning

Add `aida mailbox disposition <message-id> <acted|declined|deferred>
--reason <text> [--until <RFC3339>]` plus a read-only disposition projection
in message inspection and JSON. Require a complete, unambiguous message ID
resolution and a nonempty, bounded reason. Only `kind=request` qualifies.
`acted` requires an explicit evidence/reference string supplied by the caller;
it records the recipient's assertion, never verified completion of a spec,
review, merge, or other protected operation. `declined` is an explicit refusal
with reason. `deferred` requires a future absolute deadline no more than 24
hours away. `--until` on other outcomes refuses.

Reading, post-flush read ACK, wake acceptance, a reply, process exit, and
archive status never create an action disposition. A disposition does not
advance the read ledger. Legacy mail without a disposition remains unknown
as to action, including mail with a legacy read watermark.

## Identity, persistence and concurrency

Reuse the logical mailbox snapshot and resolved audience. A writer must be
the current authorized recipient, using the existing inbox authority resolver;
sender status, an arbitrary `--agent`, role-name text and operator inspection
alone grant no receipt authority. Record the concrete actor/session and matched
recipient keys. For shared role mail, one currently authorized reader may
resolve the request; this is a proposed policy requiring explicit sign-off.

Use a separate versioned action ledger under the verified common-Git
mail-lifecycle anchor, alongside the read receipts. Do not extend the read
receipt schema or rewrite message YAML. Bind each record to the message ID,
immutable message fingerprint, actor, audience keys, outcome, server timestamp,
reason, optional evidence and deferred deadline. Retain revisions for audit;
an authorized later disposition supersedes an earlier one. Bound entry and
file sizes and refuse exhaustion instead of pruning unresolved history.

Reuse lifecycle directory/file safety and durable atomic-write primitives.
Acquire lifecycle then action-ledger locks with bounded waits; revalidate
authority, message replicas, source generations and fingerprint under the
locks before mutation. Partial snapshots, conflicting replicas, unsafe anchors,
retiring sources, corrupt/unsupported ledgers and changed authority refuse.
Retraction/deletion makes a request unavailable for new disposition; archival
preserves evidence and does not resolve it. A role's later reader cannot
silently inherit a session-targeted request.

## Alarm correlation and escalation

Currently `pipeline_stall::Episode` retains only timing and delivery booleans,
and `supervise_cmd::nudge_pass` returns a display line rather than a message
ID. Change the internal send result to distinguish persisted mailbox message
identity, operator sent/queued, known refusal, and uncertain delivery. Persist
an episode UUID and exact request ID/fingerprint; do not infer correlation
from subject text. Version alarm state explicitly. Existing v1 accepted alarms
have no recoverable message ID: report action unknown and require explicit
recovery rather than inventing a receipt or resending automatically.

Proposed policy: one initial request; while the same predicate is continuously
true, escalate to the operator after 30 minutes without a disposition.
`acted` and `declined` close the request response obligation but do not prove
the pipeline recovered. If the predicate remains true for another 30 minutes,
escalate as a persistent stall, including the disposition and reason.
`deferred` postpones escalation until its deadline; it does not clear the
alarm. Do not send repeated advisor copies to simulate multiple nudges.
These defaults replace the parent's unspecified N-nudge policy only if approved.

Reuse `notify::send_direct_bounded` with a stable episode-specific rule key
and a ten-second bound. Respect configured quiet hours and operator opt-out;
queued means queued, never delivered. Record successful sent/queued escalation
once per episode. Known refusal remains retryable; reserve durably before
external delivery and retain ambiguous outcomes for explicit recovery, as the
current initial-delivery path does. No automatic lease release, rework launch,
merge or spec status mutation follows an alarm or disposition.

Predicate recovery closes its episode. Observation gaps/unknown state cannot
prove continuous persistence, and must not fabricate recovery or action:
retain request audit history, report unknown, and restart continuity timing
only when observations resume. Keep preview side-effect free, including no
ledger initialization, locks, receipts, notifier calls or recovery writes.

## Implementation and acceptance after approval

1. Add authorized action records and command/view projection without changing
   read ACK behavior. Update CLI help/JSON audit artifacts for the new leaf.
2. Return typed delivery evidence and correlate new alarm episodes to requests.
   Add explicit recovery for legacy or ambiguous state; never auto-migrate it
   into an acted/accepted assertion.
3. Add unresolved/persistent escalation using the existing operator notifier.
   Keep Claude/Antigravity delivery adapters in TASK-3072/TASK-3073 and other
   detector kinds in TASK-3075.
4. Test recipient versus sender/foreign/expired authority, shared-role policy,
   read/wake/reply independence, legacy evidence, partial/conflicting snapshots,
   changed replicas, unsafe anchors, concurrent revisions, corrupt/full ledgers,
   and archive/retract behavior. With injected time and delivery, test deadline
   boundaries, response versus predicate recovery, unknown/gap handling,
   cooldown/deduplication, quiet-hours/opt-out, pre/post-send persistence failure,
   uncertain delivery recovery and preview byte preservation. Run targeted Rust
   tests, CLI artifact checks, formatting and the relevant fast guards.

Approval requested: receipt/CLI schema and authority, one-reader shared-role
resolution, revision policy, deferred deadline limit, response-versus-recovery
semantics, legacy recovery, and the proposed single-request/30-minute
escalation policy. Until approved, leave TASK-3074 unfinished.
