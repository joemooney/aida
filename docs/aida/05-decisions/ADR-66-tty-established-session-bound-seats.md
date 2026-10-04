# TTY-established, session-bound seats with scoped child grants

Status: Draft — Joe's policy choice and advisor-approved implementation sketch
recorded 2026-10-03. The ADR remains Draft until implementation validates it.

Trace: STORY-1473.

## Context

The current team roster assigns a role to a person, and `AIDA_SESSION_ROLE`
can declare a different role in any process running as that person. AIDA's
role separation is therefore shell/process-wide instead of session-bound.
This weakens the product, advisor, implementer, and reviewer boundaries.

STORY-1473 requires a TTY-established session seat, a roster ceiling,
human-at-TTY roster writes, consistent CLI/MCP authority, and documentation
of the residual same-user token-replay limit. Its implementation is guided
and requires advisor signoff on a sketch before code.

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

## Implementation gate

The architecture sketch is
[`2026-10-03-story-1473-advisor-seat-issuance-sketch.md`](../../plans/2026-10-03-story-1473-advisor-seat-issuance-sketch.md).
The linked sketch's roster representation, grant validation and lifetime,
recursive delegation defaults, and CLI/MCP issuance inventory received
independent advisor signoff on 2026-10-03. It records legacy
scalar-to-singleton roster reads, TTY-selected empty-by-default delegation,
revocable opaque grant handles with a 24-hour maximum lifetime, MCP startup capture, and the initial
launcher/authorization inventory. Implementation proceeds under STORY-1473's
guided execution mode; this Draft ADR alone does not change the decision.

Implementation detail: grants expire after 24 hours at most. Child grants share
their parent's expiry and become invalid when the parent is revoked.
