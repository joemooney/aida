# aida-cli

Command-line interface for [AIDA](https://github.com/joemooney/aida) — AI-native requirements management.

## Install

```bash
cargo install aida-cli
```

## Quick Start

```bash
cd my-project
aida init                                          # initialize AIDA
aida add --title "User auth" --type functional     # add a requirement
aida list                                          # list all
aida show FR-001                                   # show details
aida edit FR-001 --status approved                 # update
aida search "auth"                                 # search
```

## What is AIDA?

AIDA tracks what you build and why, with structured context for AI coding agents.

- **Requirements link to code** via trace comments (`// trace:FR-042 | ai:claude`)
- **Commits reference specs** (`[AI:claude] feat(auth): add login (FR-042)`)
- **AI agents query structured data** instead of parsing prose (MCP server included)

## Features

- **5 storage modes** — YAML, SQLite, PostgreSQL, Git worktree (distributed), Git sibling (multi-repo)
- **20+ Claude Code skills** — `/aida-req`, `/aida-implement`, `/aida-commit`, `/aida-grill`, etc.
- **GitHub integration** — push, pull, sync requirements as GitHub issues
- **GitLab integration** — bidirectional issue sync
- **MCP server** — `aida mcp-serve` for Claude Code, Codex CLI, or any MCP-compatible agent
- **Distributed mode** — `aida init --distributed` for offline-capable, multi-node deployments
- **Two-tier IDs** — `FR-1-001` (distributed) → `FR-1` (short, at merge)
- **Analytics** — velocity trends, cycle time, quality scores, traceability coverage

## Storage Modes

| Mode | Best For | Setup |
|------|----------|-------|
| SQLite | Solo dev (default) | `aida init` |
| PostgreSQL | Teams, web dashboard | `make serve` |
| Git Worktree | Distributed, offline | `aida init --distributed` |
| Git Sibling | Multi-repo workspaces | `aida init --distributed --sibling` |
| YAML | Simplest | `aida init` |

## Documentation

- [Getting Started](https://github.com/joemooney/aida/blob/main/docs/getting-started.md)
- [Storage Modes](https://github.com/joemooney/aida/blob/main/docs/storage-modes.md)
- [Why AIDA?](https://github.com/joemooney/aida/blob/main/docs/WHY-AIDA.md)

## License

MIT — see [LICENSE](https://github.com/joemooney/aida/blob/main/LICENSE)
