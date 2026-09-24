// Fixture tests for `aida db migrate-related-edges` and for `rel remove`
// honoring `--type` against stored legacy custom related edges.
// trace:TASK-1426 | ai:claude

use crate::related_edge_migration::{
    check_migrated, duplicate_edges, is_legacy_related, plan_migration, run_migration, EdgeAction,
};
use crate::{cli_relationship_type, rel_remove_matches};
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

/// A git-initialized fixture store, so per-spec commits can be counted.
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

fn commit_count(root: &std::path::Path) -> usize {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
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
fn legacy_spellings_are_recognised_and_nothing_else() {
    assert!(is_legacy_related(&custom("related")));
    assert!(is_legacy_related(&custom("related-to")));
    assert!(is_legacy_related(&custom("relates-to")));
    assert!(is_legacy_related(&custom("Related")));
    assert!(!is_legacy_related(&custom("implements")));
    assert!(!is_legacy_related(&custom("implemented-by")));
    assert!(!is_legacy_related(&custom("sprint_assignment")));
    assert!(!is_legacy_related(&RelationshipType::References));
}

#[test]
fn orphan_is_converted_in_place_keeping_provenance() {
    let t = Uuid::now_v7();
    let reqs = vec![spec("A-1", vec![edge(custom("related"), t)])];
    let plan = plan_migration(&reqs);
    assert_eq!(plan.converted, 1);
    assert_eq!(plan.twins_deleted, 0);
    let s = &plan.specs[0];
    assert_eq!(s.edges[0].action, EdgeAction::Convert);
    assert_eq!(s.new_relationships.len(), 1);
    assert_eq!(
        s.new_relationships[0].rel_type,
        RelationshipType::References
    );
    assert_eq!(
        s.new_relationships[0].created_by.as_deref(),
        Some("fixture")
    );
}

#[test]
fn twin_is_deleted_not_converted() {
    let t = Uuid::now_v7();
    let reqs = vec![spec(
        "A-1",
        vec![
            edge(custom("related"), t),
            edge(RelationshipType::References, t),
        ],
    )];
    let plan = plan_migration(&reqs);
    assert_eq!(plan.converted, 0);
    assert_eq!(plan.twins_deleted, 1);
    let s = &plan.specs[0];
    assert_eq!(
        s.new_relationships,
        vec![edge(RelationshipType::References, t)]
    );
}

#[test]
fn mixed_spec_handles_each_pair_and_leaves_other_edges() {
    let orphan = Uuid::now_v7();
    let twin = Uuid::now_v7();
    let other = Uuid::now_v7();
    let reqs = vec![
        spec(
            "A-1",
            vec![
                edge(custom("related"), orphan),
                edge(custom("related-to"), orphan),
                edge(custom("relates-to"), twin),
                edge(RelationshipType::References, twin),
                edge(custom("implements"), other),
                edge(RelationshipType::Child, other),
            ],
        ),
        spec("A-2", vec![edge(custom("implemented-by"), other)]),
    ];
    let plan = plan_migration(&reqs);
    assert_eq!(
        plan.specs.len(),
        1,
        "a spec without legacy edges is untouched"
    );
    assert_eq!(plan.legacy_edges, 3);
    assert_eq!(plan.converted, 1);
    assert_eq!(plan.twins_deleted, 1);
    assert_eq!(plan.extras_deleted, 1);
    let new = &plan.specs[0].new_relationships;
    assert_eq!(
        new,
        &vec![
            edge(RelationshipType::References, orphan),
            edge(RelationshipType::References, twin),
            edge(custom("implements"), other),
            edge(RelationshipType::Child, other),
        ]
    );
    check_migrated("A-1", new).unwrap();
}

#[test]
fn no_duplicate_assertion_rejects_two_edges_of_one_kind() {
    let t = Uuid::now_v7();
    let dup = vec![
        edge(RelationshipType::References, t),
        edge(RelationshipType::References, t),
    ];
    assert_eq!(duplicate_edges(&dup).len(), 1);
    assert!(check_migrated("A-1", &dup).is_err());
    let leftover = vec![edge(custom("related"), t)];
    assert!(check_migrated("A-1", &leftover).is_err());
    let ok = vec![
        edge(RelationshipType::References, t),
        edge(RelationshipType::Child, t),
    ];
    check_migrated("A-1", &ok).unwrap();
}

#[test]
fn migration_refuses_before_writing_when_a_touched_spec_holds_a_duplicate() {
    let (_dir, root, backend) = open_git_backend();
    let a = add(&backend, "FR-1");
    let b = add(&backend, "FR-2");
    let _ = a;
    set_rels(
        &backend,
        "FR-1",
        vec![
            edge(custom("related"), b.id),
            edge(RelationshipType::Child, b.id),
            edge(RelationshipType::Child, b.id),
        ],
    );
    let before = commit_count(&root);
    let err = run_migration(&backend, false).unwrap_err();
    assert!(err.to_string().contains("more than one"), "{err}");
    assert_eq!(commit_count(&root), before, "nothing written");
    assert!(rels_of(&backend, "FR-1")
        .iter()
        .any(|r| is_legacy_related(&r.rel_type)));
}

#[test]
fn dry_run_writes_nothing() {
    let (_dir, root, backend) = open_git_backend();
    add(&backend, "FR-1");
    let b = add(&backend, "FR-2");
    let original = vec![edge(custom("related"), b.id)];
    set_rels(&backend, "FR-1", original.clone());
    let before = commit_count(&root);
    let status_before = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["status", "--porcelain"])
        .output()
        .unwrap()
        .stdout;

    let outcome = run_migration(&backend, true).unwrap();
    assert!(outcome.dry_run);
    assert_eq!(outcome.plan.converted, 1);
    assert_eq!(outcome.specs_written, 0);
    assert_eq!(commit_count(&root), before);
    let status_after = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["status", "--porcelain"])
        .output()
        .unwrap()
        .stdout;
    assert_eq!(status_before, status_after, "worktree untouched");
    assert_eq!(rels_of(&backend, "FR-1"), original);
}

#[test]
fn apply_writes_one_commit_per_spec_and_is_idempotent() {
    let (_dir, root, backend) = open_git_backend();
    add(&backend, "FR-1");
    let b = add(&backend, "FR-2");
    let c = add(&backend, "FR-3");
    add(&backend, "FR-4");
    // FR-1: orphan; FR-3: twin; FR-4: untouched custom edge.
    set_rels(&backend, "FR-1", vec![edge(custom("related"), b.id)]);
    set_rels(
        &backend,
        "FR-3",
        vec![
            edge(RelationshipType::References, b.id),
            edge(custom("related"), b.id),
        ],
    );
    set_rels(&backend, "FR-4", vec![edge(custom("implements"), c.id)]);
    let before = commit_count(&root);

    let outcome = run_migration(&backend, false).unwrap();
    assert_eq!(outcome.specs_written, 2);
    assert_eq!(outcome.plan.converted, 1);
    assert_eq!(outcome.plan.twins_deleted, 1);
    assert_eq!(
        commit_count(&root),
        before + 2,
        "one targeted commit per migrated spec"
    );
    let last_files = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["show", "--name-only", "--format=", "HEAD"])
        .output()
        .unwrap()
        .stdout;
    let last_files = String::from_utf8_lossy(&last_files);
    let yaml_files: Vec<&str> = last_files
        .lines()
        .filter(|l| l.starts_with("objects/"))
        .collect();
    assert_eq!(
        yaml_files.len(),
        1,
        "a commit touches one spec: {last_files}"
    );

    assert_eq!(
        rels_of(&backend, "FR-1"),
        vec![edge(RelationshipType::References, b.id)]
    );
    assert_eq!(
        rels_of(&backend, "FR-3"),
        vec![edge(RelationshipType::References, b.id)]
    );
    assert_eq!(
        rels_of(&backend, "FR-4"),
        vec![edge(custom("implements"), c.id)]
    );

    // Second run: nothing to do, nothing written.
    let after_first = commit_count(&root);
    let again = run_migration(&backend, false).unwrap();
    assert!(again.plan.is_noop());
    assert_eq!(again.specs_written, 0);
    assert_eq!(commit_count(&root), after_first);
}

#[test]
fn rel_remove_type_matching() {
    let references = RelationshipType::References;
    // `--type related` aliases to references and also matches legacy customs.
    let related = cli_relationship_type("related");
    assert_eq!(related, references);
    assert!(rel_remove_matches(&custom("related"), &related));
    assert!(rel_remove_matches(&custom("related-to"), &related));
    assert!(rel_remove_matches(&references, &related));
    // A legacy spelling asked for explicitly matches the whole family.
    let related_to = cli_relationship_type("related-to");
    assert!(rel_remove_matches(&custom("related"), &related_to));
    assert!(rel_remove_matches(&references, &related_to));
    // Other types never match across.
    assert!(!rel_remove_matches(&RelationshipType::Child, &related));
    assert!(!rel_remove_matches(&custom("implements"), &related));
    assert!(!rel_remove_matches(
        &custom("related"),
        &RelationshipType::Child
    ));
    assert!(!rel_remove_matches(
        &RelationshipType::Parent,
        &RelationshipType::Child
    ));
}
