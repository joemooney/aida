//! Cross-spec relationship graph traversal.
//!
//! AIDA stores a typed relationship graph (`Requirement.relationships`), but
//! until now the only traversal primitive was the single-step
//! [`RequirementsStore::get_relationships_by_type`]. Answering "what is blocked
//! across this whole epic?" or "what transitively depends on this spec?" — the
//! queries a flat per-feature markdown tool (Spec Kit, Kiro) structurally
//! cannot answer — needs a *transitive*, cycle-safe walk. This module is that
//! walk: a visited-set BFS over any set of relationship types, plus a status
//! rollup for epic-level views. It is the shared primitive behind the
//! `aida graph` CLI command and (post-sign-off) the `query_graph` MCP tool.
//!
//! Cycle safety is mandatory: `BlockedBy`/`Blocks` edges have no add-time cycle
//! validation (unlike Parent/Child), so a cycle is representable in the store
//! and the walk MUST terminate on it — the visited-set guarantees that.
//!
//! trace:STORY-489 | ai:claude

use crate::models::{RelationshipType, RequirementStatus, RequirementsStore};
use std::collections::{HashMap, HashSet, VecDeque};
use uuid::Uuid;

/// Which way to follow an edge during a walk.
///
/// `Outgoing` follows a requirement's own `relationships` (e.g. from a spec to
/// the things it is `BlockedBy`). `Incoming` is the reverse adjacency — it
/// scans the store for requirements whose relationship of the given type points
/// *at* the current node (e.g. "what is blocked by this spec"), which is robust
/// to relationships that were only stored one-directionally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Outgoing,
    Incoming,
}

/// A single resolved edge in a walk result: `from -(rel_type)-> to`, oriented
/// in the direction the walk traversed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEdge {
    pub from: Uuid,
    pub rel_type: RelationshipType,
    pub to: Uuid,
}

/// The outcome of a [`walk`]: every node reachable from the root (excluding the
/// root itself), in BFS discovery order, plus the edges traversed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphResult {
    pub root: Uuid,
    /// Reachable nodes excluding the root, in BFS discovery order.
    pub nodes: Vec<Uuid>,
    pub edges: Vec<GraphEdge>,
}

/// Transitive walk from `root` following `rel_types` in `direction`, guarded by
/// a visited-set so relationship cycles terminate. `max_depth` of `None` walks
/// to exhaustion; `Some(n)` stops after `n` hops from the root.
///
/// The atomic step is the existing single-hop lookup; this only adds traversal
/// plus cycle safety, so it stays consistent with how the rest of AIDA reads
/// the graph. trace:STORY-489
pub fn walk(
    store: &RequirementsStore,
    root: Uuid,
    rel_types: &[RelationshipType],
    direction: Direction,
    max_depth: Option<usize>,
) -> GraphResult {
    let mut result = GraphResult {
        root,
        ..Default::default()
    };
    let mut visited: HashSet<Uuid> = HashSet::new();
    visited.insert(root);
    let mut queue: VecDeque<(Uuid, usize)> = VecDeque::new();
    queue.push_back((root, 0));

    while let Some((node, depth)) = queue.pop_front() {
        if max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        for (rel_type, neighbor) in neighbors(store, node, rel_types, direction) {
            result.edges.push(GraphEdge {
                from: node,
                rel_type,
                to: neighbor,
            });
            // The visited-set is what makes a cyclic graph safe: a node is
            // enqueued at most once, so A->B->A terminates.
            if visited.insert(neighbor) {
                result.nodes.push(neighbor);
                queue.push_back((neighbor, depth + 1));
            }
        }
    }
    result
}

/// Walk several `(rel_types, direction)` specs from the same `root` and merge
/// the reachable node sets (deduped, first-seen order; edges concatenated).
///
/// For a query whose answer spans more than one edge orientation. The driving
/// case (BUG-411) is `impact` — "specs blocked by the root" — which is
/// reachable EITHER via an incoming `BlockedBy` edge (`X.BlockedBy → root`) OR
/// an outgoing `Blocks` edge (`root.Blocks → X`), because a relationship may be
/// stored only one-directionally. A single-orientation walk silently misses the
/// other form. Each spec's walk is independently cycle-safe; note the union is
/// NOT a fully transitive *mixed*-orientation walk (a path alternating
/// orientations per hop is not followed) — sufficient for the impact query
/// where each chain is orientation-consistent. trace:BUG-411 | ai:claude
pub fn walk_union(
    store: &RequirementsStore,
    root: Uuid,
    specs: &[(Vec<RelationshipType>, Direction)],
    max_depth: Option<usize>,
) -> GraphResult {
    let mut merged = GraphResult {
        root,
        ..Default::default()
    };
    let mut seen: HashSet<Uuid> = HashSet::new();
    for (rel_types, direction) in specs {
        let sub = walk(store, root, rel_types, *direction, max_depth);
        merged.edges.extend(sub.edges);
        for nid in sub.nodes {
            if seen.insert(nid) {
                merged.nodes.push(nid);
            }
        }
    }
    merged
}

// ---------------------------------------------------------- subtree membership
//
// TASK-1074: `descendant_ids` (the cache CTE behind `aida focus`) and
// `aida graph --tree` used to compute epic-subtree membership two different
// ways and disagreed (e.g. EPIC-54: 43 vs 44). The graph walk was a
// direction-AGNOSTIC `walk_union([Child, Parent])` connected-component walk, so
// from a subtree node it climbed the reciprocal edge UP to that node's OTHER
// (same-rank) parent and counted it as a member — one rejected node (STORY-698)
// that is a *parent* of a child of the epic, not a descendant of it. The cache
// CTE, by contrast, only understood ONE of the two orientation conventions the
// store carries, so it under-counted convention-A epics entirely.
//
// The fix routes BOTH surfaces through ONE rule: orient every hierarchy edge
// parent->child (by type rank, `rel_type` breaking same-rank ties), then walk
// strictly downward. `orient_hierarchy_edge` is the atom; `oriented_hierarchy_edges`
// is the whole-store edge set the cache materializes into `hierarchy_edges`;
// `subtree_ids` is the downward closure both the graph and rollup use.

/// Orient a single hierarchy edge `src -(rel_type)-> tgt` into
/// `(parent_id, child_id)` under the one shared subtree rule. The lower-rank
/// endpoint (see [`crate::models::RequirementType::hierarchy_rank`]) is the
/// parent — an Epic parents a Story parents a Task/Bug/Spike — so the edge is
/// oriented correctly no matter which endpoint recorded it or which of the two
/// historical conventions it used (`epic --Parent--> child` /
/// `child --Child--> parent`, or the inverse `epic --Child--> child` /
/// `child --Parent--> parent`). Only same-rank endpoints (Story↔Story,
/// FR↔FR, …), where type gives no signal, fall back to the `rel_type`
/// convention "I am X to target": a `Parent` edge names the SOURCE as parent, a
/// `Child` edge names the TARGET as parent. Returns `None` for a non-hierarchy
/// edge.
// trace:TASK-1074 | ai:claude
pub fn orient_hierarchy_edge(
    src_id: Uuid,
    src_rank: u8,
    rel_type: &RelationshipType,
    tgt_id: Uuid,
    tgt_rank: u8,
) -> Option<(Uuid, Uuid)> {
    match rel_type {
        RelationshipType::Parent | RelationshipType::Child => {}
        _ => return None,
    }
    Some(match src_rank.cmp(&tgt_rank) {
        // Source is higher in the tree (smaller rank) ⇒ source is the parent.
        std::cmp::Ordering::Less => (src_id, tgt_id),
        // Target is higher ⇒ target is the parent.
        std::cmp::Ordering::Greater => (tgt_id, src_id),
        // Equal rank — no type signal; trust the rel_type's stated role.
        std::cmp::Ordering::Equal => match rel_type {
            RelationshipType::Parent => (src_id, tgt_id), // "src is Parent of tgt"
            _ => (tgt_id, src_id),                        // "src is Child of tgt"
        },
    })
}

/// The whole-store set of hierarchy edges, each oriented `(parent_id, child_id)`
/// by [`orient_hierarchy_edge`] and deduped. This is the single substrate the
/// cache materializes into its `hierarchy_edges` table (so the `descendant_ids`
/// CTE walks the very same edges [`subtree_ids`] does) and the input to the
/// in-memory downward closure.
// trace:TASK-1074 | ai:claude
pub fn oriented_hierarchy_edges(store: &RequirementsStore) -> Vec<(Uuid, Uuid)> {
    let rank_of: HashMap<Uuid, u8> = store
        .requirements
        .iter()
        .map(|r| (r.id, r.req_type.hierarchy_rank()))
        .collect();
    let mut edges: Vec<(Uuid, Uuid)> = Vec::new();
    let mut seen: HashSet<(Uuid, Uuid)> = HashSet::new();
    for req in &store.requirements {
        let src_rank = req.req_type.hierarchy_rank();
        for rel in &req.relationships {
            // An edge to a spec not in the store defaults the target to
            // child-rank (2): a dangling parent-ref should not invert the edge.
            let tgt_rank = rank_of.get(&rel.target_id).copied().unwrap_or(2);
            if let Some(edge) =
                orient_hierarchy_edge(req.id, src_rank, &rel.rel_type, rel.target_id, tgt_rank)
            {
                if seen.insert(edge) {
                    edges.push(edge);
                }
            }
        }
    }
    edges
}

/// The transitive downward subtree of `root` — the single shared
/// subtree-membership computation behind `aida graph --tree` (via
/// [`hierarchy_tree`]), `aida focus`'s rollup (via the cache's `descendant_ids`
/// CTE, which walks the identical [`oriented_hierarchy_edges`]),
/// `queue list --epic`, and the epic-close rollup ([`child_status_rollup`]).
/// Every hierarchy edge is oriented parent->child, then walked downward from
/// `root` with a visited-set (cycle-safe). Returns nodes in BFS order EXCLUDING
/// the root (matching the [`walk`] contract) plus the traversed edges emitted as
/// `Child` edges (`from`=parent, `to`=child) so [`tree_layout`] nests them
/// correctly. `max_depth` bounds the hops as in [`walk`].
///
/// Unlike the old `walk_union([Child, Parent])` tree walk this does NOT climb
/// from a descendant UP to its OTHER parents, so a same-rank second parent (the
/// STORY-698 / EPIC-54 leak) is correctly excluded.
// trace:TASK-1074 | ai:claude
pub fn subtree_ids(store: &RequirementsStore, root: Uuid, max_depth: Option<usize>) -> GraphResult {
    let mut children: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (parent, child) in oriented_hierarchy_edges(store) {
        children.entry(parent).or_default().push(child);
    }
    downward_closure(root, &children, max_depth)
}

/// BFS the pre-oriented `parent -> [children]` adjacency downward from `root`.
/// Shared by [`subtree_ids`] and [`hierarchy_tree`]. Cycle-safe via the
/// visited-set; root excluded from `nodes`.
// trace:TASK-1074 | ai:claude
fn downward_closure(
    root: Uuid,
    children: &HashMap<Uuid, Vec<Uuid>>,
    max_depth: Option<usize>,
) -> GraphResult {
    let mut result = GraphResult {
        root,
        ..Default::default()
    };
    let mut visited: HashSet<Uuid> = HashSet::new();
    visited.insert(root);
    let mut queue: VecDeque<(Uuid, usize)> = VecDeque::new();
    queue.push_back((root, 0));
    while let Some((node, depth)) = queue.pop_front() {
        if max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        if let Some(kids) = children.get(&node) {
            for &child in kids {
                result.edges.push(GraphEdge {
                    from: node,
                    rel_type: RelationshipType::Child,
                    to: child,
                });
                if visited.insert(child) {
                    result.nodes.push(child);
                    queue.push_back((child, depth + 1));
                }
            }
        }
    }
    result
}

/// The full hierarchy TREE that contains `start`, for `aida graph --tree`.
/// Climbs to the structural root(s) — the topmost ancestors via the oriented
/// parent edges — then takes the downward [`subtree_ids`] closure from each. For
/// a query ON an epic (which has no parent) this collapses to the plain downward
/// subtree, so `aida graph <epic> --tree` and `aida focus <epic>` report the
/// SAME membership (TASK-1074); for a query on a descendant it still surfaces the
/// whole epic tree — the queried node's ancestors AND its siblings (BUG-534).
/// Roots and every descendant are included; `start` is excluded from `nodes`
/// (re-added by [`tree_layout`]). Cycle-safe.
// trace:TASK-1074 | ai:claude
pub fn hierarchy_tree(
    store: &RequirementsStore,
    start: Uuid,
    max_depth: Option<usize>,
) -> GraphResult {
    // Orient once; build both directions so we can climb up and walk down.
    let mut children: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    let mut parents: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (parent, child) in oriented_hierarchy_edges(store) {
        children.entry(parent).or_default().push(child);
        parents.entry(child).or_default().push(parent);
    }

    // Climb to structural roots (nodes with no parent), cycle-safe.
    let mut roots: Vec<Uuid> = Vec::new();
    let mut climbed: HashSet<Uuid> = HashSet::new();
    let mut up: VecDeque<Uuid> = VecDeque::new();
    up.push_back(start);
    climbed.insert(start);
    while let Some(n) = up.pop_front() {
        match parents.get(&n) {
            Some(ps) if !ps.is_empty() => {
                for &p in ps {
                    if climbed.insert(p) {
                        up.push_back(p);
                    }
                }
            }
            _ => {
                if !roots.contains(&n) {
                    roots.push(n);
                }
            }
        }
    }

    // Downward closure from each root, unioned; the roots themselves (other than
    // `start`, which tree_layout re-adds) are members too.
    let mut result = GraphResult {
        root: start,
        ..Default::default()
    };
    let mut in_nodes: HashSet<Uuid> = HashSet::new();
    in_nodes.insert(start);
    for &r in &roots {
        if in_nodes.insert(r) {
            result.nodes.push(r);
        }
        let sub = downward_closure(r, &children, max_depth);
        result.edges.extend(sub.edges);
        for n in sub.nodes {
            if in_nodes.insert(n) {
                result.nodes.push(n);
            }
        }
    }
    result
}

/// Resolve one hop of neighbors from `node` for the given direction.
fn neighbors(
    store: &RequirementsStore,
    node: Uuid,
    rel_types: &[RelationshipType],
    direction: Direction,
) -> Vec<(RelationshipType, Uuid)> {
    match direction {
        Direction::Outgoing => rel_types
            .iter()
            .flat_map(|rt| {
                store
                    .get_relationships_by_type(&node, rt)
                    .into_iter()
                    .map(move |target| (rt.clone(), target))
            })
            .collect(),
        // Reverse adjacency: scan every requirement for an edge of one of the
        // requested types that points at `node`.
        Direction::Incoming => store
            .requirements
            .iter()
            .flat_map(|req| {
                req.relationships.iter().filter_map(move |rel| {
                    if rel.target_id == node && rel_types.contains(&rel.rel_type) {
                        Some((rel.rel_type.clone(), req.id))
                    } else {
                        None
                    }
                })
            })
            .collect(),
    }
}

/// Status distribution across a set of requirements — the epic-rollup view.
/// Unresolvable ids are skipped (a dangling edge contributes no count) rather
/// than panicking, matching how `pickability` and `show` treat them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusRollup {
    pub total: usize,
    pub completed: usize,
    pub done: usize,
    pub in_progress: usize,
    /// Draft / Approved / Planned — queued, not yet started.
    pub remaining: usize,
    /// NeedsAttention — shelved, needs a decision.
    pub shelved: usize,
    pub rejected: usize,
}

/// Tally the statuses of `ids` against the live store. trace:STORY-489
pub fn status_rollup(store: &RequirementsStore, ids: &[Uuid]) -> StatusRollup {
    let mut r = StatusRollup::default();
    for id in ids {
        let Some(req) = store.get_requirement_by_id(id) else {
            continue;
        };
        r.total += 1;
        match req.status {
            RequirementStatus::Completed => r.completed += 1,
            RequirementStatus::Done => r.done += 1,
            RequirementStatus::InProgress => r.in_progress += 1,
            RequirementStatus::Draft | RequirementStatus::Approved | RequirementStatus::Planned => {
                r.remaining += 1
            }
            RequirementStatus::NeedsAttention => r.shelved += 1,
            RequirementStatus::Rejected => r.rejected += 1,
        }
    }
    r
}

/// BUG-543: the status rollup over a spec's child subtree — the SAME membership
/// `aida graph --tree` prints, exposed as a reusable primitive. Routes through
/// the one shared [`subtree_ids`] downward closure (TASK-1074), so this detector
/// AGREES with the rollup numbers an operator sees in `aida graph --tree <epic>`
/// (the contract BUG-543 references, "Rollup: 4 total · 4 completed") AND with
/// the `aida focus` subtree count — the three used to drift when the tree walk
/// was a direction-agnostic union that leaked a descendant's same-rank second
/// parent into the count.
///
/// Drives the "epic ready to close" surface: an epic whose rollup has
/// `total > 0 && completed == total` is fully delivered and can be closed. The
/// root itself is excluded from the count (the closure excludes the root).
// trace:BUG-543 trace:TASK-1074 | ai:claude
pub fn child_status_rollup(store: &RequirementsStore, root: Uuid) -> StatusRollup {
    let result = subtree_ids(store, root, None);
    status_rollup(store, &result.nodes)
}

/// Lay the walked graph out as a depth-annotated hierarchy for the `--tree`
/// render, so parents, children, and siblings are visually distinct instead of
/// a flat list (BUG-534).
///
/// Parent→child direction is normalized from the relationship type: a `Parent`
/// edge is `from`(parent)→`to`(child); a `Child` edge is its reciprocal, so
/// `to`(parent)→`from`(child). The queried `root` is included even though
/// [`walk`] excludes it from `nodes`, so the caller can mark "you are here" —
/// and because the walk records the epic→root edge (it pushes edges before the
/// visited check), a queried non-root node still nests under its real parent.
///
/// Returns `(node, depth)` in pre-order DFS rooted at structural roots (any
/// node that is not another walked node's child, in first-seen order so the
/// epic leads). Cycle-safe via a visited set; any node not reachable from a
/// structural root (a cycle remnant) is appended so nothing is dropped.
/// trace:BUG-534 | ai:claude
pub fn tree_layout(root: Uuid, nodes: &[Uuid], edges: &[GraphEdge]) -> Vec<(Uuid, usize)> {
    // First-seen node universe: the queried root, then BFS-discovered nodes.
    let mut universe: Vec<Uuid> = Vec::new();
    let mut in_universe: HashSet<Uuid> = HashSet::new();
    for &n in std::iter::once(&root).chain(nodes.iter()) {
        if in_universe.insert(n) {
            universe.push(n);
        }
    }

    // Relationship direction names the TARGET's role (the convention export.rs
    // reads): `X --Parent--> Y` ⇒ Y is X's parent; `X --Child--> Y` ⇒ Y is X's
    // child. Both reduce to a single parent→child fact. The store carries both
    // orientations (a parent with Child→child edges AND children with
    // Parent→parent edges), and walk_union returns both — they agree here.
    let mut children: std::collections::HashMap<Uuid, Vec<Uuid>> = std::collections::HashMap::new();
    let mut has_parent: HashSet<Uuid> = HashSet::new();
    for e in edges {
        let (parent, child) = match e.rel_type {
            RelationshipType::Parent => (e.to, e.from),
            RelationshipType::Child => (e.from, e.to),
            _ => continue,
        };
        if !in_universe.contains(&parent) || !in_universe.contains(&child) || parent == child {
            continue;
        }
        let kids = children.entry(parent).or_default();
        if !kids.contains(&child) {
            kids.push(child);
        }
        has_parent.insert(child);
    }

    fn dfs(
        n: Uuid,
        depth: usize,
        children: &std::collections::HashMap<Uuid, Vec<Uuid>>,
        visited: &mut HashSet<Uuid>,
        out: &mut Vec<(Uuid, usize)>,
    ) {
        if !visited.insert(n) {
            return;
        }
        out.push((n, depth));
        if let Some(kids) = children.get(&n) {
            for &k in kids {
                dfs(k, depth + 1, children, visited, out);
            }
        }
    }

    let mut out: Vec<(Uuid, usize)> = Vec::new();
    let mut visited: HashSet<Uuid> = HashSet::new();
    // Structural roots first (epic leads), then any cycle remnant so the union
    // of emitted nodes equals the universe.
    for &n in &universe {
        if !has_parent.contains(&n) {
            dfs(n, 0, &children, &mut visited, &mut out);
        }
    }
    for &n in &universe {
        dfs(n, 0, &children, &mut visited, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Relationship, Requirement};

    fn make_req(spec_id: &str, status: RequirementStatus) -> Requirement {
        use crate::models::RequirementType;
        let mut r = Requirement::new(format!("title for {spec_id}"), String::new());
        r.spec_id = Some(spec_id.to_string());
        r.status = status;
        // TASK-1074: the shared subtree rule orients edges by type rank, so the
        // hierarchy fixtures need real types. Infer from the spec_id prefix.
        r.req_type = match spec_id.split('-').next() {
            Some("EPIC") => RequirementType::Epic,
            Some("STORY") => RequirementType::Story,
            Some("SPIKE") => RequirementType::Spike,
            Some("BUG") => RequirementType::Bug,
            _ => RequirementType::Task,
        };
        r
    }

    fn link(req: &mut Requirement, rel_type: RelationshipType, target_id: Uuid) {
        req.relationships.push(Relationship {
            rel_type,
            target_id,
            created_at: None,
            created_by: None,
        });
    }

    fn store_with(reqs: Vec<Requirement>) -> RequirementsStore {
        RequirementsStore {
            requirements: reqs,
            ..Default::default()
        }
    }

    #[test]
    fn blocked_by_transitive_closure() {
        // C blocked_by B blocked_by A  ⇒  C's outgoing BlockedBy walk = {B, A}
        let a = make_req("STORY-A", RequirementStatus::InProgress);
        let mut b = make_req("STORY-B", RequirementStatus::Approved);
        link(&mut b, RelationshipType::BlockedBy, a.id);
        let mut c = make_req("STORY-C", RequirementStatus::Approved);
        link(&mut c, RelationshipType::BlockedBy, b.id);
        let (aid, bid, cid) = (a.id, b.id, c.id);
        let store = store_with(vec![a, b, c]);

        let res = walk(
            &store,
            cid,
            &[RelationshipType::BlockedBy],
            Direction::Outgoing,
            None,
        );
        let nodes: HashSet<Uuid> = res.nodes.into_iter().collect();
        assert_eq!(nodes, HashSet::from([bid, aid]));
    }

    #[test]
    fn cycle_does_not_hang() {
        // A blocked_by B, B blocked_by A — a cycle the store permits (no
        // add-time guard on BlockedBy). The visited-set must terminate it.
        let mut a = make_req("STORY-A", RequirementStatus::Approved);
        let mut b = make_req("STORY-B", RequirementStatus::Approved);
        // Need ids first; build then link by id.
        let (aid, bid) = (a.id, b.id);
        link(&mut a, RelationshipType::BlockedBy, bid);
        link(&mut b, RelationshipType::BlockedBy, aid);
        let store = store_with(vec![a, b]);

        let res = walk(
            &store,
            aid,
            &[RelationshipType::BlockedBy],
            Direction::Outgoing,
            None,
        );
        // Terminates, and reaches B (and back-edge to A is recorded but A is
        // not re-enqueued).
        assert_eq!(res.nodes, vec![bid]);
    }

    #[test]
    fn depth_bound_stops_traversal() {
        // C -> B -> A; depth 1 from C reaches only B.
        let a = make_req("STORY-A", RequirementStatus::Approved);
        let mut b = make_req("STORY-B", RequirementStatus::Approved);
        link(&mut b, RelationshipType::BlockedBy, a.id);
        let mut c = make_req("STORY-C", RequirementStatus::Approved);
        link(&mut c, RelationshipType::BlockedBy, b.id);
        let (bid, cid) = (b.id, c.id);
        let store = store_with(vec![a, b, c]);

        let res = walk(
            &store,
            cid,
            &[RelationshipType::BlockedBy],
            Direction::Outgoing,
            Some(1),
        );
        assert_eq!(res.nodes, vec![bid]);
    }

    #[test]
    fn impact_reverse_closure() {
        // B blocked_by A; the incoming BlockedBy walk from A finds B (what is
        // blocked by A), robust to the edge being stored only on B.
        let a = make_req("STORY-A", RequirementStatus::InProgress);
        let mut b = make_req("STORY-B", RequirementStatus::Approved);
        link(&mut b, RelationshipType::BlockedBy, a.id);
        let (aid, bid) = (a.id, b.id);
        let store = store_with(vec![a, b]);

        let res = walk(
            &store,
            aid,
            &[RelationshipType::BlockedBy],
            Direction::Incoming,
            None,
        );
        assert_eq!(res.nodes, vec![bid]);
    }

    #[test]
    fn tree_unions_child_and_parent_outgoing_edges() {
        // BUG-448 / TASK-679: the epic rollup walks BOTH `Child` and `Parent`
        // outgoing edges so it resolves children regardless of how the
        // hierarchy was stored. Canonically (post-TASK-679) both `aida add
        // --parent` and `aida rel add --type parent` store `epic --Parent-->
        // child`; the `Child` leg is kept for back-compat with any legacy store
        // whose epic carries `Child --> child` edges. The union must surface
        // both orientations, deduped.
        let child_via_legacy = make_req("STORY-1", RequirementStatus::Approved);
        let child_via_canonical = make_req("STORY-2", RequirementStatus::Approved);
        let mut epic = make_req("EPIC-X", RequirementStatus::InProgress);
        link(&mut epic, RelationshipType::Child, child_via_legacy.id);
        link(&mut epic, RelationshipType::Parent, child_via_canonical.id);
        // A duplicate edge (the old non-deduping `rel add` could create one) —
        // the walk must collapse it.
        link(&mut epic, RelationshipType::Parent, child_via_canonical.id);
        let (eid, legacy_id, canonical_id) = (epic.id, child_via_legacy.id, child_via_canonical.id);
        let store = store_with(vec![child_via_legacy, child_via_canonical, epic]);

        let res = walk_union(
            &store,
            eid,
            &[(
                vec![RelationshipType::Child, RelationshipType::Parent],
                Direction::Outgoing,
            )],
            None,
        );
        let nodes: HashSet<Uuid> = res.nodes.iter().copied().collect();
        assert_eq!(nodes, HashSet::from([legacy_id, canonical_id]));
        // deduped: two children, not three (the duplicate Parent edge collapses).
        assert_eq!(res.nodes.len(), 2);
    }

    #[test]
    fn tree_rollup_counts_by_status() {
        // Epic with mixed-status children.
        let done = make_req("STORY-1", RequirementStatus::Completed);
        let wip = make_req("STORY-2", RequirementStatus::InProgress);
        let shelved = make_req("STORY-3", RequirementStatus::NeedsAttention);
        let queued = make_req("STORY-4", RequirementStatus::Approved);
        let ids = vec![done.id, wip.id, shelved.id, queued.id];
        let store = store_with(vec![done, wip, shelved, queued]);

        let r = status_rollup(&store, &ids);
        assert_eq!(r.total, 4);
        assert_eq!(r.completed, 1);
        assert_eq!(r.in_progress, 1);
        assert_eq!(r.shelved, 1);
        assert_eq!(r.remaining, 1);
    }

    // BUG-543: the "epic ready to close" detector's rollup. Mirrors the real
    // store's reciprocal storage (epic carries `Child→story`, story carries
    // `Parent→epic`) and confirms the helper agrees with the tree rollup and
    // excludes the root.
    #[test]
    fn child_status_rollup_counts_an_epics_children() {
        let mut epic = make_req("EPIC-X", RequirementStatus::Draft);
        let mut s1 = make_req("STORY-1", RequirementStatus::Completed);
        let mut s2 = make_req("STORY-2", RequirementStatus::Completed);
        let mut s3 = make_req("STORY-3", RequirementStatus::InProgress);
        for s in [&mut s1, &mut s2, &mut s3] {
            link(&mut epic, RelationshipType::Child, s.id);
            link(s, RelationshipType::Parent, epic.id);
        }
        let eid = epic.id;
        let store = store_with(vec![epic, s1, s2, s3]);

        let r = child_status_rollup(&store, eid);
        // Three children counted; the epic (root) is NOT in the rollup.
        assert_eq!(r.total, 3);
        assert_eq!(r.completed, 2);
        assert_eq!(r.in_progress, 1);
    }

    // BUG-543: a fully-delivered epic — every child Completed — is the
    // `total>0 && completed==total` case the surface keys off.
    #[test]
    fn child_status_rollup_all_completed_is_ready_to_close() {
        let mut epic = make_req("EPIC-X", RequirementStatus::Draft);
        let s1 = make_req("STORY-1", RequirementStatus::Completed);
        let s2 = make_req("STORY-2", RequirementStatus::Completed);
        // Canonical post-TASK-679 leg too (epic --Parent--> child): the helper
        // must resolve children whichever orientation was written.
        link(&mut epic, RelationshipType::Child, s1.id);
        link(&mut epic, RelationshipType::Parent, s2.id);
        let eid = epic.id;
        let store = store_with(vec![epic, s1, s2]);

        let r = child_status_rollup(&store, eid);
        assert_eq!(r.total, 2);
        assert_eq!(r.completed, 2);
        assert!(r.total > 0 && r.completed == r.total);
    }

    // BUG-628: archived is a VIEW flag, not a status — it must not change the
    // rollup math. A fully-shipped epic whose Completed children were archived
    // must still roll up to all-Completed (so `derive_epic_status` reports
    // Completed, not Draft). Locks in that `status_rollup` counts archived
    // children rather than excluding them. trace:BUG-628 | ai:claude
    #[test]
    fn child_status_rollup_counts_archived_completed_children() {
        let mut epic = make_req("EPIC-X", RequirementStatus::Draft);
        let mut s1 = make_req("STORY-1", RequirementStatus::Completed);
        s1.archived = true;
        let mut s2 = make_req("STORY-2", RequirementStatus::Completed);
        s2.archived = true;
        // Mixed orientation, like the real store.
        link(&mut epic, RelationshipType::Child, s1.id);
        link(&mut s1, RelationshipType::Parent, epic.id);
        link(&mut epic, RelationshipType::Parent, s2.id);
        link(&mut s2, RelationshipType::Parent, epic.id);
        let eid = epic.id;
        let store = store_with(vec![epic, s1, s2]);

        let r = child_status_rollup(&store, eid);
        assert_eq!(r.total, 2, "archived children are still counted");
        assert_eq!(r.completed, 2, "archived-Completed counts toward completed");
        assert_eq!(
            crate::rollup::derive_epic_status_from_rollup(&r),
            Some(RequirementStatus::Completed),
            "an epic whose only children are Completed-but-archived derives Completed"
        );
    }

    // BUG-628: archived-Done children count toward the done rollup too — a fully
    // finished-on-branch epic whose children were archived derives Done.
    // trace:BUG-628 | ai:claude
    #[test]
    fn child_status_rollup_counts_archived_done_children() {
        let mut epic = make_req("EPIC-X", RequirementStatus::Draft);
        let mut s1 = make_req("STORY-1", RequirementStatus::Done);
        s1.archived = true;
        let mut s2 = make_req("STORY-2", RequirementStatus::Completed);
        s2.archived = true;
        for s in [&mut s1, &mut s2] {
            link(&mut epic, RelationshipType::Child, s.id);
            link(s, RelationshipType::Parent, epic.id);
        }
        let eid = epic.id;
        let store = store_with(vec![epic, s1, s2]);

        let r = child_status_rollup(&store, eid);
        assert_eq!(r.total, 2);
        assert_eq!(r.done, 1);
        assert_eq!(r.completed, 1);
        assert_eq!(
            crate::rollup::derive_epic_status_from_rollup(&r),
            Some(RequirementStatus::Done),
            "archived done+completed children derive Done, not Draft"
        );
    }

    // BUG-543: an epic with no children has an empty rollup — NOT ready to close
    // (nothing delivered), so the `total > 0` guard excludes it.
    #[test]
    fn child_status_rollup_empty_for_childless_epic() {
        let epic = make_req("EPIC-X", RequirementStatus::Draft);
        let eid = epic.id;
        let store = store_with(vec![epic]);

        let r = child_status_rollup(&store, eid);
        assert_eq!(r.total, 0);
    }

    // trace:BUG-534 | ai:claude
    #[test]
    fn tree_layout_roots_at_epic_and_nests_the_queried_story() {
        // The BUG-534 repro: query a STORY (not the epic). The walk goes UP to
        // the epic (reciprocal Child edge) then back DOWN to the siblings; the
        // layout must root at the epic with every story nested under it — the
        // queried story included — not a flat list.
        let mut epic = make_req("EPIC-X", RequirementStatus::InProgress);
        let mut s1 = make_req("STORY-1", RequirementStatus::Approved);
        let mut s2 = make_req("STORY-2", RequirementStatus::Approved);
        // Storage convention (export.rs): the target names the role — a parent
        // carries `Child→child`, a child carries `Parent→parent`. Mirror both,
        // as the real store does.
        link(&mut epic, RelationshipType::Child, s1.id);
        link(&mut epic, RelationshipType::Child, s2.id);
        link(&mut s1, RelationshipType::Parent, epic.id);
        link(&mut s2, RelationshipType::Parent, epic.id);
        let (eid, s1id, s2id) = (epic.id, s1.id, s2.id);
        let store = store_with(vec![epic, s1, s2]);

        let res = walk_union(
            &store,
            s1id, // query the STORY
            &[(
                vec![RelationshipType::Child, RelationshipType::Parent],
                Direction::Outgoing,
            )],
            None,
        );
        let layout = tree_layout(s1id, &res.nodes, &res.edges);
        // Epic is the structural root at depth 0; both stories nest at depth 1.
        assert_eq!(layout[0], (eid, 0), "epic roots the tree");
        let depths: std::collections::HashMap<Uuid, usize> = layout.iter().copied().collect();
        assert_eq!(
            depths.get(&s1id),
            Some(&1),
            "queried story nests under epic"
        );
        assert_eq!(depths.get(&s2id), Some(&1), "sibling nests under epic");
        // Every walked node plus the query is emitted exactly once.
        assert_eq!(layout.len(), 3);
    }

    // trace:BUG-534 | ai:claude
    #[test]
    fn tree_layout_nests_grandchildren_by_depth() {
        // EPIC → STORY → TASK must render at depths 0, 1, 2.
        let mut epic = make_req("EPIC-X", RequirementStatus::InProgress);
        let mut story = make_req("STORY-1", RequirementStatus::Approved);
        let mut task = make_req("TASK-1", RequirementStatus::Approved);
        link(&mut epic, RelationshipType::Child, story.id);
        link(&mut story, RelationshipType::Child, task.id);
        link(&mut story, RelationshipType::Parent, epic.id);
        link(&mut task, RelationshipType::Parent, story.id);
        let (eid, sid, tid) = (epic.id, story.id, task.id);
        let store = store_with(vec![epic, story, task]);

        let res = walk_union(
            &store,
            eid,
            &[(
                vec![RelationshipType::Child, RelationshipType::Parent],
                Direction::Outgoing,
            )],
            None,
        );
        let layout = tree_layout(eid, &res.nodes, &res.edges);
        assert_eq!(layout, vec![(eid, 0), (sid, 1), (tid, 2)]);
    }

    // trace:BUG-534 | ai:claude
    #[test]
    fn tree_layout_drops_nothing_on_a_cycle() {
        // A pathological parent cycle (A→B→A) must still terminate and emit
        // both nodes rather than hang or silently drop one.
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let edges = vec![
            GraphEdge {
                from: a,
                rel_type: RelationshipType::Parent,
                to: b,
            },
            GraphEdge {
                from: b,
                rel_type: RelationshipType::Parent,
                to: a,
            },
        ];
        let layout = tree_layout(a, &[b], &edges);
        let seen: HashSet<Uuid> = layout.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            seen,
            HashSet::from([a, b]),
            "both nodes emitted, none dropped"
        );
    }

    #[test]
    fn dangling_target_is_surfaced_not_panicked() {
        // A blocked_by a target that no longer exists: the edge is recorded,
        // no node is added, and rollup skips the missing id.
        let mut a = make_req("STORY-A", RequirementStatus::Approved);
        let missing = Uuid::new_v4();
        link(&mut a, RelationshipType::BlockedBy, missing);
        let aid = a.id;
        let store = store_with(vec![a]);

        let res = walk(
            &store,
            aid,
            &[RelationshipType::BlockedBy],
            Direction::Outgoing,
            None,
        );
        assert_eq!(res.edges.len(), 1);
        assert_eq!(res.edges[0].to, missing);
        // The dangling node is still enqueued as an id (we can't know it's
        // missing without resolving), but rollup contributes nothing for it.
        let rollup = status_rollup(&store, &res.nodes);
        assert_eq!(rollup.total, 0);
    }

    #[test]
    fn walk_union_impact_catches_unidirectional_blocks() {
        // trace:BUG-411 | ai:claude
        // A blocks B, stored ONLY as A.Blocks->B (unidirectional, no
        // B.BlockedBy->A). Impact-from-A = (incoming BlockedBy) ∪ (outgoing
        // Blocks) must still find B via the outgoing-Blocks leg.
        let mut a = make_req("STORY-A", RequirementStatus::InProgress);
        let b = make_req("STORY-B", RequirementStatus::Approved);
        link(&mut a, RelationshipType::Blocks, b.id);
        let (aid, bid) = (a.id, b.id);
        let store = store_with(vec![a, b]);

        let specs = [
            (vec![RelationshipType::BlockedBy], Direction::Incoming),
            (vec![RelationshipType::Blocks], Direction::Outgoing),
        ];
        let res = walk_union(&store, aid, &specs, None);
        assert_eq!(
            res.nodes,
            vec![bid],
            "impact must find the unidirectionally-blocked spec"
        );

        // And the other storage form (B.BlockedBy->A) is still caught via the
        // incoming-BlockedBy leg — and not double-counted when both exist.
        let mut a2 = make_req("STORY-A2", RequirementStatus::InProgress);
        let mut b2 = make_req("STORY-B2", RequirementStatus::Approved);
        link(&mut b2, RelationshipType::BlockedBy, a2.id);
        link(&mut a2, RelationshipType::Blocks, b2.id); // bidirectional-ish
        let (a2id, b2id) = (a2.id, b2.id);
        let store2 = store_with(vec![a2, b2]);
        let res2 = walk_union(&store2, a2id, &specs, None);
        assert_eq!(
            res2.nodes,
            vec![b2id],
            "both edge forms ⇒ B2 once, not twice"
        );
    }

    // TASK-1074: the EPIC-54 discrepancy. A story is a child of the epic AND has
    // a SAME-RANK second parent (another story) that lives OUTSIDE the epic. The
    // old direction-agnostic tree walk climbed the reciprocal edge up to that
    // second parent and counted it as a subtree member (44 vs 43); the shared
    // rank-oriented downward closure excludes it. trace:TASK-1074 | ai:claude
    #[test]
    fn subtree_ids_excludes_same_rank_second_parent() {
        let mut epic = make_req("EPIC-54", RequirementStatus::InProgress);
        let mut child = make_req("STORY-699", RequirementStatus::Completed);
        let mut second_parent = make_req("STORY-698", RequirementStatus::Approved);
        // epic --Parent--> child ; child --Child--> epic (convention B, both ends)
        link(&mut epic, RelationshipType::Parent, child.id);
        link(&mut child, RelationshipType::Child, epic.id);
        // second_parent --Parent--> child ; child --Child--> second_parent
        // (same STORY↔STORY shape as 698↔699 in the real store).
        link(&mut second_parent, RelationshipType::Parent, child.id);
        link(&mut child, RelationshipType::Child, second_parent.id);
        let (eid, cid, spid) = (epic.id, child.id, second_parent.id);
        let store = store_with(vec![epic, child, second_parent]);

        let nodes: HashSet<Uuid> = subtree_ids(&store, eid, None).nodes.into_iter().collect();
        assert!(nodes.contains(&cid), "the real child is in the subtree");
        assert!(
            !nodes.contains(&spid),
            "a descendant's same-rank second parent is NOT a subtree member"
        );
        assert_eq!(
            nodes.len(),
            1,
            "exactly the one child, not the leaked parent"
        );
    }

    // TASK-1074: the shared rule is robust to BOTH storage orientations. The
    // "convention A" store records `epic --Child--> story` (+ `story --Parent-->
    // epic`); type rank (Epic < Story) still orients the epic as the parent, so
    // the downward closure reaches the children that the old strict cache CTE
    // missed entirely. trace:TASK-1074 | ai:claude
    #[test]
    fn subtree_ids_handles_inverted_convention_via_type_rank() {
        let mut epic = make_req("EPIC-39", RequirementStatus::InProgress);
        let mut s1 = make_req("STORY-572", RequirementStatus::Approved);
        let mut s2 = make_req("STORY-573", RequirementStatus::Approved);
        // Inverted (convention-A) orientation on both endpoints.
        link(&mut epic, RelationshipType::Child, s1.id);
        link(&mut s1, RelationshipType::Parent, epic.id);
        link(&mut epic, RelationshipType::Child, s2.id);
        link(&mut s2, RelationshipType::Parent, epic.id);
        let (eid, s1id, s2id) = (epic.id, s1.id, s2.id);
        let store = store_with(vec![epic, s1, s2]);

        let nodes: HashSet<Uuid> = subtree_ids(&store, eid, None).nodes.into_iter().collect();
        assert_eq!(nodes, HashSet::from([s1id, s2id]), "both children reached");
    }

    // TASK-1074: grandchildren nest transitively (EPIC → STORY → TASK).
    #[test]
    fn subtree_ids_is_transitive() {
        let mut epic = make_req("EPIC-1", RequirementStatus::InProgress);
        let mut story = make_req("STORY-1", RequirementStatus::Approved);
        let mut task = make_req("TASK-1", RequirementStatus::Approved);
        link(&mut epic, RelationshipType::Parent, story.id);
        link(&mut story, RelationshipType::Parent, task.id);
        let (eid, sid, tid) = (epic.id, story.id, task.id);
        let store = store_with(vec![epic, story, task]);

        let nodes: HashSet<Uuid> = subtree_ids(&store, eid, None).nodes.into_iter().collect();
        assert_eq!(nodes, HashSet::from([sid, tid]));
    }

    // TASK-1074: `hierarchy_tree` (the `aida graph --tree` membership) rooted at
    // an EPIC equals the plain `subtree_ids` closure — this is what makes
    // `aida graph <epic> --tree` and `aida focus <epic>` agree — while a query on
    // a child still climbs to the epic and back down (BUG-534 preserved).
    #[test]
    fn hierarchy_tree_epic_query_equals_subtree_ids() {
        let mut epic = make_req("EPIC-1", RequirementStatus::InProgress);
        let mut s1 = make_req("STORY-1", RequirementStatus::Approved);
        let mut s2 = make_req("STORY-2", RequirementStatus::Approved);
        for s in [&mut s1, &mut s2] {
            link(&mut epic, RelationshipType::Parent, s.id);
            link(s, RelationshipType::Child, epic.id);
        }
        let (eid, s1id, s2id) = (epic.id, s1.id, s2.id);
        let store = store_with(vec![epic, s1, s2]);

        let tree: HashSet<Uuid> = hierarchy_tree(&store, eid, None)
            .nodes
            .into_iter()
            .collect();
        let subtree: HashSet<Uuid> = subtree_ids(&store, eid, None).nodes.into_iter().collect();
        assert_eq!(tree, subtree, "epic-rooted tree == focus subtree");
        assert_eq!(tree, HashSet::from([s1id, s2id]));

        // Querying a child climbs to the epic and includes its sibling.
        let from_child: HashSet<Uuid> = hierarchy_tree(&store, s1id, None)
            .nodes
            .into_iter()
            .collect();
        assert!(from_child.contains(&eid), "climbs up to the epic");
        assert!(from_child.contains(&s2id), "and back down to the sibling");
    }
}
