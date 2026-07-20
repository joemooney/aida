# ADR-18 (Accepted) - Route advisors by subsystem with master fallback

Status: Accepted 2026-07-19 (operator ratification; store decision ADR-18)
Specs: TASK-0435

## Context

SPIKE-10 established that one full-project advisor context becomes noisy as the
memory pack and subsystem count grow. AIDA already has roles, findings, punts,
reviewers, queues, and a master advisor model, but lacks a deterministic rule
for which advisor answers a scoped question.

## Decision

Route advisor and reviewer work from computed subsystem membership. If a spec or
finding has exactly one subsystem and a matching scoped role is registered, use
`advisor:<subsystem>` or `reviewer:<subsystem>`. If membership is ambiguous,
missing, conflicting, or absent, fall back to the master advisor.

Every routed answer records the chosen role, candidate subsystems, routing rule,
and fallback/arbitration reason.

## Rationale

This gives focused advisors smaller context without creating hidden authority.
The master advisor remains the coherence holder and conflict resolver.

## Consequences

Headless advisor selection must read subsystem membership before spawning the
advisor tier. `aida status`, TUI, findings, and session manifests should expose
the routing explanation for audit.

<!-- trace:TASK-0435 | ai:codex -->
