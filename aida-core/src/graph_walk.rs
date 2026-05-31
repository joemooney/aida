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
use std::collections::{HashSet, VecDeque};
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Relationship, Requirement};

    fn make_req(spec_id: &str, status: RequirementStatus) -> Requirement {
        let mut r = Requirement::new(format!("title for {spec_id}"), String::new());
        r.spec_id = Some(spec_id.to_string());
        r.status = status;
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
}
