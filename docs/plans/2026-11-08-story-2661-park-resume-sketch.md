# STORY-2661: park on ask, resume on answer

Status: proposed; advisor signoff required before implementation.
<!-- trace:STORY-2661 | ai:codex -->

The binding advisor ruling requires a sketch before code. This document is
that review artifact, not an implementation or authorization to resume.

This revision addresses the protocol findings F2–F8, R2–R10 from commits
`39ab419eb862`, `48811d8051db` and `12ea0f7b294c`. F1/R1 requires a host
metadata action and remains unresolved here; no further seat commit clears it.
Publication must use a non-completing subject:
`[AI:codex] docs(plans): sketch park-on-ask resume-on-answer (Refs STORY-2661)`.
AC1–4 remain unimplemented; merging this sketch must not complete the story.
The native seat leaves edits uncommitted for the host to publish and must not
rewrite the earlier commit or PR itself. The host/advisor must retitle PR #3666
to that exact subject before publication/merge; a Refs commit alone does not fix
the completing PR title. Advisor signoff is still outstanding.

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
| lease_id, run_id, phase, owner_role | Route the original lane and prevent phase advancement |
| vendor, vendor_session_id, harness_version | Exact transcript identity and adapter evidence |
| seat_path, worktree identity, branch, head_sha | Detect seat removal, replacement, or unrelated changes |
| posture, grant reference, requester identity | Revalidate authority; no credentials or reusable grant tokens |
| state, created_at, updated_at | Lifecycle and recovery diagnostics |
| answer revision/hash, answer event reference | Durable delivery key; answer content stays in canonical evidence |
| attempt_id, child receipt, acknowledgement | Recover dispatch without assuming successful delivery |
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

The parked lane retains exclusive ownership of its seat/worktree and branch,
including dirty edits. It holds **no running concurrency slot**, no child process,
and no continuing grant authority merely because it is parked. Other work may
use the released slot in another seat. Pickup and GC must recognize parked
ownership and refuse reuse/removal until explicit cancellation or recovery.

## Park and exit handshake

### Seat-to-host transport (new surfaces, R4)

Propose `aida questions ask <SPEC> --park` using the existing question inputs.
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
suppresses publication on any validated pending intent. Both canonical and runtime
state must reconcile before acceptance; crashes after either write are visible.
The receipt is host evidence, never accepted back as authority from the seat.

1. An explicit blocking lane ask creates the canonical question and a durable
   park intent; do not infer blocking asks from arbitrary model prose. The CLI
   returns a stop-now receipt and the lane ends its current turn.
2. The host binds and accepts the intent before publication, phase completion,
   ordinary background-work resume, or lease cleanup. A park is neither a
   successful implementation nor a failed phase to retry from scratch.
3. Before accepting an intent or exposing its receipt, the host marks the
   original launch slot waiting via `mark_slot_waiting` (or the equivalent
   durable flag under the shared slot lock), disabling final-result discounting
   throughout `park_requested`. Failure to retain that reservation refuses
   acceptance and escalates; no accepted park may have a discountable live slot.
   The current best-effort helper needs a checked persistence/readback result
   for this handshake; invoking its void-returning API alone is insufficient.
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
launching the vendor, and holds its guard through vendor exit/reaping; it never
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
not keep it parked/reject it, and `evaluate_unpark == Ready` for dependencies and
other holds, plus all authority/seat/posture checks above. Do not call
`maybe_autoqueue` or require its Approved-only predicate. A resolution changing
status to Approved or any other status blocks bound resume with a visible
status-mismatch recovery reason; it never silently resets that status. Lost or
changed bindings, terminal status and independent holds also refuse. Clearing
such a block requires authorized recovery and revalidation, not an answer alone.
A changed seat/HEAD requires explicit recovery rather than silently discarding
edits or switching sessions.

Prompt data contains the bound question, selected answer, consequence and the
complete free-text `DecisionRequest.note` (including an explicit absent note), with
their provenance and wait ID. Pass it as an argv element or existing supported
stdin transport, never shell interpolation. Launch the exact session in the
retained seat. The first successful resumed receipt acknowledges the wait;
subsequent questions use new wait IDs. The host then follows normal phase and
publication rules for the resumed child.

Compare/update under the wait lock and a single attempt ID prevent duplicate
answer events from spawning duplicate children. Persist launch reservation before
spawn and the child receipt immediately afterward. An ambiguous crash between
spawn and receipt becomes `delivery_unknown`, consumes capacity until reconciled,
and requires host process evidence or operator recovery; do not blindly replay
an answer. Failed admission stays visibly blocked and retryable without changing
the durable answer. Missing seats or transcripts become escalations, not fresh
sessions. Cancellation retains history and requires explicit ownership release.

## Vendor contract and initial scope

| Vendor | Headless resume argv shape | Admission proposal |
|---|---|---|
| Claude | `claude -p --resume <session_id> <answer_prompt>` | Legacy contained headless path only, with retained verified transcript as specified below; shared-wrapper private-state resume is excluded. |
| Codex | `codex exec resume <session_id> <answer_prompt>` | Initially only paths without AIDA containment that already admit resume, including trusted native workspace-write posture where admitted. Native is reported as native sandbox, not AIDA-contained. |
| AGY | `agy --conversation <session_id> --print=<answer_prompt>` | Implement the verified headless adapter and validate its normal launch admission before enabling park; no blanket removal of the existing refusal. |

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

Recorded fake-harness tests must exercise one resume per vendor with exact ID,
answer and cwd; question persistence before exit; slot release only after exit;
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

Review decision requested: approve the durable host-owned park/outbox protocol,
exclusive retained seat with a released running slot, and the initial admission
limits above, including manual-only initial scope (AC2 open), observer-only
resume-watch, per-child slot reservations, bound seat transport, explicit overnight authority
recovery, canonical schema additions and bounded supervisor stop policy. No implementation
should start until the advisor records signoff.
