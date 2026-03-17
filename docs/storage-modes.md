# AIDA Storage Modes

**Last updated**: 2026-03-16

AIDA supports multiple storage backends and deployment modes. This document covers all options, when to use each, and how to set them up.

---

## Overview

| Mode | IDs | Offline? | Multi-user? | Setup |
|------|-----|----------|-------------|-------|
| [YAML](#1-yaml-file) | `FR-001` | Yes (solo) | No | `aida init` |
| [SQLite](#2-sqlite) | `FR-001` | Yes (solo) | Limited | `aida init` (default) |
| [PostgreSQL](#3-postgresql) | `FR-001` | No | Yes | `make dev-pg` + migrate |
| [Git Worktree](#4-git-worktree) | `FR-1-001` → `FR-1` | Yes | Yes | `aida init --distributed` |
| [Git Sibling](#5-git-sibling-repo) | `FR-1-001` → `FR-1` | Yes | Yes | `aida init --distributed --sibling` |

---

## 1. YAML File

**Best for**: Solo developer, small project, maximum simplicity.

```bash
aida init
# Creates requirements.yaml (if no requirements.db exists)
```

**How it works**:
- All requirements stored in a single `requirements.yaml` file
- File is tracked in git — full diff history
- No external dependencies

**Limitations**:
- No concurrent access — file-level locking only
- Large files become unwieldy (>500 requirements)
- Merge conflicts on the monolithic file when multiple branches touch requirements

**Files**:
```
myproject/
├── requirements.yaml     ← all requirements in one file
└── .git/
```

---

## 2. SQLite

**Best for**: Solo developer or small team on a shared machine, moderate project size.

```bash
aida init
# Creates requirements.db (SQLite, default)
```

**How it works**:
- SQLite database with WAL mode for concurrent read access
- Optimistic locking via version column
- Pre-commit hook exports to `requirements.yaml` for git-diffable history

**Limitations**:
- Binary file — can't be diffed directly in git
- Single-machine only (SQLite doesn't support network access)
- Pre-commit hook required to maintain YAML export

**Files**:
```
myproject/
├── requirements.db       ← SQLite database (gitignored)
├── requirements.yaml     ← auto-exported by pre-commit hook (tracked)
└── .git/hooks/pre-commit ← auto-export hook
```

---

## 3. PostgreSQL

**Best for**: Teams with reliable network connectivity, web dashboard users.

```bash
# Start PostgreSQL
make dev-pg

# Migrate from SQLite
aida db migrate --from sqlite --to postgres \
  --output "postgres://aida:aida@localhost:5432/aida_default"

# Use directly
aida --file "postgres://aida:aida@localhost:5432/aida_default" list

# Or start the server for web/REST access
make serve
```

**How it works**:
- Central PostgreSQL database — all users connect to same instance
- Full concurrent read/write with MVCC
- Optimistic locking — version conflicts reported, never silent overwrites
- REST API + React dashboard via `aida-server`

**Limitations**:
- Requires running PostgreSQL server
- No offline access — needs network connectivity
- No built-in git history (use YAML export for snapshots)

**Connection methods**:

| Method | Command |
|--------|---------|
| Direct CLI | `aida --file "postgres://user:pass@host:5432/db" list` |
| REST API | `curl http://host:8080/api/v2/requirements` |
| React dashboard | `http://host:8080` in browser |
| MCP (Claude Code) | Configure `.mcp.json` with connection string |

**Files**:
```
myproject/
├── .aida/docker-compose.dev.yml  ← PostgreSQL container
├── pgdata/                       ← PostgreSQL data (gitignored)
└── (no local requirements file needed)
```

See [docs/multi-user-setup.md](multi-user-setup.md) for detailed instructions.

---

## 4. Git Worktree (Distributed, Default)

**Best for**: Single-repo projects that need offline capability, distributed teams, or git-native history.

```bash
aida init --distributed
```

**How it works**:
- Creates an **orphan branch** called `aida-store` with no shared history with your code branch
- Checks it out as a **worktree** at `.aida-store/` (gitignored on main)
- Each requirement is a separate YAML file in a sharded directory layout
- Every mutation auto-commits to the orphan branch
- IDs are node-namespaced (`FR-1-001`) with short agreed IDs (`FR-1`) at merge gate
- Sync via standard `git push origin aida-store`

**Two branches, one repo, one remote**:
```
main branch:        source code, CLAUDE.md, docs
aida-store branch:  requirements (orphan, separate history)
```

**New developer setup** (after cloning):
```bash
git clone <repo>
git worktree add .aida-store aida-store
# Done — aida commands auto-detect the store
```

**Files**:
```
myproject/
├── .aida/
│   └── config.toml               ← points to .aida-store
├── .aida-store/                   ← worktree (gitignored on main)
│   ├── .git                       ← worktree link to main .git
│   ├── metadata.yaml              ← store config, counters
│   ├── objects/
│   │   ├── FR/000/FR-1-001.yaml   ← one file per requirement
│   │   ├── BUG/000/BUG-1-002.yaml
│   │   └── ...
│   ├── registry/
│   │   └── agreed_counters.toml   ← agreed ID counters
│   └── .aida/
│       ├── node.toml              ← node identity
│       └── dispenser.toml         ← sequence counter state
├── src/                           ← your code (on main branch)
└── .git/                          ← shared git database
```

**Advantages**:
- One repo, one remote, one clone
- Clean separation — code diffs never include requirements
- Standard `git push/pull` for sync
- Full offline capability
- Works with any git host (GitHub, GitLab, Gitea, bare repos)

**Limitations**:
- Requires `git worktree add` step for new developers
- Not suitable for multi-repo workspaces (use sibling mode instead)

---

## 5. Git Sibling Repo (Distributed, Multi-Repo)

**Best for**: Multiple code repos sharing one requirements store, enterprise workspaces.

```bash
aida init --distributed --sibling [--registry-remote git@github.com:org/aida-store.git]
```

**How it works**:
- Creates a **separate git repo** at `aida-store/` alongside the code repo
- Same sharded YAML file layout as worktree mode
- Each code repo in the workspace points to the shared store
- Node registration via CAS push loop ensures unique IDs across all repos

**Workspace layout**:
```
workspace/
├── pacgate/              ← code repo 1
│   └── .aida/config.toml → store_path = "../aida-store"
├── pacinet/              ← code repo 2
│   └── .aida/config.toml → store_path = "../aida-store"
├── aida-store/           ← shared requirements store (separate git repo)
│   ├── objects/
│   ├── metadata.yaml
│   └── registry/nodes.toml
└── .aida-workspace       ← workspace config (optional)
```

**Advantages**:
- Requirements shared across multiple code repos
- Each repo can reference requirements from any other repo
- Cross-repo relationships work naturally (both objects in same store)
- Independent access control (store repo can have different permissions)

**Limitations**:
- Two repos to manage (code + store)
- Developers must clone both repos
- More complex setup than worktree mode

---

## Comparison Matrix

| Feature | YAML | SQLite | PostgreSQL | Git Worktree | Git Sibling |
|---------|------|--------|------------|-------------|-------------|
| **Concurrent writes** | No | Limited | Yes (MVCC) | Yes (per-file) | Yes (per-file) |
| **Offline capable** | Yes | Yes | No | Yes | Yes |
| **Git-diffable** | Yes (monolithic) | No (binary) | No | Yes (per-file) | Yes (per-file) |
| **Merge conflicts** | Frequent | N/A | N/A | Rare (per-file) | Rare (per-file) |
| **Web dashboard** | Via server | Via server | Via server | Via server | Via server |
| **REST API** | Via server | Via server | Via server | Via server | Via server |
| **Multi-repo workspace** | No | No | Yes | No | Yes |
| **Node-namespaced IDs** | No | No | No | Yes | Yes |
| **Agreed IDs (short)** | N/A | N/A | N/A | Yes (merge gate) | Yes (merge gate) |
| **Auto-commit history** | N/A | N/A | N/A | Yes | Yes |
| **Setup complexity** | None | None | Docker | `git worktree add` | Two repos |
| **Max practical scale** | ~500 reqs | ~100K reqs | Unlimited | ~100K reqs | ~100K reqs |

---

## Migration Between Modes

All modes use the same `Requirement` data model — migration is lossless.

```bash
# YAML → SQLite
aida db migrate --from yaml --to sqlite

# SQLite → PostgreSQL
aida db migrate --from sqlite --to postgres \
  --output "postgres://user:pass@host:5432/db"

# PostgreSQL → YAML
aida db migrate --from postgres --to yaml --output requirements.yaml

# Any backend → Git store
aida db export-git -o /path/to/store

# Git store → PostgreSQL (via REST API)
aida-server --database /path/to/store --rest-port 8080
# PostgreSQL is then a read model / cache of the git store
```

---

## Choosing the Right Mode

```
Start here
    │
    ├── Solo developer, small project?
    │       → YAML or SQLite (aida init)
    │
    ├── Team, always online?
    │       → PostgreSQL (make dev-pg + make serve)
    │
    ├── Need offline/disconnected capability?
    │   │
    │   ├── Single code repo?
    │   │       → Git Worktree (aida init --distributed)
    │   │
    │   └── Multiple code repos sharing requirements?
    │           → Git Sibling (aida init --distributed --sibling)
    │
    └── Air-gapped / classified network?
            → Git Sibling with manual sync
```

---

## Combining Modes

Modes are not mutually exclusive:

**Git store + PostgreSQL cache**:
```bash
# Git store is source of truth
aida init --distributed

# PostgreSQL provides fast queries + web dashboard
aida-server --database .aida-store --rest-port 8080
# Server reads from git store, serves via REST API
```

**SQLite + YAML export**:
```bash
# SQLite for runtime, YAML for git history
aida init  # creates requirements.db
# Pre-commit hook auto-exports to requirements.yaml
```

**Git store + GitHub Issues sync**:
```bash
# Git store locally, push selected requirements to GitHub
aida init --distributed
aida github config --repo org/project
aida github push FR-1-001  # creates GitHub issue
aida github pull            # import GitHub issues
```
