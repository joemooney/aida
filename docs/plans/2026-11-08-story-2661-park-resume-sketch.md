# STORY-2661: park on ask, resume on answer

Status: proposed; advisor signoff required before implementation.
<!-- trace:STORY-2661 | ai:codex -->

The binding advisor ruling requires a sketch before code. This document is
that review artifact, not an implementation or authorization to resume.

This revision addresses review findings F1–F8 from commit `39ab419eb862`.
Publication must use a non-completing subject:
`[AI:codex] docs(plans): clarify park and resume protocol (Refs STORY-2661)`.
AC1–4 remain unimplemented; merging this sketch must not complete the story.
The native seat leaves edits uncommitted for the host to publish and must not
rewrite the earlier commit or PR itself. Advisor signoff is still outstanding.

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
Existing legacy Claude admission must be assessed separately.

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

1. An explicit blocking lane ask creates the canonical question and a durable
   park intent; do not infer blocking asks from arbitrary model prose. The CLI
   returns a stop-now receipt and the lane ends its current turn.
2. The host binds and accepts the intent before publication, phase completion,
   ordinary background-work resume, or lease cleanup. A park is neither a
   successful implementation nor a failed phase to retry from scratch.
3. State advances `park_requested -> parked` only after the host verifies the
   vendor child has exited and transcript identity is retained. Until exit,
   its running slot remains occupied. No process is killed by the ask command.
4. Failure to persist either side refuses the handshake with recovery guidance;
   a question alone does not release a live slot. A lane that fails to stop
   becomes `stop_timeout` if its managed vendor process group has not exited
   within 60 seconds of durable park acceptance, or if it starts another model
   turn after the stop receipt (whichever comes first). The supervising launcher
   owns a monotonic deadline: at expiry it stops its verified child process group,
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
hands durable ownership to it before accepting an automatically resumable park.
The service holds one project-scoped singleton lock, scans waits at startup and
at most every 5 seconds, and checks canonical store state without model polling.
It never needs the exited launcher's PID to remain alive. Existing store sync
makes remote answers visible; the service does not invent a new remote sync or
fetch authority. `aida awaiting` reports last scan, observed store revision and
sync errors so an unsynced remote answer remains visibly awaiting local evidence.

Park admission requires an observed service heartbeat younger than 15 seconds
and a durable acknowledgement of the wait registration. Without this service,
retain the ordinary question/manual escalation path and refuse automatic park.
The service restarts and reconciles from durable records; if the machine or user
manager is down no progress is promised. `aida awaiting` detects a stale heartbeat
and names the service restart command; restored service scans all retained waits.
The service itself consumes no model concurrency slot. Its child supervisor
retains running-slot accounting until each launched child exits, even if the
watch process restarts; ambiguous receipts retain capacity.

The consumer reconciles answered waits after restart, including
an answer arriving before the original child exits. It acquires an ordinary
running slot and the seat lock before `resume_ready -> resuming`. It rechecks
current grant/parent validity, role, the resume predicate below, holds/punts,
seat identity, branch/HEAD, vendor transcript availability, and the original containment posture.
The answer is data, never approval or grant renewal.

### Parent grant and overnight recovery

The consumer may mint a child only using its own currently validated delegating
parent seat grant, through `issue_launch_child` / `issue_child_for`. The original
implementer's grant is not a delegator and is never resurrected. A service launched
under an authorized integrator/advisor delegator can use that live parent only
while its subject, roster, process binding, delegation scope and expiry validate.
An ordinary systemd restart has no such authority by default. Do not copy a
terminal token into a unit file or persist reusable tokens in wait records.

Thus an overnight answer with no valid consumer parent becomes
`blocked_authority`, retaining the answer and seat. `aida awaiting` must show
this even when no consumer is running, plus the explicit recovery: an authorized
operator enters the permitted delegating role via `aida role enter <role>` in a
live host shell, then runs the proposed `aida lane resume <wait-id>` command.
That command claims the wait under the same per-wait lock/attempt protocol and
supervises the resumed child under that caller's fresh parent grant. It does not
renew the exited session or grant the daemon permanent authority. The watcher
can continue observing without launch authority. Unattended overnight resume
requires a separately approved service-delegation lifecycle; this sketch does
not introduce one. Advisor acceptance of this manual authority recovery limit
is required before claiming the initial automatic-resume scope.

### Status and resume predicate

Parking preserves `InProgress`; it does not use NeedsAttention, Approved, queue
completion or an ordinary failed-phase shelf as a surrogate for a human wait.
The runtime wait blocks phase advancement and generic queue pickup. Resume is
allowed only for the original InProgress spec with its exact active binding,
a verified exited child, an unchanged answered question, a resolution that does
not keep it parked/reject it, and `evaluate_unpark == Ready` for dependencies and
other holds, plus all authority/seat/posture checks above. Do not call
`maybe_autoqueue` or require its Approved-only predicate. A resolution changing
status to Approved or any other status blocks automatic resume with a visible
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
| Claude | `claude -p --resume <session_id> <answer_prompt>` | Use existing admitted resume posture; shared-wrapper contained resume stays refused until private-state lifecycle support exists. |
| Codex | `codex exec resume <session_id> <answer_prompt>` | Initially only paths without AIDA containment that already admit resume, including trusted native workspace-write posture where admitted. Native is reported as native sandbox, not AIDA-contained. |
| AGY | `agy --conversation <session_id> --print=<answer_prompt>` | Implement the verified headless adapter and validate its normal launch admission before enabling park; no blanket removal of the existing refusal. |

These are identity/prompt shapes; approved model, permission, environment and
output flags still come from the vendor registry and shared launch composers.
Never drop inherited containment to make a resume work. In particular, contained
Codex cannot enter an automatically resumable park in the initial slice. Refuse
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
grant; consumer restart with no grant and fresh authorized manual resume;
remote-node answer synced after park; contained answerer without runtime access;
lost wake hint; stopped/stale service and registration refusal; sync errors;
legacy question migration; forced overwrite before/after answer; synced binding
replacement; free-text note such as “1, but name it X” in each vendor prompt;
InProgress resume versus Approved/status-mismatch refusal; cooperative one-turn
exit and 60-second stop timeout with surviving/ambiguous descendants;
hold/punt vetoes; rejected/kept-parked answers; and containment refusal
before artifacts or child spawn. PATH changes use the existing isolated-test
helper. Host integration must demonstrate the child recalls both question and
answer, with no generic queue duplicate, and no publication while parked.

Review decision requested: approve the durable host-owned park/outbox protocol,
exclusive retained seat with a released running slot, and the initial admission
limits above, including the new resume-watch service, explicit overnight authority
recovery, canonical schema additions and bounded supervisor stop policy. No implementation
should start until the advisor records signoff.
