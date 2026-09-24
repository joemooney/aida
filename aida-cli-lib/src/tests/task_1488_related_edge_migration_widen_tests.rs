// Fixture tests for the TASK-1488 widening of `aida db migrate-related-edges`:
// any Custom edge whose stored spelling parses to a standard type via
// `RelationshipType::parse_relationship_type` is converted, not just the
// `related` family (TASK-1426's original scope).
//
// NOTE on `duplicate-of`: TASK-1488's filed background cites the 2026-09-21
// live census finding "Custom depends-on (1) and duplicate-of (1)" and lists
// `duplicate-of` alongside `depends-on` / `verified_by` / `replaced_by` as a
// spelling the shared parser resolves to a standard type. Checked directly
// (`RelationshipType::parse_relationship_type("duplicate-of")`), it does
// NOT: `parse_relationship_type` and the `from_str` it falls back to only
// recognize the bare word `duplicate`, not `duplicate-of`. So under this
// migration's actual selection predicate — "parses to a standard type" —
// `duplicate-of` stays `Custom` and is left alone, exactly like `implements`.
// That's exercised below as the "unparseable" case (real spelling from the
// live store), and the "twin is deleted" case instead uses the bare
// `duplicate` spelling, which does parse. This is a real gap between the
// shared parser's alias table and the task's filed description, not a bug in
// this migration; it's called out in the TASK-1488 completion comment.
// trace:TASK-1488 | ai:claude

use crate::related_edge_migration::{
    check_migrated, plan_migration, run_migration, standard_type_for, EdgeAction,
};
use aida_core::db::DatabaseBackend;
use aida_core::models::{Relationship, RelationshipType, Requirement};
use aida_core::CachedGitBackend;
use tempfile::tempdir;
use uuid::Uuid;

fn custom(name: &str) -> RelationshipType {
    RelationshipType::Custom(name.to_string())
}

fn edge(rel_type: RelationshipType, target: Uuid) -> Relationship {
    Relationship {
        rel_type,
        target_id: target,
        created_at: None,
        created_by: Some("fixture".into()),
    }
}

fn spec(spec_id: &str, rels: Vec<Relationship>) -> Requirement {
    let mut r = Requirement::new(spec_id.into(), "desc".into());
    r.spec_id = Some(spec_id.into());
    r.relationships = rels;
    r
}

fn open_git_backend() -> (tempfile::TempDir, std::path::PathBuf, CachedGitBackend) {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join("store");
    let cache_path = dir.path().join(".aida").join("cache.db");
    std::fs::create_dir_all(&store_root).unwrap();
    aida_core::git_ops::init(&store_root).unwrap();
    aida_core::git_ops::configure_user(&store_root, "fixture", "fixture@localhost").unwrap();
    let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();
    (dir, store_root, backend)
}

fn add(backend: &CachedGitBackend, spec_id: &str) -> Requirement {
    backend.add_requirement(spec(spec_id, vec![])).unwrap()
}

fn set_rels(backend: &CachedGitBackend, spec_id: &str, rels: Vec<Relationship>) {
    let mut r = backend
        .get_requirement_by_spec_id(spec_id)
        .unwrap()
        .unwrap();
    r.relationships = rels;
    backend.update_requirement(&r).unwrap();
}

fn rels_of(backend: &CachedGitBackend, spec_id: &str) -> Vec<Relationship> {
    backend
        .get_requirement_by_spec_id(spec_id)
        .unwrap()
        .unwrap()
        .relationships
}

#[test]
fn standard_type_for_matches_the_shared_parser_exactly() {
    assert_eq!(
        standard_type_for(&custom("depends-on")),
        Some(RelationshipType::BlockedBy)
    );
    assert_eq!(
        standard_type_for(&custom("verified_by")),
        Some(RelationshipType::VerifiedBy)
    );
    assert_eq!(
        standard_type_for(&custom("replaced_by")),
        Some(RelationshipType::SupersededBy)
    );
    assert_eq!(
        standard_type_for(&custom("related")),
        Some(RelationshipType::References)
    );
    assert_eq!(
        standard_type_for(&custom("duplicate")),
        Some(RelationshipType::Duplicate)
    );
    // The live-store spelling that does NOT parse — see module doc comment.
    assert_eq!(standard_type_for(&custom("duplicate-of")), None);
    assert_eq!(standard_type_for(&custom("implements")), None);
    assert_eq!(standard_type_for(&custom("implemented-by")), None);
    assert_eq!(standard_type_for(&custom("sprint_23")), None);
    // Already-typed edges are never migration candidates.
    assert_eq!(standard_type_for(&RelationshipType::BlockedBy), None);
}

#[test]
fn custom_depends_on_is_converted_to_blocked_by() {
    let t = Uuid::now_v7();
    let reqs = vec![spec("A-1", vec![edge(custom("depends-on"), t)])];
    let plan = plan_migration(&reqs);
    assert_eq!(plan.converted, 1);
    let s = &plan.specs[0];
    assert_eq!(s.edges[0].action, EdgeAction::Convert);
    assert_eq!(s.edges[0].to_type, "blocked_by");
    assert_eq!(
        s.new_relationships,
        vec![edge(RelationshipType::BlockedBy, t)]
    );
    check_migrated("A-1", &s.new_relationships).unwrap();
}

#[test]
fn verified_by_and_replaced_by_also_convert() {
    let a = Uuid::now_v7();
    let b = Uuid::now_v7();
    let reqs = vec![spec(
        "A-1",
        vec![
            edge(custom("verified_by"), a),
            edge(custom("replaced_by"), b),
        ],
    )];
    let plan = plan_migration(&reqs);
    assert_eq!(plan.converted, 2);
    assert_eq!(
        plan.specs[0].new_relationships,
        vec![
            edge(RelationshipType::VerifiedBy, a),
            edge(RelationshipType::SupersededBy, b),
        ]
    );
}

#[test]
fn duplicate_twin_is_deleted() {
    // `duplicate` (not `duplicate-of` — see module doc comment) is the
    // spelling that actually parses to the standard `Duplicate` type.
    let t = Uuid::now_v7();
    let reqs = vec![spec(
        "A-1",
        vec![
            edge(custom("duplicate"), t),
            edge(RelationshipType::Duplicate, t),
        ],
    )];
    let plan = plan_migration(&reqs);
    assert_eq!(plan.converted, 0);
    assert_eq!(plan.twins_deleted, 1);
    assert_eq!(
        plan.specs[0].new_relationships,
        vec![edge(RelationshipType::Duplicate, t)]
    );
}

#[test]
fn unparseable_custom_name_is_left_alone() {
    let t = Uuid::now_v7();
    // The real live-store spelling (see module doc comment) plus a made-up
    // one, to confirm this isn't special-cased on the literal string.
    let reqs = vec![spec(
        "A-1",
        vec![
            edge(custom("duplicate-of"), t),
            edge(custom("totally-custom-xyz"), t),
        ],
    )];
    let plan = plan_migration(&reqs);
    assert!(plan.is_noop(), "no edge here parses to a standard type");
    assert_eq!(plan.legacy_edges, 0);
}

#[test]
fn implements_is_untouched() {
    let t = Uuid::now_v7();
    let reqs = vec![spec("A-1", vec![edge(custom("implements"), t)])];
    let plan = plan_migration(&reqs);
    assert!(plan.is_noop());
}

#[test]
fn parent_child_orphan_converts_and_does_not_touch_the_target() {
    // Design decision (documented in related_edge_migration.rs's module doc
    // comment): this migration never writes a reciprocal edge, even for the
    // Parent/Child pair that `rel add` auto-reciprocates. Pin that choice:
    // converting a Custom("child") edge must NOT create any edge on the
    // target spec.
    let (_dir, _root, backend) = open_git_backend();
    let a = add(&backend, "FR-1");
    let b = add(&backend, "FR-2");
    let _ = a;
    set_rels(&backend, "FR-1", vec![edge(custom("child"), b.id)]);

    let outcome = run_migration(&backend, false).unwrap();
    assert_eq!(outcome.specs_written, 1);
    assert_eq!(
        rels_of(&backend, "FR-1"),
        vec![edge(RelationshipType::Child, b.id)]
    );
    assert!(
        rels_of(&backend, "FR-2").is_empty(),
        "no reciprocal Parent edge written on the target"
    );
}

#[test]
fn mixed_standard_types_on_one_target_are_independent() {
    // Two different custom spellings resolving to two different standard
    // types, both pointed at the same target, convert independently of one
    // another (grouping is per (standard-type, target), not per target).
    let t = Uuid::now_v7();
    let reqs = vec![spec(
        "A-1",
        vec![
            edge(custom("depends-on"), t),
            edge(custom("verified_by"), t),
        ],
    )];
    let plan = plan_migration(&reqs);
    assert_eq!(plan.converted, 2);
    assert_eq!(plan.twins_deleted, 0);
    let new = &plan.specs[0].new_relationships;
    assert!(new.contains(&edge(RelationshipType::BlockedBy, t)));
    assert!(new.contains(&edge(RelationshipType::VerifiedBy, t)));
}

#[test]
fn apply_is_idempotent_across_the_widened_predicate() {
    let (_dir, _root, backend) = open_git_backend();
    add(&backend, "FR-1");
    let b = add(&backend, "FR-2");
    add(&backend, "FR-3");
    set_rels(
        &backend,
        "FR-1",
        vec![
            edge(custom("depends-on"), b.id),
            edge(custom("implements"), b.id),
        ],
    );
    set_rels(&backend, "FR-3", vec![edge(custom("duplicate-of"), b.id)]);

    let outcome = run_migration(&backend, false).unwrap();
    assert_eq!(outcome.specs_written, 1, "only FR-1 has a migratable edge");
    assert_eq!(
        rels_of(&backend, "FR-1"),
        vec![
            edge(RelationshipType::BlockedBy, b.id),
            edge(custom("implements"), b.id),
        ]
    );
    assert_eq!(
        rels_of(&backend, "FR-3"),
        vec![edge(custom("duplicate-of"), b.id)],
        "duplicate-of does not parse to a standard type, so it's untouched"
    );

    let again = run_migration(&backend, false).unwrap();
    assert!(again.plan.is_noop());
    assert_eq!(again.specs_written, 0);
}
