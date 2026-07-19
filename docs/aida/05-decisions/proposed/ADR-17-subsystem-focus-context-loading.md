# ADR-17 (Proposed) - Focused context loads universal plus subsystem context

Status: Proposed
Specs: TASK-0436

## Context

The memory-pack design already distinguishes universal discipline from focused
subsystem memories. Specialized advisors need the same idea across docs, plans,
requirements, helper discovery, and role prompt addenda.

## Decision

Context loading always includes universal project orientation and the owning
requirement graph. Subsystem focus only adds or prioritizes relevant memories,
docs, plans, recent specs, findings, and helper families. It never hides
universal discipline or the current spec's required context.

Agents infer focus from explicit session/queue focus, explicit subsystem tags,
metadata matches, and worktree paths, in that order.

## Rationale

Focused loading reduces noise without making agents blind to project-wide
contracts. Explicit focus wins over inference so operators can correct the
router.

## Consequences

CLI, TUI, and session manifests should surface active focus. Memory frontmatter
and existing subsystem-scoped memory concepts become one input into the broader
context-loading model.

<!-- trace:TASK-0436 | ai:codex -->
