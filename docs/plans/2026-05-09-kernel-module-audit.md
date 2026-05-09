# Kernel / Module Audit

**Pitch (north star):**
> AIDA is your project's missing index — a hidden kernel that maintains a stable, queryable graph of what exists, served to AI through MCP and to you through a small CLI.
>
> Without it, coding agents start every session cold, re-deriving the same context they had yesterday; humans rediscover and re-debate decisions for years; cross-references between code and intent rot silently. With it, *"does this already exist?"*, *"why did we choose X?"*, and *"is this code still tied to a live requirement?"* are one query away — for the agent and for you.

**Goal of this audit:** identify what stays in the always-installed *kernel* (small, invisible, daily-load-bearing) vs what moves out to *layered module repos* (opt-in, can evolve independently).

**Mark-up convention:** edit any **K** ↔ **M** ↔ **D** label below to override my call. **K** = kernel, **M** = module repo, **D** = drop entirely. Open questions are flagged `?`.

---

## Definition of the kernel

A surface stays in kernel if **all** of these are true:
1. **You touch it daily** in a typical AIDA-using project (or your AI does, transparently).
2. **Removing it would break the pitch** ("indexed semantic graph, served through MCP + small CLI").
3. **It has no business being a separable concern** — splitting wouldn't simplify, just fragment.

If only (1) is true, it's a *daily-driver module* (extract but install by default).
If only (2) is true, it's *plumbing* (kernel).
If neither, extract or drop.

---

## Verdict matrix — top-level CLI commands

Every top-level `Command` enum variant. Roughly grouped.

### Daily verbs (clearly kernel)

| Command | Daily? | Verdict | Notes |
|---|---|---|---|
| `aida list` | yes | **K** | The primary read |
| `aida show <id>` | yes | **K** | The primary read |
| `aida add` | yes | **K** | The primary write |
| `aida edit <id>` | yes | **K** | The primary write |
| `aida del <id>` | yes | **K** | |
| `aida search <query>` | yes | **K** | FTS5 against the cache |
| `aida grep <pattern>` | sometimes | **K?** | Overlaps with `search`. Could collapse — drop one? |
| `aida comment ...` | yes | **K** | Discussion thread is part of the graph |
| `aida rel ...` | yes | **K** | The relationship graph IS the value-add |
| `aida history` | sometimes | **K?** | Historical projection — kernel-shaped (read against the orphan log). Keep. |
| `aida status` | yes | **K** | Project overview is kernel-shaped |

### MCP server (clearly kernel)

| Command | Daily? | Verdict | Notes |
|---|---|---|---|
| `aida mcp-serve` | yes (by Claude Code) | **K** | The agent surface; without it the pitch evaporates |

### Init & scaffolding (split — kernel knows IDs, module ships templates)

| Command | Daily? | Verdict | Notes |
|---|---|---|---|
| `aida init` | once per project | **K (thin)** | Bootstrap orphan branch + cache + `.aida/config.toml`. Keep this minimal in kernel. |
| `aida scaffold status` | sometimes | **M** | Scaffolder is its own concept; ships the 22 skills, hooks, settings.json templates |
| `aida scaffold preview` | sometimes | **M** | |
| `aida scaffold apply` | sometimes | **M** | |
| `aida scaffold extract` | rarely | **M** | |
| `aida scaffold upgrade` | sometimes | **M** | |
| `aida scaffold diff` | rarely | **M** | |
| `aida upgrade` | rarely | **K?** | Self-update of the binary. Tiny. Probably stays in kernel for first-run UX. |

### Distributed identity (kernel — it's plumbing)

| Command | Daily? | Verdict | Notes |
|---|---|---|---|
| `aida node list/show/acquire/release` | once-per-clone | **K** | Per-clone identity; the kernel can't be hidden if this isn't first-class |
| `aida db merge-gate` | rarely | **K** | ID promotion is core graph maintenance |
| `aida db retire-legacy-ids` | once | **K** | Same |
| `aida db block claim/list/status` | rarely | **K** | Block dispense is kernel ID infrastructure |

### Storage admin (kernel — it's about the store)

| Command | Daily? | Verdict | Notes |
|---|---|---|---|
| `aida db sync` | yes (push), sometimes (pull) | **K** | The orphan-branch round-trip |
| `aida db status` | rarely | **K** | Worktree state |
| `aida db info` | rarely | **K** | |
| `aida db path` | rarely | **K** | |
| `aida db register` | rarely | **K?** | Project registry across SQLite databases. **Legacy multi-project — probably drop now that orphan-branch is canonical.** |
| `aida db migrate` | once-per-legacy-project | **D?** | YAML/SQLite/Postgres migration. Now that we're git-canonical-only, who runs this? Drop. |
| `aida db export-git` | once-per-legacy-project | **D?** | Same — extract from legacy backend → git store. One-shot, can be removed once legacy paths are gone. |
| `aida cache rebuild/status` | rarely | **K** | Cache plumbing |

### Personal workflow (clear module candidates)

| Command | Daily? | Verdict | Notes |
|---|---|---|---|
| `aida queue add/list/remove/move/clear/next/done` | yes (for you) | **M** (`aida-roles`) | Your queue is a workflow concern, not a graph concern. The graph captures the requirement; the queue is what *you* are working on. |
| `aida role enter/end/add/list/show/delete/scaffold` | yes (for you) | **M** (`aida-roles`) | Personas are workflow |
| `aida role prompt/scope ...` | sometimes | **M** (`aida-roles`) | |
| `aida session list/resume/new` | sometimes | **M** (`aida-roles`) | Claude Code session enrichment. Workflow. |
| `aida statusline` | yes (for you, via Claude Code) | **M** (`aida-roles`) | Prompt decoration |

### Dev workflow (separate, but small — bundle?)

| Command | Daily? | Verdict | Notes |
|---|---|---|---|
| `aida dev activate/deactivate/status` | yes (for AIDA contributors) | **M** (`aida-dev`) | The aida-on/aida-off shell wrappers are AIDA-internal, not user-facing |
| `aida dev shell-init` | rarely | **M** (`aida-dev`) | |
| `aida dev serve` | sometimes | **M** (`aida-dev`) | Spawns aida-server + vite |
| `aida dev release` / `dev patch` | rarely | **M** (`aida-dev`) | Release tooling |

### Reporting (module)

| Command | Daily? | Verdict | Notes |
|---|---|---|---|
| `aida report ai-integration` | rarely | **M** (`aida-reports`) | Generated docs |
| `aida user-guide` | rarely | **D?** | Just opens a URL — can be deleted, replaced by README link |

### Trace (split kernel + module)

| Command | Daily? | Verdict | Notes |
|---|---|---|---|
| `// trace:SPEC-ID` comments | yes (passive) | **K** | The convention IS the integration surface |
| `aida trace add/list/remove` | rarely | **M?** | The CLI for managing trace links explicitly. Most usage is via inline comments + `scan` — pure storage of structured links could ship as kernel, but the explicit-CRUD CLI is a module. |
| `aida trace scan` | sometimes | **K** | Scanning source for `// trace:` comments and updating the graph IS kernel value |
| `aida trace sweep` | rarely | **M** | Git-log-based reverse-trace harvesting |

### Integrations (clearly modules)

| Command | Daily? | Verdict | Notes |
|---|---|---|---|
| `aida gitlab ...` | per-team | **M** (`aida-gitlab`) | |
| `aida github ...` | per-team | **M** (`aida-github`) | |
| `aida jira ...` | per-team | **M** (`aida-jira`) | |

### Server / web (clearly modules)

| Command | Daily? | Verdict | Notes |
|---|---|---|---|
| `aida server status/list/get/ping` | per-team | **M** (`aida-web`) | Remote-CLI client |
| `aida-server` binary | per-team | **M** (`aida-web`) | REST + gRPC |
| `aida-web-react/` | per-team | **M** (`aida-web`) | The dashboard |

### Config / metadata (split — small kernel surface, big legacy)

| Command | Daily? | Verdict | Notes |
|---|---|---|---|
| `aida feature add/list/show/...` | rarely | **K** | Feature is part of the graph metadata; small footprint, kernel-shaped. |
| `aida type ...` | rarely | **K** | Custom requirement types are part of the schema — kernel |
| `aida config format/numbering/digits/migrate` | once-per-project | **K?** | ID config. Niche but kernel-adjacent. Possibly fold into `aida init --interactive`. |
| `aida rel-def add/list/remove` | rarely | **K** | Relationship type definitions. Same logic as `type`. |

### Search / explore (overlapping)

| Command | Daily? | Verdict | Notes |
|---|---|---|---|
| `aida search` | yes | **K** | |
| `aida grep` | sometimes | **K?** | Field-filtered grep. Worth keeping but maybe `search --grep` flag? |
| `aida history` | sometimes | **K** | Mentioned above |
| `aida help-all` | rarely | **K** | Trivial; stays |

### Import / export (one-shot operations)

| Command | Daily? | Verdict | Notes |
|---|---|---|---|
| `aida export --format tree --id ...` | rarely | **K** | Tree export of a sub-graph. Useful kernel surface for sharing/templating. |
| `aida import <file>` | rarely | **K** | Same |

---

## aida-core internal modules — kernel/module split

Files in `aida-core/src/` mapped to the same K/M/D scheme.

| File / module | Verdict | Notes |
|---|---|---|
| `lib.rs`, `models.rs`, `object_store.rs`, `git_ops.rs`, `oplog.rs`, `hlc.rs` | **K** | The data model + git plumbing |
| `dispenser.rs`, `node.rs` | **K** | ID infrastructure |
| `db/git_backend.rs`, `db/cache.rs`, `db/cached_git_backend.rs` | **K** | Canonical store + cache projection |
| `db/sqlite_backend.rs`, `db/yaml_backend.rs`, legacy multi-project SQLite | **D** | Deprecated since EPIC-1-001. Drop the code, simplify the trait. |
| `db/postgres_backend.rs` | **M** (`aida-postgres`) | Already feature-flagged. Extract. |
| `meta.rs`, `templates.rs` | **M** (`aida-scaffold`) | The 22 skills + AI prompt templates. Templates and embedded files are scaffolder concerns, not kernel. |
| `scaffolding/` | **M** (`aida-scaffold`) | Whole subtree. |
| `report.rs`, `analytics.rs`, `telemetry.rs` | **M** (`aida-reports`) | Reporting / metrics |
| `docs_review.rs`, `review_config.rs` | **M** (`aida-reports`) | Same |
| `integrations/github`, `integrations/gitlab`, `integrations/jira` | **M** (per-integration repos) | |
| `ai/` | **M** (`aida-ai`) | Claude API client, evaluation, prompts. The MCP server itself stays kernel; *outbound* AI calls (evaluate, find-duplicates, suggest-relationships) are a module. |
| `import.rs`, `export.rs` | **K** (light) | Tree export/import is graph maintenance |
| `conflict.rs` | **K** | Cross-node conflict detection |
| `daemon.rs` | **K?** | Long-running daemon. Currently small — keep in kernel, revisit if it grows. |
| `project.rs`, `registry.rs`, `workspace.rs` | **K?** | Project registration / multi-repo workspace. **Open question — does multi-repo workspace belong in kernel or in `aida-workspace` module?** |
| `storage.rs` | **K** (with surgery) | Legacy facade. After legacy SQLite/YAML drop, this shrinks dramatically. Worth keeping as a thin storage trait. |
| `yaml_helpers.rs` | **K** | Deterministic-serde helpers |

---

## Recommended extraction order

Most-independent-first. Each step ships with the kernel still working.

### Phase 1 — drop the dead code (≈1 session)
- Remove `db/sqlite_backend.rs`, `db/yaml_backend.rs`, `aida db migrate`, `aida db export-git`, `aida db register`, the multi-project registry path
- Simplify the `DatabaseBackend` trait now that there's one production impl
- Remove `aida user-guide` (it's just opening a URL)
- **Result:** kernel measurably smaller; nothing user-facing lost

### Phase 2 — extract `aida-scaffold` (≈1 session)
- New repo: `aida-scaffold` (or stay in-tree as a separate crate, then publish later)
- Moves: `aida-core/src/scaffolding/` + `templates.rs` + `meta.rs` + the 22 skills + commands + hooks + the `aida scaffold` CLI subcommand
- Kernel keeps a `Scaffolder` trait it expects to be implemented; the module provides it
- **Result:** the kernel's claim of "doesn't bundle 22 skills" becomes true

### Phase 3 — extract `aida-roles` (≈1 session)
- New repo: `aida-roles`
- Moves: `aida role`, `aida session`, `aida queue`, `aida statusline` and their state files (`.aida/roles/`, `~/.aida/queue/`)
- Kernel offers no role/queue abstraction; module owns the entire concept
- **Result:** the kernel becomes ID + graph + MCP only

### Phase 4 — extract integration modules (≈1 session each)
- `aida-gitlab`, `aida-github`, `aida-jira`
- Each is a thin CLI overlay + sync state types
- Already feature-flagged in Cargo.toml — extraction is mechanical

### Phase 5 — extract `aida-web` (≈2 sessions, biggest)
- New repo: `aida-web`
- Moves: `aida-server/` (REST + gRPC), `aida-web-react/`, `aida server` CLI subcommand, `aida-generate-types/`
- The kernel has no web concerns
- **Result:** the kernel binary drops by ~half its compile time

### Phase 6 — extract `aida-ai` (≈1 session)
- Outbound AI calls (evaluate, find-duplicates, suggest-relationships, chat)
- Keep the MCP server (inbound, kernel) separate
- **Result:** kernel doesn't depend on Anthropic SDK

### Phase 7 — extract `aida-reports` (≈1 session)
- Reports, analytics, history-as-digest, telemetry
- `aida history` could stay in kernel (it's a read against the oplog, kernel-shaped) or move; flag for discussion

### Phase 8 — `aida-dev` extraction (cleanup)
- Self-hosted only: `aida dev *` commands
- Could just stay in `aida-cli` since it's small; mark `#[clap(hide = true)]` and call it a day

---

## Open questions for mark-up

1. **`aida grep`** — keep alongside `search`, or fold into `search --regex`?
2. **`aida config format/numbering/digits`** — keep as separate command, or fold into `aida init --interactive`?
3. **`aida db register` / multi-project SQLite registry** — drop entirely (recommended) or keep for back-compat?
4. **`workspace.rs` / multi-repo workspace** — kernel (graph spans repos) or `aida-workspace` module?
5. **`aida history`** — kernel (it's a read against oplog) or `aida-reports` (it's "reporting")?
6. **`aida trace add/remove`** — explicit-CRUD-on-trace-links has been mostly unused; do we keep it or rely solely on `// trace:` comments + `aida trace scan`?
7. **`aida-postgres` extraction** — extract now or wait until someone actually uses Postgres?
8. **Rename**: should the kernel binary still be called `aida` or something narrower like `aidactl` to leave space for module CLIs (`aida-roles`, `aida-web`)? My instinct: keep `aida`, modules add subcommands via plugin discovery.
9. **Plugin discovery mechanism** — git-style (`aida-foo` on PATH → `aida foo`)? Or explicit registration via `~/.aida/plugins/`? Or feature-flagged build-time?

---

## What this changes for daily usability

If executed, your typical day with a deployed kernel looks like:

- `aida list / show / add / edit / search / comment / rel` — same as today
- `aida mcp-serve` running for Claude Code — same as today
- `aida queue / role / statusline` — same as today, but installed as a separate package (`aida-roles`)
- `aida-web` (dashboard) — same, installed separately when you want it
- New project setup: `aida init` is bare; `aida scaffold apply --agent claude` if you want the skills/commands; nothing else
- `aida` binary itself is small, builds fast, ships independently of any of the above

**Net daily-usability delta: zero, if the modules-you-use are installed.** Maintenance cost drops significantly.
