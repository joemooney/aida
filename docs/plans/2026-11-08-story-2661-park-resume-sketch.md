# STORY-2661: park on ask, resume on answer

Status: proposed; advisor signoff required before implementation.
<!-- trace:STORY-2661 | ai:codex -->

The binding advisor ruling requires a sketch before code. This document is
that review artifact, not an implementation or authorization to resume.

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
with a per-record lock and atomic replace. Proposed fields:

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
   remains visibly `park_requested`, with normal host failure handling.

Canonical and runtime writes cannot be one transaction. Use the immutable wait
ID on both sides; host reconciliation repairs an interrupted handshake and
reports unmatched questions/intents. Never claim a parked receipt before both
writes are durable. `aida awaiting` joins these records and shows question,
lane/spec, vendor/session ID, state, and recovery reason.

## Answer trigger and dispatch

After the existing authorized answer write succeeds, `finalize_answer` records
a resume-ready event for the matching wait generation. This is a durable outbox
handoff to the host, not a synchronous model spawn inside the answer command.
For a bound wait, suppress generic autoqueue so a second implementer cannot
race the retained lane. An answer that deliberately keeps work parked, rejects
it, or leaves another gate unsatisfied does not resume it.

The host consumes events and reconciles answered waits after restart, including
an answer arriving before the original child exits. It acquires an ordinary
running slot and the seat lock before `resume_ready -> resuming`. It rechecks
current grant/parent validity, role, spec pickability, holds/punts, seat identity,
branch/HEAD, vendor transcript availability, and the original containment posture.
It issues only an ordinarily authorized child grant; the answer is data, never
approval or grant renewal. A changed seat/HEAD requires explicit recovery rather
than silently discarding edits or switching sessions.

Prompt data contains the bound question, selected answer and consequence, with
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
grant; hold/punt vetoes; rejected/kept-parked answers; and containment refusal
before artifacts or child spawn. PATH changes use the existing isolated-test
helper. Host integration must demonstrate the child recalls both question and
answer, with no generic queue duplicate, and no publication while parked.

Review decision requested: approve the durable host-owned park/outbox protocol,
exclusive retained seat with a released running slot, and the initial admission
limits above. No implementation should start until the advisor records signoff.
