# TTY-established, session-bound seats with scoped child grants

Status: Accepted 2026-10-04 — the decision is validated by STORY-1473's
implementation: PR #2418, independent full-acceptance review APPROVED at
1f1aa6fd, merged at b602a5d. Originally recorded 2026-10-03 as Joe's policy
choice with an advisor-approved implementation sketch.

Trace: STORY-1473.

## Context

The current team roster assigns a role to a person, and `AIDA_SESSION_ROLE`
can declare a different role in any process running as that person. AIDA's
role separation is therefore shell/process-wide instead of session-bound.
This weakens the product, advisor, implementer, and reviewer boundaries.

STORY-1473 requires a TTY-established session seat, a roster ceiling,
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
another process's state; that remains separate protected-boundary work.

## Implementation gate (satisfied)

The architecture sketch is
[`2026-10-03-story-1473-advisor-seat-issuance-sketch.md`](../../plans/2026-10-03-story-1473-advisor-seat-issuance-sketch.md).
The linked sketch's roster representation, grant validation and lifetime,
recursive delegation defaults, and CLI/MCP issuance inventory received
independent advisor signoff on 2026-10-03. It records legacy
scalar-to-singleton roster reads, TTY-selected empty-by-default delegation,
and revocable opaque grant handles. Wording amendment (TASK-1594
confirmation C): the sketch said MCP "captures the grant at startup"; the
shipped implementation re-validates the grant on every call, which is
strictly tighter — revocation applies immediately. This ADR records the
per-call behavior as the decision's implemented form.

## Consumers

STORY-1485 (single-occupancy orchestrator seat) binds seat occupancy and its
hard-takeover authority to this contract: occupancy follows the validated
grant's session, and hard stop requires a direct human-at-TTY, non-child,
roster-allowed grant. Non-blocking follow-ups from the acceptance review are
tracked in TASK-1594.

---

> **Repository note — not part of the canonical ADR text.** The text above is
> a verbatim copy of `aida show ADR-66`. One later amendment is recorded only
> as a comment on ADR-66 (`aida show ADR-66 --comments`), not in its body:
> the reasoned-hold human-at-TTY carve-out (operator decision, Joe,
> 2026-10-04; TASK-1594 confirmation D). Hold authority (BUG-1793) keeps a
> bare human-at-TTY leg that this ADR removed from advisor authority, because
> holds are the emergency brake and must stay reachable by a human at an
> interactive terminal even when grant issuance is broken.
