# AIDA — AI-Native Requirements Management

**The durable, agent-readable spec layer for AI-assisted software development.**

Stable IDs, typed relationships, and code-to-spec traces give Claude (and you) a map of *what exists and why*, across sessions. AI assistants query structured data instead of parsing prose.

## Why this exists

AI coding assistants write code fast. Nobody tracks **why** the code exists. After a month of AI-assisted development:

- Can you trace a line of code back to the decision that drove it?
- Does your AI assistant know what's already been built so it doesn't re-invent it?
- Can a new agent (or human) come into the project mid-stream and orient quickly?

AIDA's bet: an opinionated, agent-readable spec layer beats unstructured markdown notes when the goal is *enforcement* — preventing duplication, surfacing prior decisions, and keeping the "why" attached to the code that implements it.

```rust
// trace:FR-042 | ai:claude
fn validate_token(token: &str) -> Result<Session> { ... }
```

```
[AI:claude] feat(auth): add token validation (FR-042)
```

Every line of code links back to a requirement. Every commit references what it implements. The MCP server exposes the whole graph to any agent.

## Quick start

### Install

**Pre-built binary** (Linux x86_64 / arm64, macOS x86_64 / arm64) — no Rust toolchain needed. Auto-detects platform, installs to `~/.local/bin/`:

```bash
curl -sSL https://raw.githubusercontent.com/joemooney/aida/main/scripts/install.sh | bash
```

Pin a specific release or change the install prefix:

```bash
curl -sSL https://raw.githubusercontent.com/joemooney/aida/main/scripts/install.sh | bash -s -- --version v0.4.0
curl -sSL https://raw.githubusercontent.com/joemooney/aida/main/scripts/install.sh | bash -s -- --prefix /usr/local/bin
```

After install, `aida upgrade` is the one-command path for future versions.

**From source** (requires Rust toolchain — always pulls latest `main`):

```bash
cargo install --git https://github.com/joemooney/aida.git aida-cli
```

> Optional integrations (PostgreSQL, GitHub/GitLab/Jira sync) are off by default. Add `--features postgres,github,gitlab,jira` to the `cargo install` line if you need them.

### Bootstrap a project

```bash
cd my-project
git init                                 # if not already a git repo
aida init                                # one command: store + cache + skills + MCP

aida add --title "User auth" --type story
aida list
aida show FR-1-001
```

`aida init` creates the orphan-branch git store, a SQLite cache for fast queries, the MCP server config, Claude Code skills + commands + hooks, and a docs/plans/ archive — in one command.

## What you get

- **CLI (`aida`)** — daily-driver command-line interface
- **MCP server** — Claude Code (and any MCP client) queries requirements natively
- **Claude Code skills** — `/aida-req`, `/aida-implement`, `/aida-commit`, `/aida-capture`, `/aida-search`, `/aida-plan`, and more
- **Trace comments + commit hooks** — code-to-requirement linkage enforced at commit time
- **React web dashboard** — kanban, sprint planning, my-queue inbox, AI chat (start with `aida-server` + `cd aida-web-react && npm run dev`)
- **Stable IDs** — `FR-1-001` (node-namespaced) and `FR-1` (agreed short ID, assigned at merge-to-trunk)
- **Distributed by default** — offline-capable, multi-node, conflict-detecting via HLC timestamps + git
- **Optional integrations** — GitHub / GitLab / Jira sync, PostgreSQL backend (compile with the corresponding feature flags)

## With Claude Code

After `aida init`, the most-used skills:

| Skill | Purpose |
|-------|---------|
| `/aida-req` | Add a requirement with AI quality feedback |
| `/aida-implement` | Implement a requirement with traceability |
| `/aida-commit` | Commit with automatic requirement linking |
| `/aida-capture` | End-of-session safety net for un-traced work |
| `/aida-search` | Unified search across requirements + code |
| `/aida-plan` | Plan implementation (vertical slice decomposition) |
| `/aida-onboard` | Interactive project walkthrough |

Run `aida` (no args) for the full CLI surface.

## Architecture (one paragraph)

Git is the canonical store: one YAML file per requirement on the orphan `aida-store` branch, sharded as `objects/TYPE/000/SPEC-ID.yaml`. A SQLite cache at `.aida/cache.db` (gitignored, auto-rebuilt) projects summary fields for fast list/filter/search. An FTS5 virtual table backs full-text search. Writes go to git first, then the cache (write-through). The cache stale-detects via the orphan branch's HEAD SHA and rebuilds when needed. PostgreSQL is opt-in via feature flag for teams wanting a server-backed shared projection. See `docs/admin-guide.md` for the full storage details and `docs/plans/2026-05-02-git-canonical-storage.md` for the design rationale.

## Documentation

| Doc | What it covers |
|-----|----------------|
| [Getting Started](docs/getting-started.md) | Install, init, first requirement |
| [Administrator's Guide](docs/admin-guide.md) | Storage backends, migration, multi-user setup |
| [User Guide](docs/user-guide.md) | Daily-use reference for the CLI and dashboard |
| [Why AIDA?](docs/WHY-AIDA.md) | Problem statement and competitive positioning |
| [Future Vision](docs/future-vision.md) | AIDA in the agentic coding era |
| [Skills vs Commands](docs/UNDERSTANDING_SKILLS.md) | How Claude Code skills and commands differ |
| [`docs/plans/`](docs/plans/) | Implementation plan archive (chronological) |

## License

MIT
