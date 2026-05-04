# Decompose Requirement Into Vertical Slices

Break a large requirement into independently deliverable vertical-slice children.

## Instructions

Follow the workflow in `.claude/skills/aida-decompose.md`:

1. Read the parent requirement (`aida show <ID>`)
2. Identify the layers it touches (DB, API, UI, infra, docs)
3. Propose 3–7 vertical slices that each cut through every relevant layer
4. Verify each slice is independently deliverable and testable
5. For each accepted slice, `aida add --type story --parent <ID> --title "..."`

Use when a requirement is too large to implement in one pass.
