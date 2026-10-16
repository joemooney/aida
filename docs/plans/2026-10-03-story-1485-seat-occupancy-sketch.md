# STORY-1485: single occupancy and takeover sketch

Status: revised 2026-10-04; proposed, awaiting independent advisor rereview.
Sketch only. No production implementation is authorized by this document.
<!-- trace:STORY-1485 | ai:codex -->

Review source: the 2026-10-03 independent advisor CHANGES REQUESTED comment on
STORY-1485, reviewing `edeac57b63`. The five sections below answer its findings.
Operator takeover acceptance remains the 2026-09-26 scope-addition comment.

## 1. Commands, configuration, and dispatch inventory

These are proposed interfaces, not commands already available. Add
`aida session seat` with the following subcommands:

| Command | Contract |
| --- | --- |
| `claim --seat orchestrator` | Acquire an empty/provably dead seat; same-holder claim is idempotent; a competing live holder refuses. |
| `status --seat orchestrator [--json]` | Inspect holder session, harness session, PID/start identity, since, generation, liveness, request and reservations. No claim. |
| `release --seat orchestrator --generation N [--request UUID]` | Current holder only; stop new dispatch and drain reservations before release. A pending takeover requires its matching request ID. |
| `takeover --seat orchestrator --force [--kill] [--grace-secs N]` | Request graceful transfer; `--kill` adds the restricted human hard-stop path after grace. No force-less takeover. |
| `ack --seat orchestrator --request UUID --generation N --safe-to-stop` | Current holder acknowledges the named pending request after writing handoff and settling dispatch commitments. |

`claim --force [--kill]` delegates to the same takeover operation; `--kill`
without `--force` is invalid. An interrupted requester uses
`takeover --seat orchestrator --force --request UUID` to resume its own
request; it cannot resume another session's request. There is one protocol,
not separate claim and takeover implementations.

Keep `aida session handoff --seat orchestrator --show|--write PATH`.
Inspection warns about a competing live/unknown holder without acquiring it.
Add `--strict` to inspection to refuse instead. Writes require ownership;
there is no warning-only write mode. Refusals name session, harness, PID,
since, and supported stop/attach commands, never a guessed vendor command.

Proposed project config in `.aida/config.toml`:

```toml
[seat_occupancy]
single_seats = ["orchestrator"]
takeover_grace_secs = 120
inspection = "warn" # "warn" or "refuse"
```

Validate names and positive, bounded grace values (1..3600 seconds). A CLI
grace override wins over config. Orchestrator occupancy cannot be disabled
by this config; `single_seats` adds other canonical roles. Additional roles
use the same ownership checks at their role mutation boundaries; roles with
no mapped boundary refuse activation until mapped. No config value grants
kill authority. Integrator drain locking and spec/triage leases remain
independent and retain their existing meaning.

Use a shared CLI/MCP mutation gateway at the following boundaries. Symbol
names identify the source inventory inspected at merged main `b602a5d9ca`;
implementation must refresh the inventory against its actual base.

| Surface | Required ownership boundary |
| --- | --- |
| `burndown run` | `handle_burndown_run` and `run_burndown_verbose` in `lib.rs`: claim before loop activation; reserve each wave enqueue/child dispatch. Planning/status are inspection. |
| `queue work --auto-complete`, resume-drain, queue advance, `aida do` dispatch | `queue_cmd.rs` auto-complete setup and `lib.rs` phase driver: claim before run marker/loop starts, reserve each phase launch/routing commitment, including resumed implementers/reviewers. |
| Child agents and sessions | `dispatch_agent_new`, `agent_new_bg_dispatch`, vendor launch helpers in `lib.rs`; headless/resume/reviewer spawn helpers in `session.rs`; session start with launch: resolve caller and reserve orchestrator-origin spawn before starting the child. Propagate a distinct child grant and recorded run provenance. |
| Auto-enqueue and queue routing | `queue_spec_entry_for_role`, findings promotion/enqueue, `add --queue`, edit/promote queue options, review/rework auto-routing, scheduler/orchestrator direct `Storage::queue_add` calls; `queue_cmd.rs` add/rework/move/remove/clear/advance mutations: orchestrator-origin writes reserve the mutation, even when they bypass a CLI wrapper. |
| MCP queue mutation | `mcp.rs` `tool_queue_add`, `tool_queue_rework`, `tool_queue_move`, `tool_queue_remove`, and any mutating portion of `tool_queue_done`: same shared grant/provenance gateway and reservation. `tool_queue_work` currently only reports a pickup target; do not mistake that read for a spawn. Queue list/next/progress stay inspection. |
| Seat handoff and lifecycle | `session_handoff_seat`/`seat_rotation.rs`: fenced handoff write; seat claim/release/ack/takeover: serialized state transitions. |

Classify by operation AND authoritative caller binding, never a caller's
optional `--seat` or role label. Every orchestrator loop start requires
occupancy regardless of its caller's product/advisor label. Persist a process
binding when the loop/managed harness starts: canonical repo, grant session
ID, scope-lease ID, run ID, harness identity, PID/start identity, and occupancy
generation. These session IDs are distinct: `SeatGrant.session_id` is the
authority session; the worktree lease ID is a scope reference, not proof of
seat authority. Descendant dispatch resolves the registered process/run
lineage and validated grant; an MCP server captures its caller binding at
startup and revalidates it on mutation.

If the binding marks the caller as orchestrator-origin, its writes/spawns
remain fenced after it drops its env labels, omits `--seat`, changes its
claimed role, or ends its grant. Missing/contradictory process identity or
binding refuses protected dispatch; it must not downgrade to ordinary work.
Keep a demotion tombstone for that live process/run lineage until it is
provably dead. A former holder cannot claim a fresh generation implicitly.
Unbound control-loop starts must explicitly claim, and unbound protected
routing/spawn requires the existing validated dispatch grant. A separately
registered implementer may edit/test its own assigned scope without claiming
the orchestrator seat; delegated workers are not allowed to originate a new
loop or route/spawn peers using their parent's occupancy.

Search all process-spawn, queue-write and authority sites during
implementation and add discovered paths to this table. Unknown managed
control paths fail closed. Same-user malicious record/grant forgery remains
the explicit ADR-66 limitation, not a security property supplied by lineage.

### C7 inventory refresh and accepted core refinements

This addendum records the advisor's 2026-10-07 C7 changes-requested decision
and the operator's mechanical rework authorization. It does not authorize
steps 4–5 gateway activation. The original signed-off sketch and its commits
remain preserved. Core regressions exercise runtime permits using fixture
repos, persisted local review relationships and test-owned processes.
<!-- trace:TASK-1607 | ai:codex -->

| Caller/path | Actual operations and disposition |
| --- | --- |
| Implementer child: `queue_cmd::handle_queue_done`, status/comment edits | Route the assigned spec; no claim or holder permit. Persisted spawn identity supplies the assigned spec, with a process/start-validated own lease as fallback. |
| Implementer session end: `try_auto_queue_pr_review`, `aida_subcmd_add_review_story`, `aida_subcmd_rel_add_implements`, `aida_subcmd_queue_add_for_reviewer` | File the review story, persist its `implements` edges, then route that story to reviewer. Forge discovery and store push occur outside the seat lock. The new story is within scope only after its local persisted relationship links the assigned spec. |
| Reviewer child: `handle_review_record_at`, `queue_cmd::handle_queue_rework`, reviewer queue completion | PR/MR lease or assigned review story covers that persisted story and its direct `implements` specs. Verdict recording and rework routing of those specs pass; unrelated specs do not. Forge labels are distinct; title prose/trailers alone confer no scope. |
| Punt/advice child: `handle_punt_command`, headless advisor phase and implementer resume | Local punt/status/comment writes and routing the assigned spec to advisor or back for rework pass. The child cannot spawn its own advisor/reviewer peer or originate a loop; the holder dispatches the required phase. |
| `maintenance_schedule::tick` / `tick_core`, including systemd/cron `aida schedule tick` → scheduled `shift tick` | Unattended control-path candidate, usually no validated grant. Maintenance/seat due delivery stays classified separately. A timer must not originate an orchestrator loop; diagnose/refuse that launch instead of inventing authority or implicitly claiming from an environment label. No timer/user service is a test target. |
| `supervisor::handle_supervise_command` → `launch_redrive` (`queue work --auto-complete --force-claim --no-human both`) | This is a LoopStart, not ordinary spec completion. The approved disposition is to hand the requeued spec to the live holder's loop, never originate a competing loop. A missing/unknown holder or invalid caller authority must diagnose/refuse; no fallback launcher. |

Child review scope is a bounded local snapshot: assigned spec, its persisted
review story, and that review's direct PR/MR specs. Never transitively walk
other PRs through shared specs. Resolve the caller against committed child
PID/start identity even after reparenting/holder exit; do not borrow a parent
spawn record or a parent lease. Unknown/ambiguous evidence adds no scope.
The core exposes `authorize_as_scoped` for this snapshot; wiring production
callers to it remains the separate post-C7 checkpoint.

ADR-68 refinement requested at C7: grant-valid plain-shell one-shot Route/Spawn
passes without installing a holder. LoopStart, harness anchors and loop
anchors still claim implicitly. Persisted holder, demotion and child lineage
are checked first and remain fenced. The refinement comment is retained in
`.aida/handoff/task1607-rework/adr68-refinement.md` if the canonical store is
unwritable; it must be reconciled to ADR-68 when store writes are available.

ADR-70 remains local-only: obtain any graph/forge/network data before taking
the permanent seat flock; validate and commit only bounded local mutations or
spawn/identity recording under it, then unlock before push/sync/gh/glab/fetch,
mail delivery, or waiting. Seat-lock acquisition itself now times out with a
diagnosis after ten seconds; a stalled commitment does not block every caller
indefinitely. Seat policy and state share the main-worktree root. C1–C8,
demotion/spoof/unknown refusals and disabled hard takeover continue to apply.

## 2. Atomic commitment, transfers, and recovery

Store a versioned record under the main worktree's
`.aida/seat-occupancy/<seat>.json`, guarded by a permanent `<seat>.lock`
sidecar and atomic replacement. Include holder authority session/grant ID,
scope lease, harness session/PID/start identity, since, monotonic generation,
state, pending request, dispatch reservations, and event outbox. Sibling
worktrees resolve the same main root. Malformed/unreadable state fails closed;
never unlink lock sidecars. Generation increases on every acquisition and
transfer, including dead-holder replacement.

Choose generation-bound dispatch reservations. Under the seat lock validate
the grant, process binding, current holder and generation, then persist a
reservation with operation UUID, scope, executor PID/start identity and
intended mutation/child. That durable reservation is the commitment boundary.
Its state is reserved, committing, committed or cancelled. A pending takeover
stops new reservations. A reservation can execute only after transitioning to
committing under the same lock; it records its final result/child identity
before clearing. Queue/handoff publication uses atomic replacement and
operation UUID receipts to make recovery idempotent. Handoff publication and
its reservation receipt are serialized under the seat lock; no external
process wait is involved. Queue writers retain their own lock; acquire seat
before queue and never invert that order.

A takeover/release cannot transfer ownership while any reservation is
reserved or committing. Already committed children are listed as in-flight
work, not silently killed or re-dispatched; their assigned work may continue.
The holder may cancel unstarted reservations or finish commitments and then
ack. Thus a late old-generation write/spawn is either rejected before
reservation or finishes while the old holder still owns the seat; it cannot
commit after transfer. Do not expire reservations by age alone. If an executor
dies, reconcile queue receipts or registered child identity; if the outcome
cannot be proved, mark Unknown and block transfer rather than retry a spawn.

Pending request contains UUID, requester validated session/grant and process
identity, expected holder session/generation, requested mode, created time,
absolute deadline, and state (requested, acknowledged, stopping, transferred,
timed-out/cancelled). Persist it before sending notification; resend is
idempotent. Only one request per generation is active; another requester
gets the existing request identity, not a competing transfer. Resumption
uses the same request and original deadline, never resets grace silently.

Release and transfer use compare-and-swap semantics under the lock. Plain
release leaves the seat empty and increments generation; release with a
pending request marks that request acknowledged and transfers to its still
valid requester in the same locked transition. An invalid/dead requester
cancels the request and permits release to empty. An ack transfers only when
requester validation and zero unsettled reservations hold. Racing requests,
old release/ack calls and retries cannot overwrite a newer holder. Timeouts
cancel only their matching request; the old holder retains its generation.
If a process dies during hard stop, recovery stays in stopping, reprobes
identity/reservations and either transfers once or reports Unknown. Never
hold a lock across mailbox delivery, peer waiting, harness termination or
child completion. Event-outbox records live in the atomic state transition;
projection retries use operation UUIDs to avoid duplicate history entries.

Project request, acknowledgment, release, timeout, stop outcome and transfer
events into the existing ledger/event feed and `aida history`. Each records
from/to sessions, mode, request UUID, generations, in-flight scope/child IDs
and outcome; dead-holder takeover is silent at the CLI but remains auditable.

## 3. Notification is not acknowledgment authority

Use the existing durable mailbox only to notify: stop new dispatch, settle
reservations, write a handoff and call the seat ack command. Its message
carries request UUID, holder/requester sessions, expected generation and
in-flight scopes. Freeform messages, sender names, read receipts and mail
intent are not acknowledgments and never transfer ownership.

`ack` and `release` themselves invoke the shipped grant resolver, validate
principal/subject/expiry/revocation/roster and process binding, then compare
the validated grant session to the holder plus request UUID and generation
under the seat lock. A caller-supplied holder/session field cannot replace
that comparison. The holder writes the handoff through the fenced operation
before ack. A scoped child, different session or demoted holder cannot ack
on behalf of the holder. Invalid acknowledgments have no transfer effect.

## 4. Liveness and hard-stop targeting

Introduce an explicit probe result, preserving diagnostics:

| Result | Evidence and consequences |
| --- | --- |
| Alive | Positive OS process query with matching captured start identity and, for managed/background harnesses, validated harness-session-to-process mapping. Protect holder; graceful ack may transfer. |
| Dead | Positive OS evidence that the recorded process no longer exists, or a different start identity proves the old process ended; background session mapping must also establish that no replacement daemon still drives the holder. Permit silent takeover only after reservation reconciliation. Never signal a reused PID. |
| Unknown | Missing start identity, permission/query error, inaccessible proc data, unverified parent/daemon mapping, unsupported platform, or inconsistent records. Refuse mutation/takeover/kill and show diagnosis. |

Do not reuse the boolean answer from `process_identity_is_alive` or
`pid_is_alive` as proof: missing identity is not Alive evidence and a query
error is not Dead evidence. Capture process start identity at claim/launch;
if that cannot be established, do not activate production occupancy.

For Claude background sessions, resolve the recorded Claude session through
the harness's supported session/daemon inventory to its current `claude
daemon run` process, match the recorded daemon start identity and repo/run
binding, and confirm `claude stop <harness-session-id>` targets exactly that
holder. `claude attach <id>` is displayed only for a verified supported
mapping. No arbitrary session string becomes a command. An unsupported
inventory or ambiguous daemon/session correspondence is Unknown, not a
fallback to killing a parent shell or guessed PID.

Other harnesses use their registered exact process identity and a supported
stop adapter. Prefer an identity-bound process handle (e.g. pidfd where
available); if the platform cannot maintain identity through signaling,
refuse hard stop instead of relying on a racy bare PID signal. After grace,
revalidate authority, request, holder generation and target identity; mark
stopping to block concurrent claims, unlock, stop the exact target, verify
termination, then reconcile reservations and CAS transfer. Stop failure or
Unknown leaves ownership fenced with explicit recovery state. Never transfer
merely because a stop command exited zero. Scan holder-owned leased worktrees
for tracked/untracked dirty work and report paths before/after stop, never
reset/remove them. Permission errors in scans are reported explicitly.

## 5. Shipped authority contract and bounded activation

STORY-1473 is merged, not a pending prerequisite: PR #2418, approved verdict
at `1f1aa6fd`, merge `b602a5d9ca`. Reuse ADR-66 and
`aida-cli-lib/src/seat_authority.rs`: `current_grant`/`current_seat`, direct
TTY issuance, distinct child grants, explicit empty-by-default delegation,
expiry/revocation and roster ceilings. CLI and MCP resolve the same grant;
role env labels and scope-lease possession confer no authority. Roster
writes remain human-at-TTY only. Bind occupancy to the validated grant's
session, not `AIDA_SESSION_ROLE` or arbitrary `AIDA_SESSION_ID` values.

Hard stop uses only the already accepted direct human-at-TTY route. Require
an interactive direct CLI invocation with a valid direct TTY-issued,
roster-allowed dispatch seat grant (no child/managed-agent grant), the shipped
direct-TTY/managed-agent checks, and a target-specific human confirmation
naming holder, generation and in-flight scopes. Repeat grant/TTY validation
after grace immediately before stopping; changed target requires fresh
confirmation. MCP, headless drains and managed agents always refuse
`--kill`, even with an inherited grant or allocated PTY. A TTY alone or a
role label alone is insufficient. Graceful takeover remains available to
an authorized session without kill permission.

No trusted-kill configuration is introduced. Repo/home config and env flags
cannot authorize peer killing, bypass a human confirmation, or widen a
child grant. The operator-config alternative in acceptance remains disabled
unless Joe separately chooses a trusted boundary; the human-at-TTY alternative
can satisfy authorized hard takeover without inventing that boundary.

ADR-66 explicitly concedes same-UID grant/record replay and has no deployed
protected broker. This design preserves that boundary: it guards supported
AIDA operations and does not claim to stop a malicious same-user process
from forging records, scrubbing agent identity, or directly signaling peers.
If a harness/platform cannot establish direct human invocation with the
shipped contract, the hard-stop adapter fails closed. Disabled-kill adapters
may be delivered as bounded scaffolding, but do not satisfy authorized hard
takeover acceptance and cannot justify completing the whole story. Production
activation requires validated grants, caller binding and tri-state probes;
there is no compatibility fallback to env-only authority.

## Validation plan and rereview request

No tests run for this sketch-only change. Implementation acceptance must
cover: concurrent sibling claims; idempotent current-holder claims; live
refusal/inspection warning; graceful ack and release; silent timeout with no
kill; interrupted/resumed requests; stale/spoofed/mail-only ack; generation
fencing after env/seat omission; reservations racing transfer and process
crashes; PID reuse/missing identity/probe errors; ambiguous Claude mapping;
MCP/headless/managed-agent kill refusal; direct human authorized stop;
dirty-worktree preservation/reporting; stop failure; silent provably dead
holder replacement; and idempotent history projection. Use controlled test
processes and injected deadlines, never existing peer sessions.

Advisor rereview requested for the concrete command/config inventory,
reservation protocol, validated ack, tri-state target probes, and reuse of
shipped ADR-66 authority. This document records proposals, not signoff, and
no production code or shipping is part of this rework.
