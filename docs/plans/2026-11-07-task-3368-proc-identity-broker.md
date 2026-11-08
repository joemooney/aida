# TASK-3368: broker-attested process observations

<!-- trace:TASK-3368 | ai:codex -->

Status: approved for implementation (observation-only), per the advisor signed
decisions recorded on TASK-3368 at 2026-11-07 17:05 MST. Implementation remains
owed under this task; this documentation slice must use `(Refs TASK-3368)`.
This is the separately reviewed S6 sketch for STORY-2537. It grants no launch,
containment, role, publication, or human-terminal authority. Approval of the
parent sketch does not approve this slice.

## Approach

Introduce a per-user, host-side observer that can attest process observations
in its own verified PID coordinates. Reuse the shared ProcIdentity judge and
retain Alive / Dead / ForeignUnknown and holder refusal as separate outcomes.
An authenticated peer is a process, not automatically an authorized AIDA seat.
The broker checks existing grants and project scope independently. Same-UID
malicious host processes and a compromised host remain outside this trust
boundary; a socket permission check cannot isolate mutually hostile same-UID
clients. No new protection against the operator is claimed.

The initial implementation should expose observation only. Scoped observation
handles identify the requester and allowed subject; they are not bearer role
grants. Grant replacement, arbitrary cross-namespace translation, signaling,
and changes to ship/train/hold-clear human-terminal rules require subsequent
explicit approval. Do not enable those features through an observation flag.

## Reusable helpers and current evidence

- `aida-core/src/proc_identity.rs` owns ProcIdentity, observer admission, the
  shared judge and cooperative holder admission. Capture alone supplies no
  holder authority. Preserve raw start ticks, boot qualification and the PID
  coordinate namespace; no wall-clock conversion participates in equality.
- `aida-cli-lib/src/deputy_shell/socket.rs` supplies a precedent for same-UID
  SO_PEERCRED plus SO_PEERPIDFD authentication, bounded framing and deadlines.
  Its socket authenticates deputy requests, not a generic identity service.
  Extract reviewed transport helpers rather than extending the deputy job
  protocol or silently reusing deputy grants for observation.
- `aida-cli-lib/src/gate/seat_authority.rs` remains the grant/policy boundary.
  `aida-cli-lib/src/ownership/judge.rs` and
  `aida-cli-lib/src/leases.rs` retain their lifecycle and generation policies.
  No broker receipt bypasses their phase, scope, tree or release checks.

## Connection and observation protocol

Version-1 AF_UNIX SOCK_SEQPACKET service resides in a host-owned private runtime
directory (0700), socket mode 0600. Validate ownership, non-symlink directory
components and socket identity before connection; refuse unsafe or replaced
paths. One explicitly managed broker instance holds an exclusive runtime lock.
Start the broker explicitly through an `aida <service> run`-style command,
optionally managed by a systemd user unit. Never auto-spawn it from a client or
seat. Version 1 serves host processes only: contained seats get no route to the
socket. Any narrow sandbox transport requires a separate spec and sketch. No
sandbox admission or vendor containment claim follows from this sketch.

At broker startup, probe SO_PEERPIDFD and SO_PASSPIDFD/SCM_PIDFD support
(Linux >= 6.5); refuse to start if unavailable. Enable SO_PASSPIDFD and
SO_PASSCRED so the kernel attaches SCM_PIDFD and SCM_CREDENTIALS to every
message. Use one request per message and authenticate each message from its
own kernel-attached pidfd, never from the connection's SO_PEERCRED or
SO_PEERPIDFD. Refuse messages without the required kernel credentials;
caller-supplied SCM_CREDENTIALS PIDs alone are not trusted. Do not substitute
pidfd_open on an untrusted numeric PID.

Require same-UID sender credentials. Capture the sender in the broker's proven
procfs coordinates, check the pinned process before and after validation, and
reject sender exit, PID reuse, incomplete procfs or unavailable namespace
evidence. Credential PID and target innermost namespace must never be combined
into one tuple. Descriptor transfer grants no authority: the authenticated
party is the sender of each message, established by that message's kernel pidfd.

A request contains protocol version, unique nonce, project/store binding,
operation, subject ProcIdentity, holder generation and the existing grant
reference where policy requires one. No caller-provided UID, node, boot,
namespace, PID or environment value is accepted as peer authentication.
Permit self-observation first; observing another subject requires an explicit
existing scoped authorization. Never offer arbitrary per-user process lookup
or treat ancestry alone as authority.

Proposed limits: 64 KiB per frame, five seconds total request deadline,
16 concurrent requests, bounded ancestry where existing grant checks need it,
and a 30-second maximum client wait. Refuse oversize, unknown versions,
unknown fields, duplicate fields, invalid tuples and unsupported operations.
Return structured refusal codes without leaking another project's process or
grant details. Busy and timeout are unavailable evidence, not Dead.

The response binds version, broker incarnation, enrolled local node, boot,
requester identity, subject identity, project/scope, holder generation, nonce,
verdict/reason and broker-local observation sequence. Receipts are consumed
on the authenticated channel; they are not portable signed credentials.
Callers cannot submit a receipt file as authority. If later persistence needs
cryptographic authentication, its key custody and verification protocol need
a new review rather than trusting editable same-user JSON.

## Namespace and holder rules

The broker observes subjects only in its verified coordinate system. It may
report a broker-coordinate identity to the authenticated process, but must
not label it as the process's inner-namespace identity. A request using child,
sibling or unverified coordinates returns ForeignUnknown: namespace mismatch.
NSpid strings and matching namespace inode numbers alone do not authorize
translation. Initial S6 provides no setns, privileged helper or remote broker.
Trusted reboot evidence is local-node evidence only.

A broker cannot remotely query an arbitrary target's subreaper flag. Existing
cooperative holder admission remains mandatory; kernel authentication does
not certify non-subreaper status or eligibility as a work holder. Unknown or
invalid admission retains work and reports the refusal. Ongoing holder-state
attestation would need its own feasible cooperative protocol and review.

## Replay, revocation and failure policy

Observation handles are broker-resident opaque random identifiers, bound to
requester identity, project, subject, generation, operation and the validated
grant's effective expiry including parent delegation. Handle possession alone
is insufficient. Revalidate grant and binding at each operation; no scope
widening on renewal. Serialize revocation and authorization at one operation
boundary so revocation acknowledged before admission prevents the operation.
An already returned observation is historical evidence, never an action lease.
Reject reused request nonces within a bounded session; when replay state fills,
close the session rather than evict entries while accepting old nonces.

Restart changes the unpredictable broker incarnation and invalidates every
handle; reconnect and reauthorize. Reboot invalidates boot-bound observations.
Disconnect, process replacement, revoked/expired parent, changed generation,
and scope changes invalidate handles. Bounded lifetime uses broker-local
monotonic time, not client wall time; restart does not restore deadlines from
unverified persisted clock domains. No offline authorization cache is accepted.

A receipt of Alive establishes only that observation at its sequence. Reclaim,
role binding and mail still require fresh evidence under their existing locked
mutation boundaries. Signaling remains out of scope and retains pidfd safety.
Unavailable broker, malformed response or socket replacement yields explicit
unavailable/ForeignUnknown or an operation refusal; none may free work, grant
mail access, reinterpret expiry as death, or fall back to numeric PID probing.

## Files and rollout

This proposal changes only this document and spec comments. A later approved
implementation would add a dedicated broker module under
`aida-cli-lib/src/`, extract narrowly reusable socket authentication from
`aida-cli-lib/src/deputy_shell/socket.rs`, and adapt shared observation APIs in
`aida-core/src/proc_identity.rs` only where necessary. Exact new module names
and command registration belong in that implementation plan.

Ship a disabled service and protocol tests first, then an opt-in self-observation
client, then individually reviewed consumer adapters. Inventory all readers,
writers and rollback binaries before enabling persisted fields; do not append
fields to strict legacy schemas. Existing direct observation remains supported;
when broker mode is explicitly required, failure refuses rather than downgrades.
Rollback disables broker consumers and invalidates handles; it does not rewrite
legacy holders, release claims or restore revoked authority. No S6 cutover
substitutes for BUG-2458 host acceptance or STORY-2325 compare gates.

## Tests and verification plan

Future implementation requires executed tests, including positive controls:

- Authenticate a live same-UID sender; reject other UID, sender exit/reuse,
  forged request identity and absent kernel-attached message credentials;
  inherited/passed connections must authenticate the actual sender. Reject
  unsafe socket parents and socket replacement. Require a real-kernel host
  check for SO_PEERPIDFD and SCM_PIDFD per-message authentication semantics;
  mocks alone do not establish admission safety.
- Exercise initial/child/sibling PID namespaces, collisions, ancestor-mounted
  and masked procfs; valid same-coordinate observations succeed and every
  unsupported translation stays Unknown. Test local reboot versus other node.
- Verify self-only scope, wrong project, subject or generation, parent expiry,
  revocation/admission races, stale incarnation, replay, response swapping,
  malformed frames, timeout, saturation and restart. A valid request must still
  succeed after each negative fixture's cleanup.
- Preserve cooperative init/subreaper refusals and legacy Unknown. Step wall
  clocks without changing identity; suspend/restart must not extend handles.
- Prove broker outage cannot reclaim ownership, free review capacity or grant
  private-mail/role access; adapters preserve existing locking and lifecycle
  checks. No terminal-authority or environment-grant replacement is tested or
  claimed by this observation-only slice.

For this docs slice, inspect the diff, verify referenced paths, and run
`aida plan verify` without fixes. No Cargo execution is needed for prose.

## Advisor decisions and manual gate

### Signed decisions 2026-11-07

The advisor's 17:05 MST signed comment on TASK-3368 supersedes the earlier
14:00 decision where they conflict. It resolves the former sender-authentication
and sandbox-transport placeholders:

- D1–D2: accept observation-only scope and the same-UID-trusted boundary.
  Grant replacement, signaling, namespace translation and human-terminal policy
  remain out of scope and separately gated.
- D3–D4: use AF_UNIX SOCK_SEQPACKET with kernel-attached SCM_PIDFD and
  SCM_CREDENTIALS on every message, as specified above. Require and probe
  SO_PEERPIDFD and SO_PASSPIDFD/SCM_PIDFD; no numeric-PID fallback.
- D5–D7: accept no namespace translation, non-portable channel-bound receipts,
  and broker-resident handles with per-operation grant revalidation,
  monotonic lifetime and restart invalidation.
- D8: one explicitly started broker holds an exclusive runtime lock; clients
  and seats never auto-spawn it.
- D9: v1 serves host processes only. Contained seats have no socket route;
  sandbox transport needs a separate spec and sketch.
- D10: retain the rollout and test gates, including real-kernel per-message
  SCM_PIDFD checks before enabling any consumer adapter.

This docs-only PR does not complete TASK-3368. Use `(Refs TASK-3368)` in the
commit subject and PR title so merge does not award completion. Keep the task
flagged for advisor attention to route implementation slices, beginning with
**disabled service + protocol tests**. Preserve existing punt/merge-hold
publication vetoes until the authorized host/advisor resolves them; this signed
design decision is not itself merge-hold clearance.

## Critical Files

Review the authentication boundary in
`aida-cli-lib/src/deputy_shell/socket.rs`, the observation/admission separation
in `aida-core/src/proc_identity.rs`, and authorization in
`aida-cli-lib/src/gate/seat_authority.rs` before extracting any shared code.
These existing files remain unchanged in this docs slice.

## Risks + gotchas

Connection credentials describe the connector, which may differ from a later
sender using a transferred descriptor. Treating those as interchangeable is an
authority bug. Same-UID filesystem ownership cannot prove which trusted binary
created an endpoint. Runtime directory custody and the same-user threat model
must therefore be accepted explicitly. A broker-coordinate observation cannot
be reused as an inner-namespace tuple, and broker availability must never become
an implicit prerequisite for retaining unknown ownership safely.

## Followups

Record the revised sketch on the owning spec and hand off implementation and
host conformance slices under TASK-3368, starting with disabled service and
protocol tests. The signed design authorizes that scope; retain Needs Attention
for advisor routing until the handoff is resolved. Do not
file grant replacement or terminal-authority changes as implicitly approved
children of observation work.

## Related

- `docs/plans/2026-11-05-story-2537-proc-identity.md`: parent coordinate,
  admission, migration and separately signed broker contract.
- STORY-2343: kernel-authenticated deputy transport precedent.
- BUG-2458 and STORY-2325: independent host acceptance and ownership cutover.
