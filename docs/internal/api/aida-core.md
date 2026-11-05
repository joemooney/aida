# aida-core public API surface

<!-- trace:STORY-266 | ai:claude -->

**Status:** documented contract (STORY-266 — standalone API-hygiene, decoupled
from the rejected EPIC-29 repo split).
**Scope:** the load-bearing types/functions of the `aida-core` crate that the
in-tree consumers depend on, with one line per item, plus an explicit
intended-public vs. internal split.

This file is the authoritative companion to the crate-level rustdoc in
`aida-core/src/lib.rs`. When you change what's re-exported from the crate root,
update this file in the same change so the two don't drift.

---

## Who consumes `aida-core` as a Rust library

Only two workspace crates link against `aida-core`:

| Consumer | What it is | How it uses the API |
|----------|-----------|---------------------|
| `aida-cli` | the `aida` binary | the heaviest consumer — store, backends, ID system, graph, scaffolding, integrations. **Includes the MCP server** (`aida-cli/src/mcp.rs`), which consumes a deliberately narrow slice: `Requirement`, `RequirementsStore`, `RequirementType`, `RequirementStatus`, `RequirementPriority`, `RelationshipType`, `Comment`, `Storage`, `PuntCategory`, `graph_walk`, `mailbox`, `forbidden_attention_transition`. |
| `aida-server` | REST + gRPC service (port 8080) | a narrower slice: the data model (`Requirement`/`RequirementsStore`/the `models` enums), the `db` backends, `ai`, `analytics`, and `determine_requirements_path`. |

**`aida-tui` does NOT depend on `aida-core`.** The PTY-hosting terminal shell
(EPIC-26) talks to the `aida` CLI subprocess and reads AIDA status/session files
on disk — it links no `aida-core` symbols. So although STORY-266 originated as a
TUI-extraction prerequisite, the actual Rust-API contract serves the
CLI/MCP/server consumers. The audit below reflects ground truth
(`grep -rh "aida_core::" aida-cli/src aida-server/src`), not the original
TUI-centric framing.

---

## Tier 1 — the stable core (always compiled, no feature gate)

These are the items downstream code should treat as the durable contract. They
have no `#[cfg(feature = …)]` gate, so they're available in every build of the
crate.

### Data model

| Item | Kind | Description |
|------|------|-------------|
| `Requirement` | struct | A single spec node — id, type, status, priority, title, description, tags, relationships, comments, history, implementation info, etc. |
| `RequirementsStore` | struct | The in-memory requirement graph: all requirements + relationships + queue entries + config. The root object backends load into and write from. |
| `RequirementType` | enum | The spec taxonomy: `Functional`, `NonFunctional`, `System`, `User`, `Bug`, `Epic`, `Story`, `Task`, `Spike`, `Sprint`, `Folder`, `Meta`, `Doc`. |
| `RequirementStatus` | enum | The lifecycle state: `Draft`, `Approved`, `Planned`, `InProgress`, `Done`, `Completed`, `Rejected`, `NeedsAttention`, … |
| `RequirementPriority` | enum | `High` / `Medium` / `Low`. |
| `RelationshipType` | enum | Typed edges between specs (`ParentChild`, `BlockedBy`, `Blocks`, `DependsOn`, …). |
| `Relationship` | struct | A single typed edge instance (source, target, type). |
| `Comment` | struct | A comment on a requirement (author, timestamp, body, reactions). |
| `QueueEntry` | struct | A work-queue item keyed by user_id (the role queue). |
| `ImplementationInfo` | struct | Git linkage for a spec: commits, files, branch, PR. |
| `AttentionReason` | enum | Why a spec was parked `NeedsAttention` (the punt/escalation taxonomy). |
| `PuntCategory` | enum | Classification of a design-fork punt. |
| `FailureReason` | enum | Why a phase/drain step failed. |
| `UrlOpenMode` | enum | How a URL link should be opened. |
| `forbidden_attention_transition` | fn | Status-transition guard (STORY-332): is a given `NeedsAttention` transition forbidden? |

The remaining `models` types (`HistoryEntry`, `FieldChange`, `TraceLink`,
`Baseline`, `CustomTypeDefinition`, the AI-prompt-config structs, the GitLab-link
structs, the meta-prefix consts, …) are all re-exported at the crate root and
documented in `aida_core::models`. They form the rest of the data model and are
public-by-intent.

### Graph + pickability

| Item | Kind | Description |
|------|------|-------------|
| `graph_walk::walk` | fn | Walk relationship edges from one spec in a `Direction`, returning the reachable set. |
| `graph_walk::walk_union` | fn | Walk from multiple roots, unioning the results. |
| `graph_walk::status_rollup` | fn | Roll up child statuses for an epic/story (the `--tree` view). |
| `graph_walk::Direction` | enum | Edge-traversal direction (e.g. blocked-by vs. blocks). |
| `graph_walk::{GraphEdge, GraphResult, StatusRollup}` | structs | Result types for the walks above. |
| `pickability::pickability` | fn | "Is this spec workable now?" given its blocked-by edges and store state. |
| `pickability::blocked_by_incomplete` | fn | Predicate: does this spec have incomplete blockers? |
| `pickability::pickability_reason_label` | fn | Human label for a `BlockedReason`. |
| `pickability::{Pickability, BlockedReason}` | enums | The pickability verdict + the reason it's blocked. |

### Distributed ID system

| Item | Kind | Description |
|------|------|-------------|
| `Hlc`, `HlcTimestamp` | struct | Hybrid logical clock — conflict-free ordering across machines. |
| `Dispenser`, `DispenserState`, `IdMode`, `MemoryDispenser` | trait/structs | Spec-ID minting abstraction + in-memory impl. |
| `NodeConfig`, `NodeRegistry`, `BlockRegistry`, `AgreedIdBlock` | structs | Per-node identity + the block-allocation registry that hands out unique ID ranges offline. |
| `IdCounterScope`, `IdFormatPolicy`, `DeploymentMode` | enums | ID-numbering policy knobs. |
| `BlockAllocationConfig`, `BlockAllocationTypeConfig` | structs | Block-allocation configuration. |

---

## Tier 2 — the `native` surface (default `native` feature: filesystem + git)

Gated behind `#[cfg(feature = "native")]`. This is on by default and is what
every real `aida` invocation uses, but a downstream crate can opt out for a
no-filesystem build.

### Storage backends

| Item | Kind | Description |
|------|------|-------------|
| `DatabaseBackend` | trait | The backend contract — load/save/list/search a `RequirementsStore`. All backends implement it. |
| `CachedGitBackend` | struct | **The default backend**: git-canonical YAML store + SQLite read-cache. |
| `GitBackend` | struct | The raw git-canonical backend (no cache layer). |
| `Cache` | struct | The SQLite read-projection (rebuildable, gitignored). |
| `SqliteBackend`, `YamlBackend` | structs | Legacy centralized backends (deprecated `--centralized` path). |
| `create_backend`, `open_or_create` | fn | Backend entry points — construct/open the configured backend. |
| `BackendType`, `DatabaseConfig` | enum/struct | Which backend + its config. |
| `ListFilter`, `ArchiveFilter`, `RequirementSummary` | structs | List/filter inputs + the lightweight cache row type. |
| `UpdateResult`, `VersionConflict` | struct/enum | Write outcome + optimistic-concurrency conflict. |
| `cache_lock_info_path`, `read_cache_lock_info`, `CacheLockInfo` | fn/struct | Cache-lock introspection. |
| migration helpers | fn | `migrate_sqlite_to_yaml`, `migrate_yaml_to_sqlite`, `export_to_json`, `import_from_json` (+ `migrate_{to,from}_postgres` under `postgres`). |

### Object store / storage facade

| Item | Kind | Description |
|------|------|-------------|
| `Storage` | struct | Higher-level facade over the object store: locking, save/add, session info. (Consumed by the MCP server.) |
| `object_store` (module) | module | Lower-level YAML object store (one file per spec). |
| `AddResult`, `SaveResult`, `EditLock`, `SessionInfo` | structs | Storage operation results + lock state. |
| `ConflictInfo`, `ConflictResolution`, `FieldConflict`, `LockFileInfo`, `StorageError` | types | Conflict + error reporting for storage ops. |

### Scaffolding, templates, project, reporting

| Item | Kind | Description |
|------|------|-------------|
| `Scaffolder`, `ScaffoldConfig`, `ScaffoldArtifact`, `ScaffoldPreview`, `ScaffoldStatus`, `ScaffoldError` | types | `aida init` scaffolding engine + preview/status. |
| `slot_merge`, `slots_for_file`, `wrap_with_aida_header`, `aida_managed_diff_slice` | fn | AIDA-managed-block merge machinery (idempotent re-scaffold). |
| `DiffSlice`, `SlotChange`, `SlotChangeKind`, `FileCategory`, `ProjectType` | types | Scaffold diff + categorization types. |
| `TemplateLoader`, `TemplateInfo`, `TemplateSource`, `get_embedded_templates`, `get_template_categories`, `get_templates_by_category` | type/fn | Embedded-template access (build-time embedded via `build.rs`). |
| `determine_requirements_path`, `check_migration_status`, `MigrationCheck` | fn/struct | Project-root + storage-mode detection. |
| `Registry`, `get_config_dir`, `get_registry_path`, `get_templates_dir` | struct/fn | Global config/registry paths. |
| `ReportGenerator`, `ReportFormat`, `check_scaffold_status`, `TraceabilityStats`, … | type/fn | Reporting surface (`report` module re-exports). |
| `UserPreferences` | struct | Per-user config. |

### Atomic FS + supporting modules

| Item | Kind | Description |
|------|------|-------------|
| `read_atomic`, `write_atomic` | fn | Crash-safe file read/write. |
| `git_ops` (module) | module | Git plumbing used across the lifecycle (native-only). |
| `rebase` (module) | module | Two-leg rebase machinery. |
| `mailbox` (module) | module | Inter-agent message substrate (`Message`, `Recipient`, `inbox_for`). Consumed by the MCP server. |
| `meta` (module) | module | META-requirement seeding + default AI prompt templates. |
| `deps_sweep` (module) | module | Dependency-sweep analysis. |
| `conflict` (module) | module | Conflict detection/resolution types. |
| `workspace` (module) | module | Workspace-config handling. |
| `analytics` (module) | module | Analytics/rollup helpers (consumed by `aida-server`). |
| `ai` (module) | module | AI evaluation client + types (`AiClient`, `AiMode`, `BackgroundEvaluator`, …). |

---

## Tier 3 — feature-gated integration surfaces

Each issue-tracker integration is behind its own feature flag and is **off** in
the default build of `aida-core`; `aida-cli` enables them.

| Feature | Re-exported items | Description |
|---------|-------------------|-------------|
| `github` | `GitHubClient`, `GitHubConfig`, `GitHubIssue`, `GitHubCreateIssueRequest`, `GitHubUpdateIssueRequest`, `GitHubIssueFilter`, `GitHubLabelConfig`, … | GitHub Issues sync client + config. |
| `gitlab` | `GitLabClient`, `GitLabConfig`, `GitLabIssue`, `SyncConfig`, `FieldSyncRules`, … | GitLab Issues sync client + config + field-sync rules. |
| `jira` | `JiraClient`, `JiraConfig`, `JiraIssue`, `JiraSearchResults`, `text_to_adf`, the `JiraCreate*`/`Jira*Ref` request types | Jira sync client + config + ADF conversion. |
| `postgres` | `db::PostgresBackend`, `migrate_to_postgres`, `migrate_from_postgres` | PostgreSQL backend (opt-in; not for new code paths). |

---

## What is intentionally NOT public API

`aida-core` exposes most of its modules as `pub mod` so that `aida-cli` — a
same-workspace sibling, not an external crate — can reach in. That `pub` is a
workspace convenience, **not** a stability promise. Treat the following as
internal; they may change without a deprecation cycle:

- **Anything not re-exported at the crate root and not listed above.** The crate
  root (`aida_core::*`) plus this document is the contract. Reaching into a
  module path that isn't named here (e.g. internals of `db::cache`,
  `object_store` internals, `dispenser` file-format details) is using internals.
- **`oplog`, `telemetry`, `docs_review`, `review_config`, `export`, `import`
  internals, `yaml_helpers`, `block_allocation` internals, `registry`
  internals, `daemon`** — these are `pub` for cross-crate wiring but are
  implementation detail. The few items from them that *are* contract (e.g.
  `read_atomic`/`write_atomic`, the import `execute_import`/`ImportConfig`
  family used by `aida import`) are re-exported at the crate root and listed
  above; the rest is internal.
- **Concrete backend internals.** Downstream code should depend on the
  `DatabaseBackend` trait + `create_backend`/`open_or_create`, not on the
  private fields or helper methods of `CachedGitBackend`/`GitBackend`.
- **The `native`/feature-gated split itself.** A no-`native` build deliberately
  drops the filesystem/git surface; do not assume Tier-2/Tier-3 items exist in
  every build configuration.

### Why no visibility was tightened in this change

STORY-266 is documentation-first by decision. The `pub mod` items above are all
consumed by `aida-cli`/`aida-server` somewhere, so demoting them to
`pub(crate)` would break the build with no offsetting benefit while the crates
remain in one workspace. The contract is enforced by *convention + this
document* (the crate-root re-export list is the public set) rather than by the
compiler. If `aida-core` is ever published or split out, this document is the
checklist for what must stay `pub` and what can be sealed.
