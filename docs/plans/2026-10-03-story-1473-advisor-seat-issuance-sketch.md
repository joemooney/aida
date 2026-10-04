# STORY-1473 advisor-seat issuance sketch

Status: advisor signoff passed 2026-10-03; ready for guided implementation.

Decision source: Joe agreed on 2026-10-03 to **TTY grant + scoped child seat**.
Related decision: ADR-66 (Draft). Requirement: STORY-1473.

## Problem

Today `registry/team.toml` maps a person to one role, while the process
environment variable `AIDA_SESSION_ROLE` can change the active role in any
process running as that person. AIDA launchers also pass role environment to
children. This makes the roster behave like a default identity and lets
same-user sessions share authority.

## Agreed policy

1. Joe explicitly chooses a roster-allowed seat at an interactive TTY. The
   roster limits which seats he may choose; the roster entry itself does not
   make every shell an advisor.
2. Each AIDA-launched child gets only a separately issued seat that its
   launcher explicitly requested and the roster permits.
3. A parent may issue only child seats explicitly listed in its own
   TTY-issued delegation scope. Joe selected this explicit-set rule on
   2026-10-03; roster membership alone never grants a parent delegation power.
4. `AIDA_SESSION_ROLE` remains a label/hint. Setting it by itself never grants
   authority.
5. Roster changes require a human at an interactive TTY.
6. CLI and MCP authority resolution use the same session-bound grant.
7. Same-user process impersonation is not claimed to be a protected OS
   boundary by this story. The residual limit is documented, with the
   protected-boundary work tracked separately.

## Proposed implementation shape

1. Add one shared seat-grant resolver in the core/CLI boundary. Protected
   operations ask it for the active session grant instead of resolving
   `AIDA_SESSION_ROLE` directly. Missing, invalid, or mismatched grants resolve
   to the least-privilege seat.
2. Make `aida role enter` the direct-session issuance path. It must require a
   controlling interactive TTY, present only roster-allowed seats, and issue a
   grant bound to that session. A headless command cannot mint a direct
   advisor grant. Refusal/help text points back to this flow rather than an
   environment override.
3. Have each AIDA launcher pass an explicit requested child seat through the
   shared issuer. Validate it against the roster and the issuer grant's
   explicit `delegable_seats`, then issue a distinct child grant; do not copy
   the parent's authority by inheriting its role label. `aida role enter`
   offers an optional TTY selection of `delegable_seats`, defaulting to the
   empty set. Each child grant also defaults to an empty delegation set; the
   launcher may pass a nonempty subset only when the parent explicitly
   selected those seats and the child role is in that subset. This applies at
   every generation of child launches.
4. Have MCP capture the grant belonging to its server process at startup and
   use the shared resolver for tool authorization. Changing the caller shell's
   hint does not change a running server's authority; reconnect after a new
   grant is issued.
5. Gate `aida team set-role` and equivalent roster writes on a human TTY, and
   keep the roster's existing guardrail-not-security caveat.
6. Keep the existing drain-token replay limitation explicit. Do not broaden
   this change into a protected broker or claim protection against a same-UID
   process that can inspect another process's state.

## Data shape, command behavior, and grant lifecycle

- Canonical roster shape: `[members]` maps each user to an array of allowed
  seats, for example `joe = ["advisor", "product", "implementer"]`. On
  read, a legacy scalar such as `joe = "advisor"` means the singleton set
  `["advisor"]`; migration must not add seats implicitly.
- `aida team set-role <user> <seat>` remains a replace operation and writes a
  singleton array. Add explicit allow/disallow-seat operations for editing a
  multi-seat set. `unset-role <user>` removes that user's roster entry and
  invalidates any active grants on their next validation. Every roster write
  requires a controlling human TTY. Advisor signoff should confirm these
  proposed compatibility semantics before implementation.
- A grant is an opaque, unguessable handle plus a validated record held in
  AIDA's local session-grant store. The handle locates the record; possession
  is not treated as proof by itself. The record binds principal, a fresh
  session ID, issued seat, TTY issuance event, explicit delegable-seat set,
  optional parent grant ID, and issued/expiry/revocation state. Protected
  operations validate the record and current roster ceiling through the
  shared resolver.
- The interactive shell/session that completes `aida role enter` receives
  the handle. Descendant commands inherit it only through explicit AIDA
  launcher plumbing. A fresh independent shell has no handle and receives no
  seat, regardless of environment labels. `aida role end` revokes the active
  grant; missing, expired, revoked, malformed, or roster-mismatched records
  fail closed. Environment variables may carry a handle locator but never a
  seat assertion accepted as authority.
- MCP captures and validates the exact grant handle at server startup. It
  does not need a TTY because the grant was issued by the interactive parent;
  it cannot elevate itself. Reconnect/restart is required to use a later
  grant. A same-UID process that can inspect/copy another process's handle or
  local grant state remains outside this story's OS security claim.

## Required launcher and authorization inventory

Before implementation is complete, trace every protected role decision and
every AIDA-managed process spawn. The initial migration inventory is:

- Direct interactive issuance and revocation: `aida role enter` and
  `aida role end`.
- Interactive and background child launch: `aida agent new` (all vendor
  adapters and `--bg`), plus `aida session` launch/spawn paths.
- Queue pickup, drain, burndown, and orchestrator phase dispatch, including
  implementer, reviewer, advisor, and integrator children.
- MCP stdio server startup and its advisor-gated tool checks.
- All CLI and MCP authorization reads currently consulting
  `AIDA_SESSION_ROLE`, roster role defaults, or inherited session context.

The implementation must produce a call-site inventory from source search and
tests. Unknown/unmigrated launch paths and protected reads fail closed; there
is no env-only fallback. Add any discovered spawn/authorization surfaces to
this list before claiming acceptance.

## Advisor signoff requested

Please review the concrete roster compatibility behavior, explicit empty-by-
default delegation, grant locator/validation/lifecycle, MCP startup capture,
and launcher/authorization inventory above. Confirm whether the design is
ready for implementation or identify specific remaining gaps. No code is
authorized until this review signs off.

## Acceptance evidence to carry into the implementation plan

- An env-only `AIDA_SESSION_ROLE=advisor` cannot authorize a protected CLI or
  MCP operation.
- A human can select a permitted seat at a controlling TTY; a non-TTY attempt
  cannot mint one, and a roster-denied choice is refused.
- Every AIDA child launch path passes an explicit, validated child seat; a
  child cannot receive a seat beyond the issuer's delegation set or roster,
  and cannot delegate further without an explicit subset in its own grant.
- CLI and MCP produce the same authority result for the same grant.
- Roster writes refuse without a human TTY and still explain the roster's
  guardrail-not-security limit.
- Missing, expired, revoked, or mismatched grants fail closed to
  least-privilege behavior; environment labels cannot repair them.
- Documentation names the same-user replay limitation and the protected
  boundary as separate follow-up work.

## Not in this slice

- A protected broker or OS-level boundary against a malicious same-user
  process.
- A blanket ban on AIDA-launched advisor sessions.
- Any authority grant through environment variables alone.
- Changes to `registry/team.toml` seat membership without an explicit
  roster-write decision at a TTY.
