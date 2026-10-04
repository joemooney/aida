# STORY-1485: single occupancy and takeover sketch

Status: proposed; independent advisor signoff required before implementation.
<!-- trace:STORY-1485 | ai:codex -->

## Existing surfaces

`session_handoff_seat` in `aida-cli-lib/src/lib.rs` resolves the main worktree
root and delegates handoff persistence to `seat_rotation.rs`. It currently
has no holder check. Session leases already carry active process identity,
including process start identity; `aida-core/src/liveness.rs` provides shared
identity-aware probes. Use those probes rather than treating elapsed age or
a successful bare PID probe as proof of ownership.

## Proposed contract

Default single occupancy applies to the orchestrator seat. Other roles remain
unchanged unless explicitly configured as single occupancy. Integrator drain
locking remains independent; this story must not replace it. Role authority
from STORY-1473 is a prerequisite for claiming authority, while occupancy
determines which authorized session currently holds the seat.

Keep a versioned local record under the main worktree's `.aida/`, keyed by
canonical seat, with session ID, harness, harness session ID, active PID and
start identity, acquisition time, and a monotonically increasing generation.
Serialize read-modify-write through a permanent sidecar flock and atomic
replacement. All sibling worktrees resolve to the same record. Never keep
the flock held while waiting for a peer or stopping a harness.

A claim with no holder succeeds; a provably dead holder is silently replaced.
Unknown liveness fails closed. A competing live claim refuses by default,
printing the holder and supported stop/attach commands. Reading a handoff
warns without acquiring the seat; writing requires current ownership.
Configured warning mode may relax competing inspection/claim behavior but
must never transfer ownership implicitly or permit a demoted session to
dispatch or overwrite a handoff.

Require the holder session and generation immediately before seat-scoped
dispatch and handoff writes. A resumed old holder cannot regain ownership
through a stale environment variable. Enumerate the actual orchestrator
dispatch entry points before coding, including CLI and MCP routes, and keep
ordinary implementer dispatch unaffected.

## Takeover protocol

`--force` requests graceful takeover through the existing durable mailbox.
The request names both sessions, generation, and in-flight work. The holder
stops new dispatch, writes a handoff, and acknowledges safe release. Wait up
to a configurable grace period, default 120 seconds, then recheck ownership
under the lock. Transfer only after a matching acknowledgment/release or
proven process death. A silent live holder times out with no transfer or kill.

`--force --kill` follows the same grace period. Validate human-at-TTY or
explicit operator-config authority before any takeover side effect and again
before termination. Agents cannot self-authorize termination through an
environment flag. Recheck process start identity and generation immediately
before invoking the supported harness stop command or a process signal;
never signal an unrelated process after PID reuse. Verify termination before
transferring. Report dirty leased worktrees without modifying their contents.

Record request, timeout, release, transfer, and hard-stop outcome in existing
events, with from/to identity, mode, generation, and in-flight scopes. Surface
these in `aida history`; recover interrupted requests without double transfer.

## Acceptance and validation plan

Cover concurrent claims across sibling worktrees; same-holder idempotency;
live-holder refusal; graceful acknowledgment; silent-holder timeout; agent
hard-stop refusal; authorized hard stop; dirty-worktree reporting; silent
dead-holder takeover; PID reuse; unknown liveness; takeover races; former
holder write/dispatch refusal; and history projection. Use controlled child
processes and bounded injected deadlines, never real peer sessions.

## Decisions requested from the independent advisor

Approve orchestrator-only default, fail-closed mutation checks, warning-only
handoff reads, generation fencing, and the acknowledgment-based transfer
protocol. Confirm configuration/CLI names and the operator-authority boundary
against STORY-1473 before implementation. This sketch is not a signoff.
