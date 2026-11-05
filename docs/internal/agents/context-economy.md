# Claude context economy

Claude Code reloads project instructions, skill/command catalogue descriptions,
hook output, and enabled connector schemas on calls. Keep always-loaded text as
an index; put workflow detail in skills and topic documents read on demand.

## Budgets

The authoritative byte limits are in
`aida-core/templates/context-budget.toml`. Unit tests measure this repository's
`CLAUDE.md` and a generated scaffold baseline (generated `CLAUDE.md`, its AIDA
and discipline imports, and catalogue descriptions). A budget change therefore
requires an explicit config diff instead of silently weakening a test constant.

## Connectors

AIDA's routine repository workflow does not require claude.ai connectors. In
Claude Code, run `/mcp`, select each unused claude.ai connector, and choose
Disable for the project/session before starting a cost-sensitive run. Connector
availability is account-managed and Claude Code does not currently expose a
portable project `settings.json` key that AIDA can safely scaffold, so AIDA does
not write guessed connector names into project settings. Keep only connectors a
task actually needs and re-enable them through `/mcp` on demand.

## Measurement

Compare fresh sessions using the same Claude Code version and the same connector
state. Record the first API-call input-token count for both this repository and a
scratch repository immediately after `aida init`. The irreducible harness floor
must be measured separately; the scaffold target is floor plus 10k tokens.

<!-- trace:TASK-1441 | ai:codex -->
