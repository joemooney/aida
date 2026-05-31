# Implementation plan — `aida graph`: cross-spec relationship queries

**Date:** 2026-05-31 · **Specs:** STORY-489 (parent SPIKE-45 P2) · **Status:** Sketch — needs sign-off on the MCP-contract slice · **Complexity:** Medium

> The flagship "outsmart the flat-markdown SDD tools" capability. AIDA stores a typed relationship graph; Spec Kit and Kiro store per-feature markdown that **structurally cannot** answer "what is blocked across this epic" or "what does changing X impact." This plan makes that latent advantage a demonstrable command. Authored from a two-agent inventory of the existing relationship data + surface layers (2026-05-31).

## Approach

Add a centralized, cycle-safe **graph-walk primitive** in `aida-core` (the one thing the codebase lacks — it has only single-step `get_relationships_by_type`), then expose it through a read-only CLI subcommand and a mirroring MCP tool. Three query modes cover the demo-critical questions:

- **`--blocked-by` / `--blocks`** — transitive closure over `BlockedBy`/`Blocks` (the "what's the full blocking chain" query pickability only answers one hop of).
- **`--tree`** — `Parent`/`Child` descendants with a **status rollup** (Completed/InProgress/blocked counts) — the epic-rollup view.
- **`--impact`** — reverse closure: every spec that transitively depends on / is blocked by the target (the "what breaks if I touch this" query).

```
aida graph STORY-489 --blocked-by        aida-core::graph_walk (NEW)
        │                                   ├─ walk(store, root, &[rel_type], dir, depth) -> GraphResult
        ▼                                   │     visited-set BFS (reuse would_create_cycle pattern)
  CLI handler  ──────────────────────────▶ │     atomic step = get_relationships_by_type (EXISTING)
 (cli.rs Graph variant +                    │
  main.rs handle_graph_command)             └─ rollup(store, descendants) -> StatusRollup
        │
        ▼  (same primitive)
  MCP query_graph tool  ◀── CONTRACT ADDITION — sign-off gate before this slice
```

The CLI + core slices are non-contract and buildable immediately. The MCP `query_graph` tool **adds to the MCP contract** → requires master/operator sign-off before that slice opens (per the one-master-advisor discipline). Build core+CLI first; land MCP as a follow-up PR once signed off.

## Key decisions

1. **New `aida-core/src/graph_walk.rs` module**, not inline in the CLI — so the MCP tool and any future surface share one cycle-safe walker. The atomic operation is the existing `RequirementsStore::get_relationships_by_type`; the walker only adds traversal + a visited-set.
2. **Cycle safety is mandatory and centralized.** `collect_descendant_uuids` (export.rs) has *no* cycle guard; `BlockedBy`/`Blocks` have *no* cycle validation at add-time (only Parent/Child do). So the walker MUST carry its own `HashSet<Uuid>` visited-set — model it on `would_create_cycle` (models.rs). A `BlockedBy` cycle is possible in the store and must not hang the walk.
3. **Read-only, additive.** No new relationship types, no mutation, no change to pickability or export. Pure projection over existing edges.
4. **Mode flags are mutually exclusive**, defaulting to `--tree` when none given (the most intuitive "show me this spec's graph"). `--depth N` bounds traversal (default unbounded for blocked-by/impact since chains are short; default a sane cap for tree).
5. **`--json` from day one** (read-command convention) so agents/scripts consume it — this is half the moat (MCP/agent-queryable), not just human output.
6. **SPEC-IDs in output are developer-facing here** (this is a developer query tool), but keep stdout clean of internal noise per the user-facing-text conventions.

## Files (build order)

1. **`aida-core/src/graph_walk.rs`** (NEW) — `pub fn walk(store, root: Uuid, rel_types: &[RelationshipType], direction: Direction, max_depth: Option<usize>) -> GraphResult` with a visited-set BFS; `pub fn status_rollup(store, ids: &[Uuid]) -> StatusRollup` (counts by RequirementStatus). `GraphResult { edges: Vec<(Uuid, RelationshipType, Uuid)>, nodes: Vec<Uuid> }`.
2. **`aida-core/src/lib.rs`** — `pub mod graph_walk;` + re-exports.
3. **`aida-cli/src/cli.rs`** — add `Graph { id, blocked_by, blocks, tree, impact, depth, json }` variant to the `Command` enum (match the `Show`/`queue progress` read-query house style).
4. **`aida-cli/src/main.rs`** — `handle_graph_command(...)`: resolve root via `get_requirement_by_spec_id`/`matches_id`, call `graph_walk::walk`, render with the `colored` conventions from `show_requirement` (cyan rel-type, yellow spec-id, plain title) or emit JSON. Add the dispatch match arm.
5. **`aida-cli/src/mcp.rs`** *(CONTRACT SLICE — after sign-off)* — `query_graph` tool descriptor (mirror `add_relationship`'s descriptor shape) + `tool_query_graph` handler calling the same `graph_walk::walk`. Register in `tool_descriptors()`.

## Critical files

- `aida-core/src/models.rs` — `RelationshipType` (Parent/Child/BlockedBy/Blocks/Verifies/VerifiedBy/References/Duplicate/Custom), `Relationship { rel_type, target_id, .. }`, `Requirement.relationships`, `get_relationships_by_type`, `would_create_cycle` (visited-set precedent), `display_id`/`matches_id`/`get_requirement_by_spec_id`.
- `aida-core/src/pickability.rs` — `pickability()`: the one-hop BlockedBy logic the `--blocked-by` mode generalizes to transitive. Match its dangling-edge defensiveness (treat unresolvable targets as a surfaced edge, don't panic).
- `aida-core/src/export.rs` — `collect_descendant_uuids` (Child DFS, no cycle guard) — the anti-pattern to fix by centralizing the guarded walker.
- `aida-cli/src/main.rs` `show_requirement` — relationship-rendering format to match.

## Reusable helpers (don't reimplement)

- `RequirementsStore::get_relationships_by_type(id, rel_type) -> Vec<Uuid>` — the atomic traversal step.
- `RelationshipType::inverse()` — for `--impact` reverse walk (walk `Blocks` to find what a spec blocks, etc.).
- `would_create_cycle`'s `HashSet` + stack pattern — copy for the visited-set.
- `get_requirement_by_id` / `display_id` — node resolution + rendering.
- `colored::Colorize` conventions + the `--json` flag pattern from `aida list`/`aida usage`.

## Risks + gotchas

- **Relationship cycles WILL hang an unguarded walk** — `BlockedBy`/`Blocks` have no add-time cycle validation. Visited-set is non-negotiable. (Test a deliberate A→B→A blocked-by cycle.)
- **Dangling edges** — `target_id` may not resolve (doctor repairs these lazily). Surface as `(unresolved <uuid>)` like `show_requirement`/pickability do; never panic.
- **Direction confusion** — `BlockedBy` vs `Blocks` are inverses stored possibly only one-way (bidirectional add isn't guaranteed for all historical data). For `--impact`, walk BOTH the stored `Blocks` edges AND reverse-scan for `BlockedBy` edges pointing at the target, to be robust to one-directional storage.
- **MCP contract** — `query_graph` is a new tool surface; do NOT land it without sign-off (one-master-advisor).

## Tests (named)

- `graph_walk::tests::blocked_by_transitive_closure` — A blocks B blocks C ⇒ querying C's `--blocked-by` returns {A,B}.
- `graph_walk::tests::cycle_does_not_hang` — A↔B blocked-by cycle terminates with visited-set.
- `graph_walk::tests::tree_rollup_counts_by_status` — epic with mixed-status children yields correct StatusRollup.
- `graph_walk::tests::impact_reverse_closure` — what transitively depends on a root.
- `graph_walk::tests::dangling_target_is_surfaced_not_panicked`.
- CLI: a test asserting `--blocked-by` and `--tree` are mutually exclusive / default-to-tree.

## Verification

```bash
cargo test -p aida-core graph_walk
cargo test -p aida-cli graph        # CLI handler tests
cargo build -p aida-cli && cargo fmt --all -- --check
# Manual demo (the moat shot): pick an epic with children + a blocked spec
aida graph <EPIC> --tree            # status rollup across descendants
aida graph <SPEC> --blocked-by      # full transitive blocking chain
aida graph <SPEC> --impact --json   # machine-readable reverse closure
```

## Followups

- MCP `query_graph` tool (contract slice — separate PR after sign-off).
- `aida://requirements/graph` MCP resource (alongside the existing `tree` resource).
- `--format dot` for Graphviz export (visual moat demo for the README/marketing phase).
- Wire `DependsOn`/`dependency_of` (defined in `RelationshipDefinition::defaults` but not yet pickability-wired) into the walk.

## Related

- Parent: SPIKE-45 (capability roadmap, P2). Sibling shipped: STORY-490 (P5 drain legibility).
- Positioning: `docs/positioning/vs-kiro.md`, `vs-spec-kit.md` — this command is what those pages claim AIDA can do and they can't.
- Synthesis: `docs/competitive-analysis/2026-05-31-round2-moat-gaps-moves.md` (P2 = the flagship outsmart move).
