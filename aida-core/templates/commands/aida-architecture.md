# Review Codebase Architecture

Audit structural health and propose improvements aligned with requirements.

## Instructions

Follow the workflow in `.claude/skills/aida-architecture.md`:

1. Map the actual module/crate structure against the domain model in requirements
2. Identify structural issues: god objects, circular deps, shallow modules, orphaned code
3. Surface findings with concrete file paths and line ranges
4. For each significant issue, offer to file an improvement requirement (`aida add --type task`)
5. Summarize the architecture health verdict

Use when the user asks for a structural review or before a major refactor.
