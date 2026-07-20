# ADR-17 (Accepted) - First-class subsystems as project registry

Status: Accepted 2026-07-19 (operator ratification; store decision ADR-17)
Specs: TASK-0434

## Context

AIDA currently infers scope from features, tags, paths, queues, roles, and
conventions such as the docs lane. Those signals are useful but not one model.
Subsystem-scoped advisors need a durable representation that can be shared by
the CLI, TUI, memory loader, queue router, and MCP server.

## Decision

Represent subsystems as project-local registry configuration in the
git-canonical store, initially `.aida-store/registry/subsystems.yaml`.
Subsystems are not requirement nodes. They are classification and routing
policy read by requirement, session, queue, and context-loading surfaces.

Each subsystem has a stable `name`, descriptive fields, role names, matching
rules, memory focus keys, queue defaults, risk policy, and escalation policy.

## Rationale

This keeps work items and routing policy separate. Requirements stay the graph
of desired work. Subsystems become the shared vocabulary that explains which
part of the project owns a file, spec, question, or review.

## Consequences

The store gains one registry file and validation path. Membership should be
computed into cache, not stamped onto every requirement. Explicit
`subsystem:<name>` tags remain available when the operator wants to pin a spec.

<!-- trace:TASK-0434 | ai:codex -->
