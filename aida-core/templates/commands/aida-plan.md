# Plan Requirement Implementation

Decompose and design an approved requirement before coding begins.

## Instructions

Follow the workflow in `.claude/skills/aida-plan.md`:

1. Read the requirement (`aida show <ID>`) and its related links
2. Survey the affected code surface
3. Propose an implementation plan: ordered steps, files to touch, risks, test strategy
4. Save the plan to `docs/plans/YYYY-MM-DD-<slug>.md` using `docs/plans/_TEMPLATE.md` as the starting structure (Approach + diagram, Decisions, Files in build-order, Critical Files, Reusable helpers, Risks + gotchas, Tests named, Verification, Followups, Related). Prefer symbol refs (`fn foo`) over line refs (`main.rs:123`) — symbol refs survive edits.
5. Link the plan back to the requirement (description or comment)

Use when an approved requirement is about to enter implementation.
