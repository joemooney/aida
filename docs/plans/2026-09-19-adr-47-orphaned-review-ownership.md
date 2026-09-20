# ADR-47: Orphaned review ownership is recovered by handoff plus a bounded sweep

**Status:** Accepted  
**Decision owner:** advisor  
**Linked spec:** BUG-1291  
**Related:** STORY-1218, TASK-1284, BUG-1268, STORY-1354

## Context

Review dispatch is phase-coupled. A drain normally reaches its reviewer phase
and therefore owns the transition from an implementation PR to an independent
review. If the run shelves first, or is killed between phases, a clean open PR
can have no live owner even though every individual surface looks healthy.
PR 1970 and its review story STORY-1354 are the historical regression: the PR
was clean and green, but the run shelved before review and nothing claimed it.

## Decision

Use two complementary recovery paths:

1. **Primary — hand off while shelving.** After a successful shelf, a run that
   knows an open PR must idempotently place that PR's review story on the
   reviewer queue. The queue note records the failed phase and stop reason so
   the reviewer inherits the context.
2. **Backstop — bounded scheduler-tick sweep.** The existing scheduler tick
   inspects the bounded open-PR set and claims for review any clean PR with no
   verdict, no merge hold, and no live owner. Liveness uses the lease's PID plus
   process-start identity (TASK-1284), not PID alone. The sweep only queues a
   reviewer; it never reviews or merges.
3. **Visibility — `aida awaiting`.** A clean open PR without an approved
   verdict remains visible in the awaiting report until review ownership and
   the normal review/merge gates resolve it.

The per-repository drain lock remains strictly single-owner. Neither recovery
path starts a concurrent implementer drain or weakens the lock.

## Rejected alternative

Allowing a reviewer drain alongside the implementer drain was rejected. It
turns a dispatch failure into concurrent claims over shared leases and
worktrees, weakening an integrity invariant to repair a recoverable handoff.

## Consequences

Shelving performs one additional best-effort queue operation when a PR exists.
Abrupt process death is covered at the next scheduler tick. Both paths are
idempotent because the existing review-story auto-queue recognizes and
re-queues the one story associated with a PR. Review authority and merge
authority are unchanged.
