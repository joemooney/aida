# AIDA — AI-Native Requirements Management

**Track what you build and why, with structured context for AI agents.**

AIDA bridges the gap between business intent and code reality. Requirements link to code through trace comments. Commits reference spec IDs. AI assistants query structured data instead of parsing prose.

## Install

```bash
# From source (requires Rust)
cargo install --git https://github.com/joemooney/aida.git aida-cli

# Pre-built binary (Linux/macOS)
# See https://github.com/joemooney/aida/releases
```

## Quick Start

```bash
cd my-project
aida init                                          # initialize AIDA
aida add --title "User auth" --type functional     # add a requirement
aida list                                          # list all
aida show FR-001                                   # show details
aida edit FR-001 --status approved --owner alice    # update
aida search "auth"                                 # search
```

## Why AIDA?

AI coding assistants write code fast, but nobody tracks **why** the code exists. After a month of AI-assisted development:

- Can you trace a line of code back to a business decision?
- Does your AI assistant know what the system is supposed to do?
- Can a new team member understand what was built and why?

AIDA answers these questions with **queryable, typed, relational requirements data** that both humans and AI agents can work with.

```rust
// trace:FR-042 | ai:claude
fn validate_token(token: &str) -> Result<Session> { ... }
```

```
[AI:claude] feat(auth): add token validation (FR-042)
```

Every line of code links back to a requirement. Every commit references what it implements.

## Features

- **CLI** — full-featured with interactive prompts and search
- **React Dashboard** — kanban, list, sprint planning, timeline, AI chat
- **5 Storage Modes** — YAML, SQLite, PostgreSQL, Git worktree, Git sibling
- **Distributed Mode** — offline-capable with node-namespaced IDs and git sync
- **20+ Claude Code Skills** — `/aida-req`, `/aida-implement`, `/aida-commit`, `/aida-grill`, etc.
- **MCP Server** — any AI agent can query requirements via Model Context Protocol
- **GitHub Integration** — push/pull/sync requirements as GitHub issues
- **GitLab Integration** — bidirectional issue sync with label mapping
- **Two-Tier IDs** — `FR-1-001` (distributed) → `FR-1` (short, at merge)
- **Conflict Detection** — field-level conflict detection on sync
- **Telemetry** — measure skill usage, cycle time, traceability coverage

## Storage Options

| Mode | Best For | IDs | Setup |
|------|----------|-----|-------|
| SQLite | Solo dev (default) | `FR-001` | `aida init` |
| PostgreSQL | Teams, always connected | `FR-001` | `make serve` |
| Git Worktree | Distributed, offline | `FR-1-001` | `aida init --distributed` |
| Git Sibling | Multi-repo workspaces | `FR-1-001` | `aida init --distributed --sibling` |
| YAML | Simplest, git-friendly | `FR-001` | `aida init` |

Migrate between any modes: `aida db migrate --from sqlite --to postgres`

## With Claude Code

After `aida init`, these skills are available:

| Skill | Purpose |
|-------|---------|
| `/aida-req` | Add a requirement with AI quality feedback |
| `/aida-implement` | Implement a requirement with traceability |
| `/aida-commit` | Commit with automatic requirement linking |
| `/aida-grill` | Adversarial design review — walk every decision branch |
| `/aida-plan` | Plan implementation with vertical slice decomposition |
| `/aida-decompose` | Break large requirements into vertical slices |
| `/aida-triage` | Structured bug investigation |
| `/aida-sprint` | Sprint planning and management |
| `/aida-capture` | End-of-session requirement capture |

## With GitHub

```bash
export AIDA_GITHUB_TOKEN="ghp_..."
aida github config --repo org/project
aida github push FR-001        # create GitHub issue from requirement
aida github pull               # import GitHub issues as requirements
aida github sync               # detect drift between AIDA and GitHub
```

## Multi-User

```bash
# Start PostgreSQL + server
make dev-pg && make serve

# Other machines connect via REST API or CLI
aida --file "postgres://aida:aida@server:5432/aida_default" list
# Or open http://server:8080 for the React dashboard
```

## Architecture

```
aida-core/         Core library — models, storage, distributed architecture
aida-cli/          CLI binary (aida)
aida-server/       REST + gRPC server
aida-web-react/    React dashboard (Vite + Tailwind)
aida-desktop/      Native GUI (egui)
```

Built in Rust. 150 tests. 5 storage backends. MIT licensed.

## Documentation

| Doc | What it covers |
|-----|----------------|
| [Getting Started](docs/getting-started.md) | Install, init, first requirement |
| [Storage Modes](docs/storage-modes.md) | All 5 modes with comparison matrix |
| [Multi-User Setup](docs/multi-user-setup.md) | PostgreSQL team deployment |
| [Why AIDA?](docs/WHY-AIDA.md) | Problem statement and competitive positioning |
| [Future Vision](docs/future-vision.md) | AIDA in the agentic coding era |

## License

MIT
