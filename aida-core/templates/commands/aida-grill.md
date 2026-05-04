# Grill A Requirement

Interrogate a requirement or design decision by walking every branch of the tree.

## Instructions

Follow the workflow in `.claude/skills/aida-grill.md`:

1. Read the requirement or proposal under scrutiny
2. Walk every decision branch — happy path, error paths, edge cases, scale, security, ops
3. List unstated assumptions and contradictions explicitly
4. For each gap, propose a clarifying question or a child requirement
5. Summarize verdict: ready to implement, needs decomposition, or blocked on decisions

Use before implementation begins, especially for high-risk or cross-cutting work.
