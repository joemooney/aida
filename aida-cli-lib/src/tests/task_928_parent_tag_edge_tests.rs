use super::ensure_parent_edge_from_tag;
use aida_core::db::DatabaseBackend;
use aida_core::graph_walk::{walk_union, Direction};
use aida_core::models::{Relationship, RelationshipType, Requirement};
use aida_core::CachedGitBackend;
use tempfile::tempdir;

fn open_backend() -> (tempfile::TempDir, CachedGitBackend) {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join("store");
    let cache_path = dir.path().join(".aida").join("cache.db");
    std::fs::create_dir_all(&store_root).unwrap();
    let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();
    (dir, backend)
}

fn add_spec(backend: &CachedGitBackend, spec_id: &str, tags: &[&str]) -> Requirement {
    let mut r = Requirement::new(spec_id.into(), "desc".into());
    r.spec_id = Some(spec_id.into());
    for t in tags {
        r.tags.insert((*t).into());
    }
    backend.add_requirement(r).unwrap()
}

#[test]
fn parent_tag_materializes_bidirectional_edge_and_is_graph_visible() {
    let (_dir, backend) = open_backend();
    let epic = add_spec(&backend, "EPIC-1", &[]);
    let child = add_spec(&backend, "TASK-1", &["parent:EPIC-1"]);
    let epic_id = epic.id;
    let child_id = child.id;

    // The tag alone (pre-fix) left no edge — assert the helper writes both.
    let linked = ensure_parent_edge_from_tag(&backend, "TASK-1").unwrap();
    assert_eq!(linked.as_deref(), Some("EPIC-1"));

    // Read BOTH ends back from the canonical store (full YAML load), no
    // manual cache rebuild between the write and the read.
    let child = backend
        .get_requirement_by_spec_id("TASK-1")
        .unwrap()
        .unwrap();
    assert!(
        child
            .relationships
            .iter()
            .any(|r| r.rel_type == RelationshipType::Child && r.target_id == epic_id),
        "child must carry Child --> epic"
    );
    let epic = backend
        .get_requirement_by_spec_id("EPIC-1")
        .unwrap()
        .unwrap();
    assert!(
        epic.relationships
            .iter()
            .any(|r| r.rel_type == RelationshipType::Parent && r.target_id == child_id),
        "epic must carry the reciprocal Parent --> child"
    );

    // Part D: the new spec appears UNDER the epic in a graph query off a
    // fresh load — the exact thing SPIKE-71 found broken until a rebuild.
    let store = backend.load().unwrap();
    let res = walk_union(
        &store,
        epic_id,
        &[(
            vec![RelationshipType::Child, RelationshipType::Parent],
            Direction::Outgoing,
        )],
        None,
    );
    assert!(
        res.nodes.contains(&child_id),
        "graph --tree from the epic must reach the tag-linked child without a manual rebuild"
    );

    // The tag itself is retained (additive, not a move).
    assert!(child.tags.contains("parent:EPIC-1"), "parent: tag is kept");
}

#[test]
fn parent_tag_unresolvable_target_is_lenient_noop() {
    let (_dir, backend) = open_backend();
    add_spec(&backend, "TASK-1", &["parent:EPIC-999"]);
    // No such EPIC-999 → no error, no edge, tag preserved.
    let linked = ensure_parent_edge_from_tag(&backend, "TASK-1").unwrap();
    assert_eq!(linked, None, "unresolvable parent target is a silent no-op");
    let child = backend
        .get_requirement_by_spec_id("TASK-1")
        .unwrap()
        .unwrap();
    assert!(
        child.relationships.is_empty(),
        "no edge written for an unresolvable target"
    );
    assert!(
        child.tags.contains("parent:EPIC-999"),
        "tag is left in place"
    );
}

#[test]
fn parent_tag_is_idempotent() {
    let (_dir, backend) = open_backend();
    add_spec(&backend, "EPIC-1", &[]);
    add_spec(&backend, "TASK-1", &["parent:EPIC-1"]);
    // First call links; second is a no-op (no duplicate edges).
    assert!(ensure_parent_edge_from_tag(&backend, "TASK-1")
        .unwrap()
        .is_some());
    assert!(
        ensure_parent_edge_from_tag(&backend, "TASK-1")
            .unwrap()
            .is_none(),
        "re-running must not relink"
    );
    let epic = backend
        .get_requirement_by_spec_id("EPIC-1")
        .unwrap()
        .unwrap();
    let child = backend
        .get_requirement_by_spec_id("TASK-1")
        .unwrap()
        .unwrap();
    assert_eq!(
        child
            .relationships
            .iter()
            .filter(|r| r.rel_type == RelationshipType::Child)
            .count(),
        1,
        "exactly one Child edge"
    );
    assert_eq!(
        epic.relationships
            .iter()
            .filter(|r| r.rel_type == RelationshipType::Parent)
            .count(),
        1,
        "exactly one reciprocal Parent edge"
    );
}

#[test]
fn parent_child_rel_add_visible_without_manual_cache_rebuild() {
    // Part A + D: a parent/child edge written through the backend (the same
    // update_requirement path `aida rel add` uses) is BOTH-directional and
    // immediately visible to a fresh full-store load — no `cache rebuild`.
    let (_dir, backend) = open_backend();
    let epic = add_spec(&backend, "EPIC-1", &[]);
    let child = add_spec(&backend, "TASK-1", &[]);
    let (epic_id, child_id) = (epic.id, child.id);

    // Write the Parent edge + its reciprocal (what rel add --type parent does
    // by default post-TASK-679, via rel_should_write_inverse).
    let now = chrono::Utc::now();
    let mut epic_mut = epic.clone();
    epic_mut.relationships.push(Relationship {
        rel_type: RelationshipType::Parent,
        target_id: child_id,
        created_at: Some(now),
        created_by: None,
    });
    backend.update_requirement(&epic_mut).unwrap();
    let mut child_mut = child.clone();
    child_mut.relationships.push(Relationship {
        rel_type: RelationshipType::Child,
        target_id: epic_id,
        created_at: Some(now),
        created_by: None,
    });
    backend.update_requirement(&child_mut).unwrap();

    // Fresh load (graph's data source) sees both directions right away.
    let store = backend.load().unwrap();
    let res = walk_union(
        &store,
        epic_id,
        &[(
            vec![RelationshipType::Child, RelationshipType::Parent],
            Direction::Outgoing,
        )],
        None,
    );
    assert!(
        res.nodes.contains(&child_id),
        "rel-add must be graph-visible without a manual cache rebuild"
    );
}
