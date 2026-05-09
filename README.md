# AIDA — Your project's missing index

**A hidden kernel that maintains a stable, queryable graph of what exists, served to AI through MCP and to you through a small CLI.**

**Without it**, coding agents start every session cold, re-deriving the same context they had yesterday; humans rediscover and re-debate decisions for years; cross-references between code and intent rot silently. **With it**, *"does this already exist?"*, *"why did we choose X?"*, and *"is this code still tied to a live requirement?"* are one query away — for the agent and for you.

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

**Working on AIDA itself?** Clone the repo, `cargo build`, then `eval "$(target/debug/aida dev shell-init)"` (or `target/debug/aida dev shell-init --install` to wire `aida-on`/`aida-off` aliases into your `~/.bashrc` permanently). After that, `aida-on` from inside the repo activates your in-repo build pyenv-style — no need to install a released binary first. See [CLAUDE.md](CLAUDE.md) for the full dev workflow.

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
