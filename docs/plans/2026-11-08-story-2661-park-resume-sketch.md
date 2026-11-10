# STORY-2661: park on ask, resume on answer

Status: signed sketch; advisor authorized runtime-record slice 1 on 2026-11-10.
<!-- trace:STORY-2661 | ai:codex -->

The binding advisor ruling requires a sketch before code. This document is
that review artifact, not an implementation or authorization to resume.

AC1–4 remain open; this slice does not complete the story.

### Implementation slice 1 (2026-11-10 advisor ruling)

The canonical question schema now has optional stable `question_id` and
`park_binding` fields. New question constructors assign UUIDs; legacy unbound
questions deserialize unchanged. `park_runtime.rs` supplies the host runtime
record, private bounded file I/O, permanent per-wait locks, atomic durable
replacement, identity hashes and conservative restart reconciliation. Migration
assigns an ID once; callers must persist it under the canonical store writer
lock before binding. The lock order for future integration is ownership lock,
then store writer lock. These primitives do not enable park acceptance.

A binding without a record, a record without its canonical question, conflicting
identity, or an observed answer that changed is recovery uncertainty. All
non-released states retain ownership regardless of PID, PR, dirty edits, or grant
expiry; an answer or delivery acknowledgement never releases ownership. Runtime
read failures must likewise retain ownership at future consumer boundaries.
Records reject identity replacement and resurrection after explicit release.

This slice deliberately adds no exit handshake, answer dispatch, vendor adapter,
watcher, CLI parking surface or mutation/cleanup integration. Before enabling
parking, the later integration must wire the shared retained-work probe into the
closed reclaim inventory and serialize those writers with acceptance and checked
release. The record's `released` state is data for that future checked protocol,
not a cancellation command or authority grant. Tests in `park_runtime_tests.rs`
cover record persistence, incomplete canonical/runtime writes, interrupted atomic
replacement, stable legacy migration, blocked ownership, changed evidence and
unsafe files. They do not claim the future integrated R19–R23 regressions.

Initial scope decision (R2): parking plus **manual authorized resume only**.
Answer arrival makes a wait ready and observable, but does not launch a child.
AC2 remains open; this is not delivery of the automatic-resume story. A later
advisor-approved service-delegation design is required for automatic dispatch.
Admission and `aida awaiting` must say `manual_resume_required` before parking,
including when a watcher is healthy; heartbeat is never launch authority.

## Existing seams and limits

`questions.rs::questions_ask` persists a DecisionRequest;
`questions_answer_one` and the batch answer path call `finalize_answer`, which
persists the resolution and invokes `maybe_autoqueue` when ready. Neither binds
the answer to the asking lane. Both answer paths must share the new handoff.
Ordinary questions without a lane binding retain their existing behavior.

`session.rs::spawn_vendor_headless_resume_granted` already dispatches Claude
and Codex resumes, but refuses AGY. The SPIKE-711 host results establish the
AGY CLI surface, not integrated admission. The shared contained wrapper in
`launch/contained.rs` explicitly refuses Codex resume/fork pending private-state
lifecycle support (PR2), and also refuses Claude resume on that wrapper.
Legacy Claude admission is assessed in the path table below; it remains distinct
from the shared private-state wrapper.

## Durable records and ownership

Keep the canonical human question on its spec. Add a versioned host-owned
park record under the main project's runtime state, keyed by immutable wait UUID,
with a per-record lock and atomic replace. Include the owning host/node ID;
only that host can launch this wait. Other nodes can answer but cannot claim
its retained seat. Proposed fields:

| Field | Purpose |
|---|---|
| version, wait_id, generation | Closed schema and compare/update identity |
| spec_uuid, question_id, question_revision/hash | Bind one exact question, not an index that can move |
| owner_node_id, lease_id, run_id, phase, owner_role | Route the original lane and prevent phase advancement |
| vendor, vendor_session_id, capture source/generation, harness_version | Exact transcript identity and adapter evidence |
| seat_path, worktree identity, branch, head_sha | Detect seat removal, replacement, or unrelated changes |
| posture, grant reference, requester identity | Revalidate authority; no credentials or reusable grant tokens |
| state, created_at, updated_at | Lifecycle and recovery diagnostics |
| stop_boundary, stop_deadline, completion outcome, launch generation | Durable stop budget and generation-bound command completion evidence |
| answer revision/hash, answer event reference | Durable delivery key; answer content stays in canonical evidence |
| attempt_id, child receipt, acknowledgement | Recover dispatch without assuming successful delivery |
| transport archive generation/digests, preparation state | Recover consumption and clearing of per-launch seat files |
| post_exit_stage, publication declaration/digest, publication receipt, cleanup checkpoint | Recover post-exit processing without replaying the vendor or assuming publication |
| error/recovery reason | Visible escalation or admission block |

### Canonical question identity (new schema)

`DecisionRequest` currently has no ID or park binding, and a requirement has only
one `decision_request` slot. Add optional serde-defaulted `question_id: UUID` and
`park_binding: { version: 1, wait_id: UUID, generation: u64, owner_node_id }` fields.
New asks get a fresh question ID; legacy unbound questions remain readable and
keep existing semantics. An existing legacy question must receive an ID under
the store writer lock before it can be bound. The immutable question hash covers
question text, choices (including resolution and consequence), recommendation
and rationale, excluding answer fields. Answer identity covers the selected
choice, note and answered_at; changed answers never silently redeliver.

All question writers must refuse replacing a question with an active park binding,
including `questions ask --force`, even after it is answered but before delivery
is acknowledged. Explicit host cancellation must durably cancel the wait and
release the binding under checked identities before replacement is allowed.
A synced deletion, replacement or conflicting binding is an escalation, never
an answer to the old question. Preserve cancelled/delivered question evidence
in canonical history before allowing the single slot to be reused.

The host validates lane-supplied park intent against its current lease, actual
vendor transcript identity, and seat identity. An arbitrary session ID supplied
by the model grants no resume authority. Do not use `--last` or transcript search
as a fallback. Runtime files require bounded regular-file reads, no symlink
traversal, private permissions, and the established runtime writer discipline.

### Exact vendor session capture (new host admission preflight)

The host persists capture provenance with the launch generation before accepting
any canonical park question. The model's intent cannot establish identity.
Missing, conflicting or malformed identity refuses park before the canonical
question/binding is accepted; retain the submitted intent as a publication veto
and show `session_identity_unavailable` with manual recovery guidance.

| Vendor | Capture source and timing | Initial limitation |
|---|---|---|
| Claude | Host-minted UUID passed as `--session-id` before fresh exec; for a resumed launch, the exact admitted `--resume` ID. Bind it to the launch receipt and verify the exact transcript before parking. | Never substitute a model-supplied ID or locate another transcript. |
| Codex | Validated vendor-session lease stamp bound to this launch, or `thread_id` from this child's captured `codex exec --json` `thread.started` event, persisted as it arrives before ask acceptance. Conflicting evidence refuses. | Use only these two sources; do not call `resolve_thread_id` unchanged, since its unique-rollout fallback is forbidden here. |
| AGY | SPIKE-711's verified `agy --print=... --output-format json` result field `conversation_id`, captured from this child's completed output and persisted by the host. This is research evidence only. | AGY cannot park in this slice: there is no admitted bootstrap into a prior-ID resumed lane, and no verified mid-turn capture. Refuse `--park` before intent submission, canonical question creation or a stop instruction. Ordinary completion/publication remains available. |

AGY hook `conversationId` is not accepted as headless capture evidence in this
slice. Do not guess an event contract or scan transcripts. The host spike proves
resume, not integrated park eligibility. AC3’s AGY park/resume adapter is
unreachable until verified mid-turn capture and normal resume admission exist;
these are deferred prerequisites, not a path enabled by this sketch. This scope
limit requires advisor acceptance. Fixtures cover missing identity for eligible
vendors, Codex conflicting/stale stamps, and AGY refusal before submission with
no intent, question, stop instruction or publication veto.

The parked lane retains exclusive ownership of its seat/worktree and branch,
including dirty edits. It holds **no running concurrency slot**, no child process,
and no continuing grant authority merely because it is parked. Other work may
use the released slot in another seat. Pickup and GC must recognize parked
ownership and refuse reuse/removal until explicit cancellation or recovery.

### Parked work is live work (R19)

An open host-owned wait is durable work ownership even after its process lease
is dead or reaped. Extend `live_work::spec_live_work` to return `Keep` for an
exact spec UUID/current canonical park binding with a nonterminal wait, including
blocked authority, delivery uncertainty and post-exit recovery. This evidence
does not require an open PR, committed changes or a live PID. It grants no
running-slot or launch authority. Delivered acknowledgement alone does not end
ownership: retain it through resumed work and the post-exit pipeline until
explicit checked completion/cancellation releases it.

Doctor's status-drift detection and heal must use this shared ownership probe;
heal rechecks it at the status-write boundary under the ownership lock, so a
new park cannot race an already collected no-lease finding. An unreadable,
conflicting or partially persisted binding retains the spec and reports recovery
uncertainty rather than authorizing a reset to Approved. Use a documented common
ownership/store lock order for park acceptance, cancellation and this heal.
`ps_integrate` orphan detection and its `orphaned_in_progress_items` projection
must use the same evidence: show a parked/recovery wait instead of abandoned
InProgress work, without inventing a live lease. Pickup, worktree GC and doctor
worktree/branch cleanup also honor retained ownership. Only checked release
restores ordinary abandonment/cleanup rules; timestamp age is insufficient.

`claim.rs::persist_unclaim_status` must consult the same retained-ownership
evidence at its locked status-write boundary (R20). Releasing a shell claim
removes only that claim; it does not cancel a wait or release its seat. A dead
lane lease and confirmed absence of an open PR are insufficient to return a
parked spec to Approved. Recheck the current canonical binding and host wait
under the common ownership/store lock order before calling `unclaim_status`;
an active wait or unreadable/conflicting ownership evidence retains InProgress
and the wait, with recovery uncertainty reported. This applies even if park
acceptance races the earlier lease/forge lookup. Only checked cancellation or
completion releasing retained ownership restores ordinary unclaim behavior.

Metadata-only `aida queue rework` must consult the same retained-ownership
evidence at its locked status-write boundary (R21). Today,
`queue_cmd.rs::rework_target_for_mode` selects Approved for InProgress when no
work is launched, while `requeue_lease_gate` protects live leases; a parked
lane's dead/reaped lease is insufficient protection. Before applying that
status flip or inserting/re-routing a queue row, recheck the exact canonical
binding and host wait under the common ownership/store lock order. Refuse
rework while retained ownership exists, including with `--force`; require
explicit checked cancellation/completion releasing that ownership first.
Unreadable, conflicting or partially persisted ownership evidence also refuses
without mutation and reports recovery uncertainty. Recheck at the write
boundary even if the earlier lease check passed, so concurrent park acceptance
cannot be overwritten. A rework reason, dead lease or missing PR does not
cancel a wait. Refusal preserves InProgress, the question/binding, wait, dirty
seat and existing queue state; it never manufactures a status-mismatch block
for an otherwise eligible bound resume.

### Closed inventory of reclaim doors (R22, R23)

Retained ownership is a separate gate from PID lease liveness, status
preflight and force flags. All of the following doors must consult the same
canonical-binding/host-wait probe under the common ownership/store lock order
at their final mutation or claim boundary:

| Door | Protected boundary |
|---|---|
| CLI `aida claim` (`claim.rs::handle_claim`, `persist_claim_status`) | Before creating or refreshing a scope lease and before changing status or `req.owner`. |
| MCP `claim_task` (`mcp.rs::tool_claim_task`) | Before creating a scope lease, even after the original lane lease has been reaped. |
| CLI metadata-only `aida queue rework` | Before status changes, queue insertion or routing. |
| MCP `queue_rework` (`mcp.rs::tool_queue_rework_inner`) | Before status changes or queue writes, including explicit status overrides and `force: true`. |
| CLI `aida queue rework --work` | Before queue/status writes and again before claiming or launching fresh work, even when status remains InProgress. |
| Ordinary pickup and direct `session_start` | Before lease/seat claim, status bump, adoption or fresh child launch. |
| Explicit `--force-claim` | The same claim boundary; forced status preflight cannot bypass retained ownership. |
| Orchestrator-corroborated auto-claim | The same claim boundary, including parent phase registration; corroboration does not supersede a parked owner. |
| Orphan adoption (`session_start` and `leases.rs` adoption path) | Before adopting/rebinding the lease or seat; a dead lease with an open wait is retained work, not an adoptable orphan. |
| `persist_unclaim_status`, doctor status-drift heal, orphan detection, pickup and worktree/branch cleanup | The ownership/status/cleanup boundaries described above. |

This is the closed reclaim inventory for implementation review; trace each
entry to its final writer/claim helper and test it before enabling park. Share
one gate rather than duplicating a live-lease-only check in each surface.
Retained ownership refuses **even with `--force`, `force: true`, or
`--force-claim`**. An unchanged InProgress status is not an exemption: creating a
second session or queue route also violates ownership. Unreadable, conflicting
or partially persisted evidence refuses without mutation. Refusal directs the
caller to bound `aida lane resume` or explicit checked cancellation; it must not
recommend a force flag as a recovery bypass. Only checked cancellation/completion
releasing ownership allows ordinary reclaim. Bound resume itself verifies the
exact wait/generation and uses its dedicated continuation admission; it is not
an exemption available to fresh sessions. Explicit `aida edit --status` retains
the already specified visible status-mismatch behavior.

The two claim verbs refuse while retained ownership exists (R23); they are
not advisory-only exceptions. CLI claim must not refresh an existing shell claim,
create a lease, or overwrite `req.owner` through `claim_status` on a parked spec.
MCP claim_task must not infer availability from an absent lease file. Both
recheck retained ownership at their final claim boundary, serialized with park
acceptance; the CLI status/owner write rechecks under the common lock order too.
Uncertain evidence refuses without lease, status or owner mutation. Checked
cancellation/completion is required before ordinary claims become available.

Bound resume also checks `live_spec_claim_by_other`: a live scope claim by
another session, including a legacy claim created before this gate, blocks
resume visibly in `aida awaiting` as a claim conflict. Retained ownership stays
intact; no claim is stolen, ignored or force-overridden. Recovery requires the
other claimant's explicit release (or ordinary verified dead-claim cleanup),
then a fresh checked bound-resume attempt. Releasing that conflicting claim
must preserve the parked spec's status and owner under the unclaim rule above.
The dedicated continuation may acquire its own lease only after validating the
exact wait/generation and all existing authority and ownership gates.

## Park and exit handshake

### Seat-to-host transport (new surfaces, R4)

Propose `aida questions ask <SPEC> --park` using the existing question inputs.
Check vendor/posture eligibility before submitting an intent. In particular, AGY
returns an unsupported-mode refusal without creating transport files or a bound
question, and without instructing the lane to stop; ordinary questions remain available.
Before launch the host writes a private expected binding under
`<main>/.aida/park-bindings/<lease-id>.json`, and supplies a read-only copy at
`<seat>/.aida/park-binding.json`. The closed binding contains version, random
launch token, lease ID, spec UUID, run ID and launch generation, following
`native_publication.rs::Binding`. It is scoped to this launch, not a grant.
The seat atomically writes `<seat>/.aida/park-intent.json` with that binding,
wait UUID, question UUID and bounded question payload. It never writes host runtime
state. Reject links, nonregular/multiply linked files, wrong owner/mode, oversized
payloads, stale generations and token mismatches before accepting an intent.

The live host supervisor polls that exact file at most every second, including
while collecting vendor output. Under current authority it writes the canonical
question plus binding through the existing store writer lock, then the host wait
record, then `<seat>/.aida/park-receipt.json`. This host-mediated canonical write
works when containment forbids direct store access; the seat needs only its own
write root. The ask command waits at most 10 seconds for the matching receipt,
then returns pending/failure guidance, never a false park acknowledgement.
A pending result requires the lane to stop work and end its current turn, just
as an accepted receipt does; it must not continue editing or attempt publication.
A failure after intent submission has an uncertain acceptance outcome and requires
the same stop pending host reconciliation. Only a definitive pre-submission
refusal establishes that no park intent exists. Retries reuse the wait UUID.
Late receipts remain recoverable and the supervisor
suppresses publication on any present intent, including malformed or rejected
intents. Both canonical and runtime
state must reconcile before acceptance; crashes after either write are visible.
The receipt is host evidence, never accepted back as authority from the seat.

1. An eligible explicit blocking lane ask submits the durable seat intent; the
   host writes the canonical question and binding as described above. Do not
   infer blocking asks from arbitrary model prose. After submission the lane
   stops work and ends its current turn whether the CLI returns an accepted
   stop-now receipt, pending guidance or an uncertain failure. A definitive
   pre-submission refusal creates no stop obligation.
2. The host binds and accepts the intent before publication, phase completion,
   ordinary background-work resume, or lease cleanup. A park is neither a
   successful implementation nor a failed phase to retry from scratch.
3. For a still-live child, before accepting an intent or exposing its receipt,
   the host marks the
   original launch slot waiting via `mark_slot_waiting` (or the equivalent
   durable flag under the shared slot lock), disabling final-result discounting
   throughout `park_requested`. Failure to retain that reservation refuses
   acceptance and escalates; the present intent still vetoes publication. No
   accepted park may have a discountable live slot.
   The current best-effort helper needs a checked persistence/readback result
   for this handshake; invoking its void-returning API alone is insufficient.
   Already-exited children use the exit barrier below instead.
   This rule covers original launches as well as managed resumes. State advances
   `park_requested -> parked` only after the host verifies the
   vendor child has exited and transcript identity is retained. Until exit,
   its running slot remains occupied. No process is killed by the ask command.
4. Failure to persist either side refuses the handshake with recovery guidance;
   a question alone does not release a live slot. A lane that fails to stop
   becomes `stop_timeout` if its managed vendor process group has not exited
   within 60 seconds of the stop boundary below.
   This is a wall-clock safety deadline, not a claim to detect model turns.
   A slow final response can time out; that is an operational failure requiring
   recovery, not evidence of a second turn. No Claude, Codex or AGY stream parser
   is assumed to identify turn boundaries in this slice. The one-turn stop is
   a cooperative instruction for all three; a future per-vendor event contract
   would need separate fixtures before enforcing a turn-count rule. The host
   observes command completion via a bound seat completion marker written just
   before every post-submission CLI return, including pending/failure, carrying
   the same wait and launch generation and its return outcome. If the command
   already returned when the host accepts (late acceptance), the deadline starts
   at host acceptance, regardless of that return outcome. Otherwise it starts
   when the matching completion marker is observed. Missing marker uses
   acceptance plus 10 seconds as a conservative fallback and reports
   `stop_boundary_unknown`. A marker observed after the fallback cannot extend
   an established deadline. The host records the chosen boundary and deadline
   durably for restart reconciliation; a restart never resets the budget.
   The supervisor owns a monotonic deadline while alive: at expiry it stops its
   verified child process group,
   gives it 5 seconds to exit, then kills that same group if necessary and reaps
   it. This is a proposed new supervisor behavior requiring advisor approval,
   not permission for the ask command to kill anything. Preserve transcript and
   dirty edits; suppress publication and escalate the failed cooperative stop.
   Free the slot only after verified exit; if descendants cannot be accounted
   for, retain an uncertain-capacity reservation and require operator recovery.
   Only the cooperative path satisfies AC1; timeout is an explicit failure.

### Exit barrier, including unaccepted intents

Every managed launch must perform a final synchronous intent drain after vendor
exit/reaping and output collection, before publication, phase completion, lease
cleanup or SlotGuard release. Polling is only an early notification path. The
host retains its guard through this barrier even if the child exits between
polls. Once the managed child/group is verified exited, inspect the exact intent
path with the same safe bounded reads; an unreadable or unsafe path is evidence
requiring escalation, not absence. Any present intent suppresses ordinary success
publication, even if rejected for a bad token, oversize payload, unsupported
posture, missing identity or reservation failure. Rejection preserves diagnostics
and dirty seat ownership for explicit recovery; clean vendor exit never turns
that refusal into a publishable result. This permits denial of publication by
the seat, not authorization to park or resume.

For a valid intent first read after verified exit, accept through the same
canonical/runtime reconciliation and exact-session preflight. Record exit
identity and accept directly into `parked`; no live-child waiting mark or stop
deadline is needed. The original slot guard is still held through durable seat
retention, acceptance and receipt persistence, then released. If group exit is
uncertain, retain capacity and escalate instead. For invalid/unaccepted intents,
write the rejection/recovery state and retain the seat before releasing capacity
only on verified group exit. No answer or late receipt can bypass this barrier.
A crash during the barrier leaves an unresolved launch reservation for host
reconciliation, never inferred publication authority.

### Consuming transport between launch generations

After the exit barrier has durably reconciled an accepted park, the host archives
that generation's intent, receipt and command-completion marker in its private
wait history, recording their identities/digests under the wait lock. The original
barrier still vetoes publication for that parked run. Rejected or unsafe files
remain a recovery block; never erase them just to make a launch succeed.

Before every new launch generation (including manual resume), hold the wait and
seat locks, verify the previous child/group has exited, and complete an
idempotent prepare transaction: persist the prior accepted transport archive,
remove the consumed intent, receipt and completion marker from the seat, verify
all three are absent, then persist the fresh generation/token binding. No child
may spawn until preparation is durable. Any archive, read, clear or binding error
fails closed with `transport_prepare_blocked`. A crash resumes preparation from
its checkpoint under the same locks; it cannot authorize publication or discard
unresolved evidence. This follows the per-launch clearing principle in
`native_publication.rs::prepare`, extended to these three park artifacts.

The resumed run's exit barrier inspects its current generation's transport paths.
A new intent (including a mismatched or unsafe one) still vetoes publication; the
archived consumed intent does not. A resume that finishes cleanly without a new
ask can therefore proceed through ordinary publication gates. Retain host history
through cancellation/completion; transport removal is not history deletion.

Canonical and runtime writes cannot be one transaction. Use the immutable wait
ID on both sides; host reconciliation repairs an interrupted handshake and
reports unmatched questions/intents. Never claim a parked receipt before both
writes are durable. `aida awaiting` joins these records and shows question,
lane/spec, vendor/session ID, state, and recovery reason.

## Answer trigger and dispatch

The authoritative trigger is the canonical question becoming answered, with the
same question ID, binding and immutable question hash. The owning host derives
`resume_ready` from that evidence, even if the answer was written on another
node and later synced. `finalize_answer` may emit a local outbox wake hint after
its authorized write succeeds, but inability to access host runtime state must
not invalidate the answer. Hints carry identities only and cannot authorize a
spawn. A contained answerer needs no write access to host runtime state.
For a bound wait, suppress generic autoqueue so a second implementer cannot
race the retained lane. An answer that deliberately keeps work parked, rejects
it, or leaves another gate unsatisfied does not resume it.

### Resume consumer and availability (new service)

Introduce a host-only `aida lane resume-watch --project <main-root>` service,
installed as a per-user systemd service with `Restart=on-failure` and enabled at
login; surviving logout requires operator-enabled user lingering. This is a new
service to implement, not an existing launcher loop. The short-lived launcher
persists seat ownership in runtime records before exiting; the observer only
reconciles these records and cannot take launch authority.
The service holds one project-scoped singleton lock, scans waits at startup and
at most every 5 seconds, and checks canonical store state without model polling.
It never needs the exited launcher's PID to remain alive. Existing store sync
makes remote answers visible; the service does not invent a new remote sync or
fetch authority. `aida awaiting` reports last scan, observed store revision and
sync errors so an unsynced remote answer remains visibly awaiting local evidence.

This initial observer is optional for manual parking. It reports readiness and
never spawns, even when launched from a delegator. Its systemd unit uses
`KillMode=control-group`: it has no resumed children to preserve on restart.
No service heartbeat can make a wait automatically resumable. `aida awaiting`
reads canonical and runtime evidence directly even with the observer stopped,
shows `manual_resume_required`, and shows stale scan/sync evidence separately.

### Per-child running-slot ownership (R3)

`aida lane resume <wait-id>` is a foreground host supervisor, one process per
wait. It acquires its own PID/start-identity slot via `acquire_slot`, before
launching the vendor, and holds its guard through vendor exit/reaping and the final intent barrier; it never
uses the observer PID for child slots. Observer restart cannot release these
slots or kill these processes, which are outside the observer unit cgroup.
Existing final-result-based slot discounting must be disabled for both the
original `park_requested` launch and this managed resume until verified child
exit; a vendor final message is not process exit.

Supervisor death must not make a surviving child invisible to the cap. Before
spawn persist an attempt reservation under the shared slot lock; after spawn
bind the child PID/start identity. Extend slot reconciliation to count either
live supervisor or child, and conservatively count unresolved spawn windows.
This is a required slot schema/reader change, not existing `acquire_slot`
behavior. All slot admission paths must count reservations under the same lock.
Only verified child/group exit clears a reservation; ambiguous descendants
require operator recovery. Tests must kill the supervisor and restart the
observer with live children, then attempt admission at the cap.

The observer reconciles readiness after restart, including
an answer arriving before the original child exits. The manual resume supervisor acquires an ordinary
running slot and the seat lock before `resume_ready -> resuming`. It rechecks
current grant/parent validity, role, the resume predicate below, holds/punts,
seat identity, branch/HEAD, vendor transcript availability, and the original containment posture.
The answer is data, never approval or grant renewal.

### Parent grant and overnight recovery

Only the foreground manual resume command may mint a child, using its caller's
currently validated delegating parent seat grant through `issue_launch_child` /
`issue_child_for`. The watcher never mints one and has no credential handoff.
The original implementer's grant is never resurrected. A systemd start or restart
has no launch authority. Do not copy terminal tokens into units or wait records.

An answer is always `manual_resume_required`; a manual attempt without a valid
parent becomes
`blocked_authority`, retaining the answer and seat. `aida awaiting` must show
this even when no observer is running, plus the explicit recovery: an authorized
operator enters a permitted parent role in a live host shell with an explicit
seat delegation for the wait's `owner_role`, then runs the proposed resume command.
For an implementer wait, the shell-init helper commands are
`aida role enter <permitted-parent-role> --delegate-seat implementer`, followed
by `aida lane resume <wait-id>`. Without the helper, capture the role command's
output and evaluate it only after successful exit and validation of the complete
standalone eval-marker pair, using the existing guarded shell-init protocol.
For other waits substitute their exact `owner_role` for `implementer`.
`aida awaiting` must print this delegation flag and the wait's concrete role
in its recovery guidance. A bare role entry without `--delegate-seat` is
insufficient; role admission and delegation validation still apply.
That command claims the wait under the same per-wait lock/attempt protocol and
supervises the resumed child under that caller's fresh parent grant. It does not
renew the exited session or grant the daemon permanent authority. The watcher
only observes; even a live delegator does not enable automatic spawning. Unattended overnight resume
requires a separately approved service-delegation lifecycle; this sketch does
not introduce one. Advisor acceptance of this manual authority recovery limit
is required before implementing this manual-only partial slice.

### Status and resume predicate

Parking preserves `InProgress`; it does not use NeedsAttention, Approved, queue
completion or an ordinary failed-phase shelf as a surrogate for a human wait.
The runtime wait blocks phase advancement and generic queue pickup. Resume is
allowed only for the original InProgress spec with its exact active binding,
a verified exited child, an unchanged answered question, a resolution that does
not keep it parked/reject it, and the new `evaluate_bound_resume` predicate below,
plus all authority/seat/posture checks above. Do not call `evaluate_unpark`,
`burndown::classify`, or use autonomous implementer lane classification here.
Do not call
`maybe_autoqueue` or require its Approved-only predicate. A resolution changing
status to Approved or any other status blocks bound resume with a visible
status-mismatch recovery reason; it never silently resets that status. Lost or
changed bindings, terminal status and independent holds also refuse. Clearing
such a block requires authorized recovery and revalidation, not an answer alone.
A changed seat/HEAD requires explicit recovery rather than silently discarding
edits or switching sessions.

`evaluate_bound_resume` is a new explicit continuation check, not a new-work
pickability check (R18). Share the structural deferral and `BlockedBy` checks
with `evaluate_unpark`: every blocker must resolve to Completed, and missing or
unreadable targets refuse. Require the exact bound answered decision and refuse
any additional pending decision, independent hold/punt, or unresolved design,
decision, draft-review or deferral gate. Preserve existing manual-clear and
reasoned-defer authority requirements. Do not clear gates merely because an
answer exists. Generic autoqueue retains its current predicate.

A research spike may continue in its original admitted research role; its type
alone never refuses bound resume or routes it into an implementer lane. Retain
the original execution mode. `needs-human`, `needs-human-input`,
`needs-supervised-build` and `operator-action` are supervision requirements here:
continuation requires fresh explicit supervised admission for that original
role and mode, otherwise return `blocked_supervision`. They are not unconditional
type/lane exclusions. Other parking tags (`blocked`, `needs-design-signoff`,
`needs-design`, `needs-decision`, `review:draft-only`, and `deferred:*`) remain
blocking until independently resolved through their authorized surfaces. This
closed distinction must be explicit in tests; unknown gate evidence refuses.
The same predicate runs before spawn after acquiring the wait/seat locks, not
only when the answer is observed. Research completion uses its original role's
completion pipeline; it does not gain implementer publication requirements.

Prompt data contains the bound question, selected answer, consequence and the
complete free-text `DecisionRequest.note` (including an explicit absent note), with
their provenance and wait ID. Pass it as an argv element or existing supported
stdin transport, never shell interpolation. Launch the exact session in the
retained seat. The first successful resumed receipt acknowledges the wait;
subsequent questions use new wait IDs. The foreground `aida lane resume`
supervisor owns the entire post-exit pipeline described below.

Compare/update under the wait lock and a single attempt ID prevent duplicate
answer events from spawning duplicate children. Persist launch reservation before
spawn and the child receipt immediately afterward. An ambiguous crash between
spawn and receipt becomes `delivery_unknown`, consumes capacity until reconciled,
and requires host process evidence or operator recovery; do not blindly replay
an answer. Failed admission stays visibly blocked and retryable without changing
the durable answer. Missing seats or transcripts become escalations, not fresh
sessions. Cancellation retains history and requires explicit ownership release.

### Post-exit owner and interrupted completion

The foreground `aida lane resume` process owns output collection, verified group
exit, final intent drain, publication gates/declaration handling, phase completion,
reviewer handoff and lease cleanup through terminal success or a durable block.
It invokes the same checked pipeline as the original launcher; the exited original
launcher and observer service have no completion responsibilities. A new park
branches back to retained ownership and suppresses publication as above.

Under the wait lock, persist a `post_exit_pending` checkpoint with the child exit
receipt, generation, result/declaration digest and intended next stage before
publication. Record each external publication receipt before advancing to phase
completion, reviewer handoff and cleanup. Each stage uses a stable attempt key
and rechecks ordinary authority, hold/punt and publication gates. Release running
capacity only after verified group exit and the exit barrier; retain seat ownership
until the pipeline has durably completed or been explicitly recovered.

If the foreground supervisor is interrupted after vendor exit, the observer only
reports `post_exit_recovery_required`; it never publishes. A fresh authorized
`aida lane resume <wait-id>` detects the checkpoint and continues post-exit
processing without relaunching the vendor or redelivering the answer. It verifies
the same seat/result and existing publication/handoff receipts before each stage.
An ambiguous external side effect must be reconciled against host/forge evidence;
if it cannot be proved, block for operator recovery rather than repeat it. Cleanup
cannot erase unresolved checkpoints, dirty edits or ownership. Interruption before
the first post-exit checkpoint uses the persisted launch reservation and child
receipt to establish exit/result evidence, otherwise escalates without publication.

## Vendor contract and initial scope

| Vendor | Headless resume argv shape | Admission proposal |
|---|---|---|
| Claude | `claude -p --resume <session_id> <answer_prompt>` | Legacy contained headless path only, with retained verified transcript as specified below; shared-wrapper private-state resume is excluded. |
| Codex | `codex exec resume <session_id> <answer_prompt>` | Initially only paths without AIDA containment that already admit resume, including trusted native workspace-write posture where admitted. Native is reported as native sandbox, not AIDA-contained. |
| AGY | `agy --conversation <session_id> --print=<answer_prompt>` | Deferred, unreachable in this slice. Refuse park before intent submission; verified mid-turn identity capture and integrated resume admission are prerequisites. |

### Claude path decision (R5)

| Launch path | Initial park eligibility | Transcript after exit |
|---|---|---|
| Default legacy AIDA-contained Claude using `session.rs::spawn_claude_headless_resume_granted` | Yes for manual resume after retained-transcript preflight; preserve contained flags, mailbox preparation and child grant checks. | Retain the exact session JSONL in the host Claude project store keyed by original seat cwd (`~/.claude/projects/<cwd-slug>/<sid>.jsonl`); retain seat ownership and prevent transcript cleanup while parked. |
| Legacy lease-tree session listed under another checkout | Yes only after checked staging succeeds. | Use `stage_resume_transcript` into the retained lease-tree project store; require an actual verified destination, since the helper's missing-source success alone is insufficient. Conflicting bytes refuse. |
| Shared `launch/contained.rs` private-state Claude wrapper | No. | Private config lifecycle is not retained by this proposal; existing resume refusal remains. Ordinary manual escalation only. |
| Uncontained Claude | Only where ordinary launch policy already admits it. | Same exact-cwd transcript retention/preflight; no downgrade from containment. |

Thus the default legacy contained Claude adapter is reachable in the manual
slice, subject to integrated tests, but AC3 remains open until all three vendors
have evidence. Transcript loss escalates before spawn. Codex AIDA-contained
resume remains excluded until PR2; trusted native Codex is a distinct posture.

These are identity/prompt shapes; approved model, permission, environment and
output flags still come from the vendor registry and shared launch composers.
Never drop inherited containment to make a resume work. In particular, contained
Codex under AIDA containment cannot enter the resumable park protocol in this slice. Refuse
that mode before claiming park support, retain the human question through the
ordinary manual escalation path, and explain the PR2 dependency. The same rule
applies to any unsupported contained Claude or AGY surface. This scope restriction
requires advisor acceptance; the story's three-vendor acceptance is not complete
until each adapter and its admitted posture have integrated evidence.

## Implementation and verification after signoff

Implement the runtime record and reconciliation first, then host exit/publication
handling and seat retention, then the common answer outbox and vendor adapters.
Reuse store locks, launch admission, injected execution context and environment
projection; do not create another ambient-reader or allowlist mechanism.

Recorded fake-harness tests must exercise each eligible vendor resume with exact
ID, answer and cwd; missing-session-ID refusal before question acceptance;
AGY refusal before intent submission with ordinary clean publication still allowed;
future AGY adapter fixtures are required when its deferred prerequisites are met;
park, archive/clear transport, resume and clean exit reaching publication;
preparation failures and restart at every archive/clear/binding boundary;
new-generation malformed intent still vetoing publication;
manual resume reaching phase completion, reviewer handoff and cleanup;
interruption after vendor exit and around each publication/checkpoint boundary,
recovering without vendor relaunch, duplicate publication or lost ownership;
exit within the poll interval followed by final intent drain; rejected intent
after a stop (bad token, oversize and reservation failure), with no publication;
valid late intent accepted after verified exit without a live-child waiting mark;
question persistence before exit on the live-child path; slot release only after exit;
retained dirty seat; answer-before-exit; duplicate and batch answers; restart at
each write/spawn boundary; uncertain delivery; lost seat/transcript; expired
grant; observer restart with no grant and fresh authorized manual resume;
observer restart with multiple live resumed children; supervisor death and cap
reservation recovery; native and contained seat intent/receipt transport; stale
binding tokens and interrupted canonical write; legacy contained Claude transcript
staging, missing/conflicting transcripts and shared-wrapper refusal;
remote-node answer synced after park; contained answerer without runtime access;
lost wake hint; parking and `aida awaiting` with the observer stopped; stale
observer diagnostics without an admission refusal; sync errors;
legacy question migration; forced overwrite before/after answer; synced binding
replacement; free-text note such as “1, but name it X” in each vendor prompt;
InProgress resume versus Approved/status-mismatch refusal; cooperative one-turn
exit and 60-second stop timeout with surviving/ambiguous descendants;
pending-return stop followed by late acceptance with a deadline starting at
acceptance, missing completion marker and restart without deadline extension;
admission at cap during `park_requested` after the original vendor emits a
final result but before verified exit; recovery with explicit owner-role
`--delegate-seat` and refusal when that flag is absent;
hold/punt vetoes; rejected/kept-parked answers; and containment refusal
before artifacts or child spawn. PATH changes use the existing isolated-test
helper. Host integration must demonstrate the child recalls both question and
answer, with no generic queue duplicate, and no publication while parked.

R18 regressions must park an admitted research spike, answer it and resume the
same session in the research role; verify supervised tags require fresh
supervised admission, while unresolved dependencies, deferrals and independent
design/hold gates still refuse. R19 regressions must park with only dirty
uncommitted edits, no PR/unmerged commits and a dead/reaped lease, then run
autonomous doctor heal and orphan detection: status stays InProgress, pickup
remains blocked, dirty seat survives and awaiting shows the wait. Repeat with
a stale pre-collected heal finding, acceptance racing heal, unreadable runtime
evidence and explicit cancellation; only checked ownership release allows the
ordinary abandonment path. These are required future implementation tests,
not tests executed by this sketch-only change.

R20 regressions must run unclaim from a shell holding a claim on a parked spec
with dirty edits, a dead/reaped lane lease and no open PR: the shell claim is
released, status stays InProgress, and the wait, binding and retained seat remain
intact. Repeat with park acceptance racing the pre-write lookup, unreadable or
conflicting ownership evidence, and checked cancellation; only the last case
permits the ordinary return to Approved after ownership is released.

R21 regressions must invoke metadata-only `aida queue rework` (without `--work`)
on an InProgress parked spec with dirty edits, a dead/reaped lane lease and no
open PR. Assert refusal leaves status InProgress, the active wait and canonical
binding intact, the seat unchanged and no queue row inserted or re-routed;
answering must still permit the ordinary checked bound-resume path. Repeat
with `--force`, park acceptance racing the earlier lease check, unreadable or
conflicting ownership evidence, and checked cancellation. Only checked
ownership release permits ordinary rework to Approved and queue insertion.
These are required future implementation tests, not executed regressions in
this sketch-only revision.

R22 regressions must exercise every reclaim door in the inventory separately:
MCP `queue_rework` with `force: true` and a status override; CLI rework with
`--work` (including `--force`); ordinary pickup/direct session start; explicit
`--force-claim`; orchestrator-corroborated auto-claim; and orphan adoption via
both session-start and lease adoption entry points. Use a parked InProgress spec
with dirty edits, a dead/reaped lease and no PR. Each refuses before status,
queue, lease, seat or child mutation and preserves the exact wait/binding and
resume eligibility. For each door repeat with park acceptance racing its earlier
preflight, unreadable/conflicting evidence, and checked cancellation: only the
last permits ordinary reclaim. Verify refusal guidance names bound resume or
checked cancellation, never force as a bypass. These are required future
implementation tests; this sketch does not claim they have run.

R23 regressions must exercise CLI `aida claim` and MCP `claim_task` separately
on a parked InProgress spec after its lane lease is reaped. Each refuses and
preserves `req.owner`, status, question/binding, wait and seat, without creating
or refreshing a scope lease. Include an existing same-caller CLI claim refresh,
park acceptance racing the lease creation and CLI owner-write boundaries,
unreadable/conflicting ownership evidence, and checked cancellation; only
checked ownership release restores ordinary claim behavior. Seed a legacy live
claim by another session and verify bound resume refuses with an awaiting
claim-conflict diagnostic and no child; explicit release preserves the parked
owner/status and permits a fresh otherwise-authorized bound resume. No force
flag may bypass either gate. These are future implementation regressions,
not executed tests in this sketch-only revision.

Publication metadata gate (R1): host/advisor must retitle PR #3666 exactly
`[AI:codex] docs(plans): sketch park-on-ask resume-on-answer (Refs STORY-2661)`.
Use non-completing Refs metadata for this documentation revision as well.
A seat edit or publication declaration does not repair the existing PR title;
retain the publication veto until title repair and advisor sketch signoff.

Review decision requested: approve the durable host-owned park/outbox protocol,
exclusive retained seat with a released running slot, and the initial admission
limits above, including manual-only initial scope (AC2 open), observer-only
resume-watch, per-child slot reservations, bound seat transport, explicit overnight authority
recovery, canonical schema additions, per-generation transport consumption,
post-exit recovery ownership, deferred AGY eligibility and bounded supervisor stop policy. No implementation
should start until the advisor records signoff.
