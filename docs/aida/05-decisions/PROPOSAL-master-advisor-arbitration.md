# ADR-20 (Accepted) - Master advisor arbitrates cross-subsystem and high-risk work

Status: Accepted 2026-07-19 (operator ratification; store decision ADR-20)
Specs: TASK-0437

## Context

Subsystem advisors are useful only if their narrower context does not fracture
AIDA's architecture. Existing autonomy rules already distinguish reversible
local decisions from strategy, irreversibility, security, and unrecorded human
preference.

## Decision

Subsystem advisors may decide local, reversible, grounded questions inside their
subsystem authority. Cross-subsystem conflicts, keystone/security work, public
contracts, file formats, orchestrator semantics, lifecycle vocabulary, and
autonomy machinery default to the master advisor or human gate unless explicitly
delegated.

Advisor autopilot authority narrows under subsystem scope: a scoped advisor can
auto-execute only in-fence, grounded, reversible actions for its subsystem.
Approvals, rejections, and risk-loosening follow the existing conservative
authority map.

## Rationale

Focused advisors should reduce context load, not create multiple final
architects. The master advisor preserves whole-project coherence, and the human
remains the terminus for strategy or irreversible choices.

## Consequences

Routing code needs a deterministic escalation explanation. ADR-14's asymmetric
override applies: tightening supervision is free; loosening from master/human to
local subsystem authority requires an explicit force-style act and leaves an
audit trail.

<!-- trace:TASK-0437 | ai:codex -->
