# Multi-repo → one `.aida-store`: implementation review + design direction

**Date:** 2026-06-15
**Status:** Review / direction (advisor-authored; draft specs filed, not approved)
**Specs:** SPIKE (deep design) + BUG (live trap) filed alongside this doc
**Audience:** whoever picks up multi-repo work; the operator deciding whether to invest now

## Question

How well does AIDA support **multiple git repos pointing at one `.aida-store`** — a single
requirement view across a group of repos? Worth investing now? Does it warrant a repo-id in
the schema, or a subcomponent dimension that feeds scope?

## Method

Four parallel code-reading passes (store/sibling model; git-linkage single-repo assumptions;
schema scope/component dimensions; node/dispenser/cache/queue multi-clone readiness). Load-bearing
claims (workspace module existence; single-`project_root` scanning) re-verified directly.

## Verdict in one line

The **foundation is strong** (two store-sharing mechanisms + conflict-free distributed IDs already
exist), but the **git-scanning + linkage layer assumes a single repo**, which turns the documented
`--sibling` "multi-repo workspace" mode into a **silent-correctness trap** today. The data model
also has **no repo/component dimension** to disambiguate cross-repo linkage.

## What already works (the expensive parts)

- **Store sharing — two mechanisms.**
  - `aida init --sibling` → `handle_init_distributed_sibling` (aida-cli/src/main.rs ~19876): writes
    `.aida/config.toml [deployment] store_path = "../aida-store"`, store as a sibling git repo.
  - **Workspace manifest** — `aida-core/src/workspace.rs` (`WorkspaceManifest` line 22, `init_workspace`
    line 114; CLI `WorkspaceInit` at cli.rs:2183). `.aida-workspace` lists repos + shared store path.
- **Store resolution is multi-repo-correct.** `detect_distributed_store_from` (main.rs ~10780) walks
  up for `.aida/config.toml`, resolves `store_path` relative to config dir, BUG-331 `git-common-dir`
  fallback. `AIDA_STORE` override checked first. Tests: `detect_distributed_store_resolves_sibling_store_outside_repo`.
- **The hard distributed-systems problem is solved.** Conflict-free ID allocation across many writers:
  node-namespaced dispenser (`FR-7-001`) + HLC total ordering `(wall_time, counter, node_id)`
  (hlc.rs) + per-node local sequences, no coordination needed (dispenser.rs). Per-clone cache
  (`.aida/cache.db`, rebuildable, stale-detect by HEAD SHA — cache.rs). **N clones = N caches over 1
  store; ids never collide by construction.**

## What's broken / missing (the gap)

### 1. Every git-aware op assumes a single `project_root` (CORRECTNESS)

Verified by function signatures + scan sites:

| Function | Site | Cross-repo failure |
|---|---|---|
| `auto_bump_done_to_completed` | main.rs ~82872 | Spec whose code merged in repo B **never auto-completes** when pulling from repo A. Silent. |
| `handle_db_reconcile_status` | main.rs ~83339 | The *recovery* tool silently no-ops cross-repo — propagates the miss. |
| `collect_git_linkage(_opts)` | main.rs 71149/71162 | `aida show <ID>` shows 0 commits/files for work in a sibling repo. Breaks core trace/recall. |
| `scan_trace_graph` | main.rs ~69773 | Trace comments in sibling repos never indexed. |
| forge/PR detection (`origin_url`, `change_lookup_for_branch`) | forge.rs ~550; main.rs ~45632 | PR/branch lookups hit only the local origin; wrong when repos use different forges/orgs. |

Net: the **store layer supports N repos; the scanning layer does not**. `--sibling` is documented as
"recommended for multi-repo workspaces" — so this is a doc-vs-behavior defect, not a future feature.

### 2. No repo/component dimension in the schema (MODEL)

- `Requirement` has no field for repo/component. Closest: `feature` (ID-prefix, semantic mismatch),
  `tags` (freeform, no enforcement), `folder` (logical grouping), node-id (implicit in `spec_id`).
- `ImplementationInfo.completion_sha` (models.rs ~3040) is a **bare SHA** — ambiguous across repos.
- `GitLinkage.branch` / `shipped_pr` (main.rs ~71055) carry **name/number only**, no repo/forge.

### 3. Queue is user-keyed, conflict-prone across clones (KNOWN)

Queue keyed off shell USER (BUG-89), stored at `registry/queues/{user}.yaml` in the shared store.
Two clones, same user → contention on one queue file (TASK-618, BUG-220 history). Pre-existing;
multi-repo amplifies it. Out of scope for the first tier but on the radar.

## Direction: three tiers (split cheap-correctness from deep-model)

- **Tier 1 — stop the trap (soon, bounded).** Make the scanners iterate the workspace manifest's
  repos, or at minimum **fail loudly** when a spec's referencing repo isn't visible (vs silent miss).
  Highest priority because it's a live correctness bug the moment `--sibling` is used as documented.
- **Tier 2 — repo-qualify linkage (schema).** Optional `repo`/`component` field on `Requirement`;
  qualify `completion_sha`/branch/PR with a repo identifier. Backward-compatible (`None` = single-repo).
- **Tier 3 — subcomponent → scope (SPIKE, the rabbit hole).** A first-class component/repo dimension
  feeding `list`/`search`/queue filtering and forward into SPIKE-10 subsystem-scoped advisors. Touches
  scope + queue routing + advisor model at once — think it through before committing.

## Anti-overengineering note

Tier 1 is a bug fix and should ship independent of the model debate. Tiers 2–3 should NOT be built
speculatively — gate Tier 3 on a real multi-repo project actually in use (the operator named the use
case as prospective). The dispenser/cache foundation means we are NOT racing to avoid a costly
retrofit; the expensive part is already paid.
