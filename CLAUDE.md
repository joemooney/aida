# AIDA repository guide

AIDA is an agent-collaboration substrate: a git-canonical requirements graph,
typed coordination, isolated work sessions, and CLI/MCP surfaces. Start with
`OVERVIEW.md`; the former comprehensive repository guide is preserved at
`docs/agents/aida-repository-guide.md` and should be read only when its topic is
needed.

## Rules every session needs

- Requirements are canonical in AIDA, not `REQUIREMENTS.md`. Read with
  `aida show <ID>`, `aida list`, and `aida search`; mutate through AIDA commands.
- Work in an isolated sibling worktree and claim the owning spec before editing.
- Add `// trace:<SPEC-ID> | ai:<tool>` beside implementation code.
- AI-assisted commits use `[AI:tool] type(scope): summary (SPEC-ID)`. The trailing
  parenthesized IDs drive merge-time completion.
- Keep internal SPEC IDs out of end-user CLI prose unless the surface is
  explicitly for developers/operators.
- Architecture changes require a sketch on the owning spec and advisor signoff
  before implementation.

## Develop AIDA itself

Use `aida dev activate` so `aida` resolves to this checkout, and
`aida dev deactivate` to return to the released binary. Common checks:

```bash
make build-fast
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace
tests/test_mcp_stdio.sh --skip-agent-contract
```

## Template architecture

`aida-core/templates/` is the source of truth. The repository's `.claude/skills`,
`.claude/commands`, `.claude/hooks`, `.claude/settings.json`, discipline pack,
and agent docs are links or generated mirrors. Edit the template master, then run
`make sync-templates` and `make check-templates`; never edit a generated mirror.

## Read on demand

| Need | Read |
|---|---|
| Product and architecture | `OVERVIEW.md` |
| Full repository/storage/CLI guide | `docs/agents/aida-repository-guide.md` |
| Worktree, review, and sync discipline | `.aida/discipline/README.md` |
| Agent communication | `docs/agents/session-communication.md` |
| MCP/client setup | `docs/agents/aida-mcp-install-matrix.md` |
| CLI reference | `docs/cli/` or `aida <command> --help` |
| Specialized workflow | invoke the matching AIDA skill; do not preload its body |

## Context economy

Keep this file below the configured budget in
`aida-core/templates/context-budget.toml`. Put specialized instructions in
skills or topic docs and link them here. Connector configuration is documented
in `docs/agents/context-economy.md`; this repository does not require claude.ai
connectors for routine development.

<!-- trace:TASK-1441 | ai:codex -->
