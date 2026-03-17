# AIDA — AI-Native Requirements Management

Track what you build and why, with structured context for AI coding agents.

## Install

```bash
cargo install aida-cli
```

## What is AIDA?

AIDA bridges the gap between business intent and code reality:

- **Requirements link to code** via trace comments (`// trace:FR-042 | ai:claude`)
- **Commits reference specs** (`[AI:claude] feat(auth): add login (FR-042)`)
- **AI agents query structured data** via MCP, not parse prose

## Crates

| Crate | Purpose |
|-------|---------|
| [`aida`](https://crates.io/crates/aida) | This crate — re-exports aida-core |
| [`aida-cli`](https://crates.io/crates/aida-cli) | CLI tool (`cargo install aida-cli`) |
| [`aida-core`](https://crates.io/crates/aida-core) | Core library (models, storage, analytics) |

## Features

- 5 storage backends (YAML, SQLite, PostgreSQL, Git worktree, Git sibling)
- 20+ Claude Code skills for AI-assisted development
- GitHub and GitLab integration
- MCP server for any AI agent
- Distributed mode with offline capability
- Analytics: velocity, cycle time, quality scores

## Links

- [GitHub](https://github.com/joemooney/aida)
- [Getting Started](https://github.com/joemooney/aida/blob/main/docs/getting-started.md)
- [Why AIDA?](https://github.com/joemooney/aida/blob/main/docs/WHY-AIDA.md)

## License

MIT
