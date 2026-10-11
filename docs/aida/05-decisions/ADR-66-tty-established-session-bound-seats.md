# TTY-established, session-bound seats with scoped child grants

Status: Accepted 2026-10-04 — the decision is validated by STORY-1473's
implementation: PR #2418, independent full-acceptance review APPROVED at
1f1aa6fd, merged at b602a5d. Originally recorded 2026-10-03 as Joe's policy
choice with an advisor-approved implementation sketch, and held as Draft until
that implementation landed.

Trace: STORY-1473.

## Context

The previous team roster assigned a role to a person, and `AIDA_SESSION_ROLE`
could declare a different role in any process running as that person. AIDA's
role separation was therefore shell/process-wide instead of session-bound.
This weakened the product, advisor, implementer, and reviewer boundaries.

STORY-1473 required a TTY-established session seat, a roster ceiling,
human-at-TTY roster writes, consistent CLI/MCP authority, and documentation
of the residual same-user token-replay limit. Its implementation was guided
and required advisor signoff on a sketch before code.

## Decision

Joe selected **TTY grant + scoped child seat**:

- Joe explicitly chooses a roster-allowed seat at an interactive TTY.
- The roster limits eligible seats; it is not itself the active session seat.
- AIDA launchers may issue each child only the explicitly requested,
  roster-allowed seat for that child session.
- Joe further selected explicit per-session delegation: a parent may issue
  only child seats listed in its TTY-issued delegation scope. Roster membership
  alone does not let an arbitrary launcher grant another seat.
- `AIDA_SESSION_ROLE` is a display/routing hint and cannot grant authority.
- Roster writes require a human at an interactive TTY.

This preserves advisor agents while separating their sessions. It does not
claim an OS-level security boundary against a same-user process that can read
another process's state. A same-user process can still copy a grant handle or
replay a drain token; STORY-1473 does not prevent that or claim protection from
it. The protected broker follow-up remains separately tracked by BUG-1669,
deferred pending its operator authorization gate; SPIKE-91 records the design
package. No protected broker is deployed.

## Implementation gate (satisfied)

The architecture sketch is
[`2026-10-03-story-1473-advisor-seat-issuance-sketch.md`](../../plans/2026-10-03-story-1473-advisor-seat-issuance-sketch.md).
The linked sketch's roster representation, grant validation and lifetime,
recursive delegation defaults, and CLI/MCP issuance inventory received
independent advisor signoff on 2026-10-03. It records legacy
scalar-to-singleton roster reads, TTY-selected empty-by-default delegation,
and revocable opaque grant handles. Implementation proceeded under STORY-1473's
guided execution mode and completed with PR #2418.

Implementation detail: grants expire after 24 hours at most. Child grants share
their parent's expiry and become invalid when the parent is revoked.

## Amendments

**Per-call validation (TASK-1594 confirmation C, 2026-10-04).** The sketch said
MCP "captures the grant at startup"; the shipped implementation re-validates
the grant on every call, which is strictly tighter — revocation applies
immediately. This ADR records the per-call behavior as the decision's
implemented form. A running server still cannot acquire a *different* grant:
changing its authority means entering the seat at a TTY and restarting or
reconnecting the server.

**Reasoned-hold human-at-TTY carve-out (operator decision, Joe, 2026-10-04;
TASK-1594 confirmation D).** Hold authority (BUG-1793) deliberately retains a
bare human-at-TTY authority leg that this ADR removed from advisor authority.
This is a recorded exception, not drift: holds are the supervision floor and
emergency brake — their purpose is to reach a human, so brake operations must
remain reachable by a human at an interactive terminal even when grant issuance
is broken or misconfigured. Fail-closed applies to dispatch; the brake fails
open to the human. The carve-out covers ONLY the interactive TTY leg; the
orchestrated leg routes through the validated grant.

## Consumers

STORY-1485 (single-occupancy orchestrator seat) binds seat occupancy and its
hard-takeover authority to this contract: occupancy follows the validated
grant's session, and hard stop requires a direct human-at-TTY, non-child,
roster-allowed grant. Non-blocking follow-ups from the acceptance review are
tracked in TASK-1594.
