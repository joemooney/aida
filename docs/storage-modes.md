# AIDA Storage Modes

**Setup/storage review**: 29 October 2026. Start with the [operator guide](operator-setup.md); backend-specific legacy details remain scoped below.

<!-- trace:TASK-B6B1278890774EF993D2EBC327862470-112 | ai:codex -->

AIDA's recommended deployment is **git-canonical with a SQLite read cache**. Other modes exist for specific scenarios (PostgreSQL for shared server-backed deployments) or backwards compatibility (legacy single-file YAML / standalone SQLite). This document explains all options and how to choose.

## TL;DR

```
Default:                                      aida init
                                              → orphan branch + cache view

Multi-repo workspace (one store, many repos): aida init --sibling

Server-backed shared projection:              build with `--features postgres`,
                                              point at postgres:// URL

Legacy SQLite-canonical (deprecated):         aida init --centralized
```

---

## Overview

| Mode | IDs | Offline? | Multi-user? | Status |
|------|-----|----------|-------------|--------|
| [Git worktree (default)](#git-worktree-default) | Clone-local IDs; global IDs require enrollment | Yes | Yes (via git) | **Recommended** |
| [Git sibling](#git-sibling-repo) | Clone-local IDs; global IDs require enrollment | Yes | Yes (via git) | Recommended for multi-repo |
| [PostgreSQL](#postgresql-opt-in) | `FR-001` | No | Yes | Opt-in (feature flag) |
| [Legacy SQLite](#legacy-sqlite-deprecated) | `FR-001` | Yes (solo) | Limited | Deprecated |
| [Legacy YAML](#legacy-yaml-deprecated) | `FR-001` | Yes (solo) | No | Deprecated |

---

## Git Worktree (default)

**Best for**: Solo developers and teams alike. The default for `aida init`.

```bash
cd my-project
git init       # if not already a git repo
aida init
```

**How it works**:
- Requirements live on an orphan git branch named `aida-store` (separate history from your code)
- One YAML file per requirement: `objects/<TYPE>/000/SPEC-ID.yaml`
- A worktree at `.aida-store/` (gitignored) is the live working copy
- A SQLite read cache at `.aida/cache.db` (gitignored, derived; bounded reads can report stale context) projects summary fields for fast list/filter/search
- Writes persist through the git-canonical backend and its derived cache. Cache freshness is checked against store revisions; bounded reads may label stale/deferred/busy context rather than promise a synchronous full rebuild.
- Default WORK clones allocate UUID-bound, per-type node-local IDs. Only an explicitly enrolled HOME clone may consume global blocks or promote IDs; cached blocks and shared metadata are not authority. These are clone allocation names, not privacy profiles or physical locations.
- Clone-local identity, counters and transport policy are shared by linked worktrees through the common Git directory. Store history, the cache and local operational evidence have different recovery and backup requirements.
- Read the [allocation and transport reference](cli/07-project-setup.md) for source-backed policy and limitations; do not assume a pull/push automatically promotes IDs or provides a future hierarchical allocation/sharing protocol.

**Files**:
```
myproject/
├── .git/                                  ← code branch (main, etc.)
├── .aida/
│   ├── config.toml                        ← distributed-mode marker
│   └── cache.db                           ← SQLite read cache (gitignored)
├── .aida-store/                           ← worktree of aida-store branch (gitignored)
│   ├── metadata.yaml
│   └── objects/
│       ├── FR/000/FR-1-001.yaml
│       └── EPIC/000/EPIC-1-001.yaml
└── (your code)
```

**Sync to remote**:
```bash
cd .aida-store && git push -u origin aida-store
# Or via aida helper:
aida db sync --pull --push
```

**New developer setup**:
```bash
git clone <repo>
cd <repo-directory>
aida init                                  # or: aida node acquire; establish this clone before creating specs
aida list                                  # attaches available store history; inspect freshness labels
```

---

## Git Sibling Repo

**Best for**: Multi-repo workspaces where multiple code repos share one requirements store.

```bash
cd my-workspace
aida init --sibling --registry-remote git@github.com:org/aida-registry.git
```

**How it works**:
- The store lives in `../aida-store/` as a separate git repo, not as an orphan branch in your code repo
- The cache and canonical-store distinction still applies. Use `--attach` with the configured store path to join an existing sibling store without reseeding it; inspect allocation and transport for this exact attachment.

**Files**:
```
workspace/
├── code-repo-1/
│   ├── .aida/config.toml          ← points to ../aida-store
│   └── .aida/cache.db
├── code-repo-2/
│   ├── .aida/config.toml          ← points to ../aida-store
│   └── .aida/cache.db
└── aida-store/                    ← shared requirements (own git repo)
```

---

## PostgreSQL (opt-in)

**Best for**: Teams that want a server-backed shared projection and don't need offline capability.

PostgreSQL support is gated behind the `postgres` Cargo feature. Default builds **don't** include it; you build/install with the feature explicitly:

```bash
cargo install --git https://github.com/joemooney/aida.git aida-cli --features postgres
```

Then point at a connection string:

```bash
aida --file "postgres://user:pass@host:5432/aida_default" list
```

**How it works**:
- One PostgreSQL row per requirement in a normalized schema
- JSON columns for `history`, `comments`, `relationships`, etc.
- No git store, no orphan branch, no offline capability
- Concurrency via PostgreSQL's MVCC

**Future direction** (per EPIC-1-001): PostgreSQL support will be extracted into a separate `aida-backend-postgres` plugin crate. Same usage; cleaner separation.

---

## Legacy SQLite (deprecated)

**Status**: Deprecated. `aida init --centralized` prints a warning. Use git-canonical instead.

```bash
# Only if you really need it:
aida init --centralized
# Creates requirements.db (SQLite, single file)
```

**Why it exists**: Pre-EPIC-1-001 default. Single SQLite file with optimistic locking; the pre-commit hook auto-exports to `requirements.yaml` for diffable history.

**Why you shouldn't use it for new projects**: No distributed support, no offline-friendly conflict resolution, no agent-readable per-object YAML, single point of contention for writes. The git-canonical store gives you all the same single-machine ergonomics plus everything else.

---

## Legacy YAML (deprecated)

**Status**: Deprecated. The standalone `requirements.yaml` mode predates SQLite-canonical and predates git-canonical by a wider margin.

**Why you shouldn't use it**: A monolithic YAML file forces full reload + full rewrite for every operation; concurrent edits can conflict. The git-canonical store gives you per-object YAML files (still diffable, with conflict handling for concurrent edits) plus the cache for query speed.

---

## Comparison Matrix

| Feature | Git Worktree | Git Sibling | PostgreSQL | Legacy SQLite | Legacy YAML |
|---------|------|--------|------------|-------------|-------------|
| **Default in `aida init`** | ✅ | --sibling | (opt-in build) | --centralized | (no longer offered) |
| **Concurrent writes** | Yes (per-file in git) | Yes (per-file in git) | Yes (MVCC) | Limited (file lock) | No |
| **Offline capable** | ✅ | ✅ | ❌ | ✅ (solo) | ✅ (solo) |
| **Agent-readable YAML** | ✅ | ✅ | ❌ | (yaml export only) | ✅ |
| **Cache-accelerated queries** | ✅ | ✅ | (postgres is its own index) | ❌ | ❌ |
| **Git history per object** | ✅ | ✅ | ❌ | ❌ | ❌ |
| **Merge conflicts** | Rare (per-file) | Rare (per-file) | N/A | N/A | Frequent (monolithic file) |
| **Web dashboard** | Via aida-server | Via aida-server | Via aida-server | Via aida-server | Via aida-server |
| **Multi-repo workspace** | ❌ | ✅ | ✅ | ❌ | ❌ |
| **Node-namespaced IDs** | ✅ (`FR-1-001`) | ✅ (`FR-1-001`) | ❌ | ❌ | ❌ |
| **Agreed short IDs (merge gate)** | ✅ | ✅ | ❌ | ❌ | ❌ |
| **Setup complexity** | Trivial (`aida init`) | One extra flag | Build w/ feature + DB setup | Trivial (`--centralized`) | Trivial (manual) |
| **Max practical scale** | ~100K reqs | ~100K reqs | Unlimited | ~100K reqs | ~500 reqs |

---

## Migration

```bash
# Export legacy backend → git-canonical store (one-time, idempotent)
aida db export-git -o aida-store

# Migrate between legacy backends (still works for backwards compat)
aida db migrate --from yaml --to sqlite
aida db migrate --from sqlite --to postgres --output "postgres://..."
```

A first-class `aida db migrate --to git-canonical` command for legacy SQLite/YAML projects is planned but not yet shipped. For now, use `aida db export-git -o aida-store` followed by `aida init` (which creates the `.aida/config.toml` distributed marker).

---

## Choosing the Right Mode

```
Start here
    │
    ├── New project? → aida init  (git worktree, default)
    │
    ├── Multiple code repos sharing requirements?
    │       → aida init --sibling  (git sibling repo)
    │
    ├── Team with always-on connectivity, want server-backed shared projection?
    │       → Build with --features postgres, point at a postgres:// URL
    │
    └── Existing project on legacy SQLite/YAML?
            → Continue using it (deprecated, still works)
            → Or migrate: aida db export-git -o aida-store && aida init
```

For most projects: `aida init` and don't think about it.

---

## Combining Modes

The git store is the canonical write-of-record; PostgreSQL or external systems can be derived projections.

**Git store + PostgreSQL projection** (advanced; requires postgres feature):
```bash
aida init                       # git is canonical
# Periodically replicate the git store into a PostgreSQL projection for
# read-heavy team queries. Implementation pattern; not yet a built-in command.
```

**Git store + GitHub Issues sync**:
```bash
aida init
aida github config --repo org/project
aida github push FR-1-001       # creates GitHub issue from requirement
aida github pull                # imports GitHub issues as requirements
```

**Git store + GitLab / Jira sync**: same pattern via `aida gitlab` and `aida jira` subcommands (require corresponding feature flags).

### Allocation retry and duplicate-ID safeguards

The online store-allocation retry budget defaults to three attempts. Set a positive `[store.allocation] retry_max = N` in `.aida/config.toml` to change it. This is a retry control; it does not grant a WORK clone HOME allocation authority.

Store synchronization checks for duplicate `SPEC-ID` claims. If different objects claim the same ID, AIDA refuses to continue and prints recovery guidance rather than silently discarding one object's content. Preserve both objects and follow that guidance before retrying synchronization.
