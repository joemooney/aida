// Handler-level fixture tests for `aida rel remove`, invoked through the
// real `Command::Rel(RelationshipCommand::Remove { .. })` dispatch path
// (`handle_git_backend_command`) rather than only the pure
// `rel_remove_matches` helper `task_1426_related_edge_migration_tests.rs`
// already covers. Every fixture is a throwaway git store under a tempdir;
// none of this ever touches a real AIDA store.
//
// Gaps closed (BUG-1602, found by the TASK-1426 review):
//   1. `rel remove --type parent|child` without `-b` used to leave the
//      reciprocal edge `rel add` wrote behind — fixed by mirroring
//      `rel add`'s `rel_should_write_inverse` in the remove path.
//   2. The CLI type parser rejected spellings core/MCP accepted
//      (`verified_by`, `depends-on`, `replaced-by`) — fixed by routing all
//      three surfaces through one shared parser in aida-core.
// trace:BUG-1602 | ai:claude

use crate::{Command, RelationshipCommand};
use aida_core::db::DatabaseBackend;
use aida_core::models::{Relationship, RelationshipType, Requirement};
use aida_core::CachedGitBackend;
use tempfile::tempdir;

fn custom(name: &str) -> RelationshipType {
    RelationshipType::Custom(name.to_string())
}

fn edge(rel_type: RelationshipType, target: uuid::Uuid) -> Relationship {
    Relationship {
        rel_type,
        target_id: target,
        created_at: None,
        created_by: Some("fixture".into()),
    }
}

/// A git-initialized fixture store plus its cache-backed handle, so setup
/// (seeding requirements/edges) can go through the same backend the handler
/// re-opens from `store_path`.
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
    let mut r = Requirement::new(spec_id.into(), "desc".into());
    r.spec_id = Some(spec_id.into());
    backend.add_requirement(r).unwrap()
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

/// Runs `aida rel remove <from> <to> --type <type> [-b]` through the real
/// command handler against `store_path`.
fn rel_remove(store_path: &std::path::Path, from: &str, to: &str, r#type: &str, bidi: bool) {
    let cmd = Command::Rel(RelationshipCommand::Remove {
        from_pos: Some(from.to_string()),
        to_pos: Some(to.to_string()),
        from_flag: None,
        to_flag: None,
        r#type: r#type.to_string(),
        bidirectional: bidi,
    });
    crate::git_backend_cmd::handle_git_backend_command(store_path, &cmd).unwrap();
}

// --- 1. Typed removal: only the named type comes off, a different edge to
// the same target survives. ---
#[test]
fn typed_removal_leaves_other_edge_types_to_same_target_untouched() {
    let (_dir, root, backend) = open_git_backend();
    add(&backend, "A-1");
    let b = add(&backend, "A-2");
    set_rels(
        &backend,
        "A-1",
        vec![
            edge(RelationshipType::Duplicate, b.id),
            edge(RelationshipType::References, b.id),
        ],
    );

    rel_remove(&root, "A-1", "A-2", "duplicate", false);

    assert_eq!(
        rels_of(&backend, "A-1"),
        vec![edge(RelationshipType::References, b.id)],
        "only the duplicate edge is removed"
    );
}

/// Also exercises the shared-parser aliases now accepted CLI-side
/// (BUG-1602 point 2): `verified_by` (underscore) used to be rejected by
/// `cli_relationship_type`, so a typed remove with it found nothing.
#[test]
fn typed_removal_accepts_the_shared_parser_aliases() {
    let (_dir, root, backend) = open_git_backend();
    add(&backend, "A-1");
    let b = add(&backend, "A-2");
    set_rels(
        &backend,
        "A-1",
        vec![edge(RelationshipType::VerifiedBy, b.id)],
    );

    rel_remove(&root, "A-1", "A-2", "verified_by", false);

    assert!(
        rels_of(&backend, "A-1").is_empty(),
        "the underscore spelling must resolve to VerifiedBy, same as `rel add`"
    );
}

#[test]
fn typed_removal_accepts_depends_on_and_replaced_by_aliases() {
    let (_dir, root, backend) = open_git_backend();
    add(&backend, "A-1");
    let b = add(&backend, "A-2");
    add(&backend, "A-3");
    let c = backend.get_requirement_by_spec_id("A-3").unwrap().unwrap();
    set_rels(
        &backend,
        "A-1",
        vec![
            edge(RelationshipType::BlockedBy, b.id),
            edge(RelationshipType::SupersededBy, c.id),
        ],
    );

    rel_remove(&root, "A-1", "A-2", "depends-on", false);
    rel_remove(&root, "A-1", "A-3", "replaced-by", false);

    assert!(
        rels_of(&backend, "A-1").is_empty(),
        "depends-on -> BlockedBy and replaced-by -> SupersededBy must both resolve"
    );
}

// --- 2. --bidirectional: an explicit -b removes the inverse edge for a
// type that ISN'T in the forced parent/child pair. ---
#[test]
fn bidirectional_flag_removes_inverse_for_a_non_forced_pair() {
    let (_dir, root, backend) = open_git_backend();
    let a = add(&backend, "A-1");
    let b = add(&backend, "A-2");
    set_rels(
        &backend,
        "A-1",
        vec![edge(RelationshipType::BlockedBy, b.id)],
    );
    set_rels(&backend, "A-2", vec![edge(RelationshipType::Blocks, a.id)]);

    rel_remove(&root, "A-1", "A-2", "blocked-by", true);

    assert!(rels_of(&backend, "A-1").is_empty(), "forward edge removed");
    assert!(
        rels_of(&backend, "A-2").is_empty(),
        "-b must remove the reciprocal Blocks edge too"
    );
}

/// Without `-b`, a non-forced pair leaves the reciprocal edge alone — the
/// existing, still-correct behavior this bug does NOT change.
#[test]
fn without_bidirectional_flag_a_non_forced_pair_keeps_the_inverse() {
    let (_dir, root, backend) = open_git_backend();
    let a = add(&backend, "A-1");
    let b = add(&backend, "A-2");
    set_rels(
        &backend,
        "A-1",
        vec![edge(RelationshipType::BlockedBy, b.id)],
    );
    set_rels(&backend, "A-2", vec![edge(RelationshipType::Blocks, a.id)]);

    rel_remove(&root, "A-1", "A-2", "blocked-by", false);

    assert!(rels_of(&backend, "A-1").is_empty(), "forward edge removed");
    assert_eq!(
        rels_of(&backend, "A-2"),
        vec![edge(RelationshipType::Blocks, a.id)],
        "no -b, no inverse cleanup, for a non-forced pair"
    );
}

// --- 3. The parent/child pair: remove always mirrors add, dropping BOTH
// reciprocal edges even without `-b`. ---
#[test]
fn child_removal_without_bidirectional_also_drops_the_parent_edge() {
    let (_dir, root, backend) = open_git_backend();
    let parent = add(&backend, "EPIC-1");
    let child = add(&backend, "TASK-1");
    // What `aida rel add TASK-1 EPIC-1 --type child` (no -b) actually writes:
    // both reciprocal edges, per `rel_should_write_inverse`.
    set_rels(
        &backend,
        "TASK-1",
        vec![edge(RelationshipType::Child, parent.id)],
    );
    set_rels(
        &backend,
        "EPIC-1",
        vec![edge(RelationshipType::Parent, child.id)],
    );

    rel_remove(&root, "TASK-1", "EPIC-1", "child", false);

    assert!(
        rels_of(&backend, "TASK-1").is_empty(),
        "the child's own Child edge is removed"
    );
    assert!(
        rels_of(&backend, "EPIC-1").is_empty(),
        "the parent's reciprocal Parent edge must be removed too, matching rel add \
         (this was the bug: it used to survive without -b)"
    );
}

#[test]
fn parent_removal_without_bidirectional_also_drops_the_child_edge() {
    let (_dir, root, backend) = open_git_backend();
    let parent = add(&backend, "EPIC-1");
    let child = add(&backend, "TASK-1");
    set_rels(
        &backend,
        "EPIC-1",
        vec![edge(RelationshipType::Parent, child.id)],
    );
    set_rels(
        &backend,
        "TASK-1",
        vec![edge(RelationshipType::Child, parent.id)],
    );

    rel_remove(&root, "EPIC-1", "TASK-1", "parent", false);

    assert!(rels_of(&backend, "EPIC-1").is_empty());
    assert!(
        rels_of(&backend, "TASK-1").is_empty(),
        "removing from the parent side must also drop the child's reciprocal edge"
    );
}

/// Two children under one epic: removing one child edge must not disturb
/// the sibling's.
#[test]
fn child_removal_only_touches_the_named_target() {
    let (_dir, root, backend) = open_git_backend();
    let epic = add(&backend, "EPIC-1");
    let a = add(&backend, "TASK-1");
    let b = add(&backend, "TASK-2");
    set_rels(
        &backend,
        "TASK-1",
        vec![edge(RelationshipType::Child, epic.id)],
    );
    set_rels(
        &backend,
        "TASK-2",
        vec![edge(RelationshipType::Child, epic.id)],
    );
    set_rels(
        &backend,
        "EPIC-1",
        vec![
            edge(RelationshipType::Parent, a.id),
            edge(RelationshipType::Parent, b.id),
        ],
    );

    rel_remove(&root, "TASK-1", "EPIC-1", "child", false);

    assert!(rels_of(&backend, "TASK-1").is_empty());
    assert_eq!(
        rels_of(&backend, "EPIC-1"),
        vec![edge(RelationshipType::Parent, b.id)],
        "the sibling's Parent edge survives"
    );
    assert_eq!(
        rels_of(&backend, "TASK-2"),
        vec![edge(RelationshipType::Child, epic.id)],
        "the sibling itself is untouched"
    );
}

// --- 4. The legacy Custom-related family: `--type references` (and its
// `related` alias) also removes stored legacy `related`/`related-to`/
// `relates-to` custom edges to the same target. ---
#[test]
fn references_removal_also_clears_a_legacy_related_custom_edge() {
    let (_dir, root, backend) = open_git_backend();
    add(&backend, "A-1");
    let b = add(&backend, "A-2");
    set_rels(
        &backend,
        "A-1",
        vec![
            edge(custom("related"), b.id),
            edge(custom("related-to"), b.id),
        ],
    );

    rel_remove(&root, "A-1", "A-2", "references", false);

    assert!(
        rels_of(&backend, "A-1").is_empty(),
        "`references` must also sweep the legacy related/related-to family"
    );
}

#[test]
fn related_alias_removal_matches_the_same_legacy_family() {
    let (_dir, root, backend) = open_git_backend();
    add(&backend, "A-1");
    let b = add(&backend, "A-2");
    set_rels(
        &backend,
        "A-1",
        vec![
            edge(custom("relates-to"), b.id),
            edge(RelationshipType::References, b.id),
        ],
    );

    rel_remove(&root, "A-1", "A-2", "related", false);

    assert!(
        rels_of(&backend, "A-1").is_empty(),
        "the `related` alias must match both the standard References edge and \
         the legacy custom spelling to the same target"
    );
}

/// A legacy related edge to a DIFFERENT target is not swept by a `references`
/// removal aimed at one target.
#[test]
fn references_removal_does_not_touch_a_legacy_related_edge_to_another_target() {
    let (_dir, root, backend) = open_git_backend();
    add(&backend, "A-1");
    let b = add(&backend, "A-2");
    let c = add(&backend, "A-3");
    set_rels(
        &backend,
        "A-1",
        vec![
            edge(RelationshipType::References, b.id),
            edge(custom("related"), c.id),
        ],
    );

    rel_remove(&root, "A-1", "A-2", "references", false);

    assert_eq!(
        rels_of(&backend, "A-1"),
        vec![edge(custom("related"), c.id)],
        "the unrelated target's legacy edge survives"
    );
}
