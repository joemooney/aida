//! Regression coverage for locked heals and confirmed repair sets.
//! trace:TASK-1981 | ai:codex
use super::*;
use aida_core::{AgreedIdBlock, BlockRegistry, Relationship, RelationshipType, Requirement};
use std::collections::HashSet;
use std::time::{Duration, Instant};

#[test]
fn task_1981_gitignore_heal_waits_for_writer_lock() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    let before = aida_core::db::store_write_lock_contended_for(&store);
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let mut child = None;
    aida_core::db::with_store_write_lock(&store, || {
        let contender = store.clone();
        child = Some(std::thread::spawn(move || {
            let result = ensure_store_tracked_runtime_gitignore(&contender);
            done_tx.send(result).unwrap();
        }));
        let start = Instant::now();
        while aida_core::db::store_write_lock_contended_for(&store) == before {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "heal never contended"
            );
            assert!(done_rx.try_recv().is_err(), "heal escaped lock");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!store.join(".gitignore").exists());
        Ok(())
    })
    .unwrap();
    assert!(done_rx
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap());
    child.unwrap().join().unwrap();
}

#[test]
fn task_1981_relationship_repair_intersects_confirmed_edges_and_rechecks_targets() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    aida_core::git_ops::init(&store).unwrap();
    aida_core::git_ops::configure_user(&store, "Test", "test@example.com").unwrap();
    let mut req = Requirement::new("source".into(), "".into());
    req.spec_id = Some("TASK-1".into());
    let missing = uuid::Uuid::new_v4();
    let new_missing = uuid::Uuid::new_v4();
    let mut appeared = Requirement::new("appeared since preview".into(), "".into());
    appeared.spec_id = Some("TASK-2".into());
    for target_id in [missing, new_missing, appeared.id] {
        req.relationships.push(Relationship {
            rel_type: RelationshipType::References,
            target_id,
            created_at: None,
            created_by: None,
        });
    }
    // A newly added edge of another type to the same missing target was
    // never confirmed and must survive.
    req.relationships.push(Relationship {
        rel_type: RelationshipType::Parent,
        target_id: missing,
        created_at: None,
        created_by: None,
    });
    let confirmed = HashSet::from([
        (req.id, missing, RelationshipType::References),
        (req.id, appeared.id, RelationshipType::References),
    ]);
    let objects = store.join("objects");
    aida_core::object_store::write_object(&objects, &req).unwrap();
    aida_core::object_store::write_object(&objects, &appeared).unwrap();
    assert_eq!(
        doctor_repair_dangling_relationships_in_store(&store, Some(&confirmed)).unwrap(),
        1
    );
    let saved = aida_core::object_store::read_object(&objects, "TASK-1").unwrap();
    assert_eq!(saved.relationships.len(), 3);
    assert!(saved
        .relationships
        .iter()
        .any(|r| r.target_id == new_missing));
    assert!(saved
        .relationships
        .iter()
        .any(|r| r.target_id == appeared.id));
    assert_eq!(
        doctor_repair_dangling_relationships_in_store(&store, None).unwrap(),
        1
    );
}

fn block(node: &str, prefix: &str, start: u32) -> AgreedIdBlock {
    AgreedIdBlock {
        node_id: node.into(),
        owner: "test".into(),
        hostname: "host".into(),
        type_prefix: prefix.into(),
        range_start: start,
        range_end: start + 9,
        next: start,
        allocated_at: chrono::Utc::now(),
    }
}

#[test]
fn task_1981_stale_block_repair_leaves_unconfirmed_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    aida_core::git_ops::init(&store).unwrap();
    aida_core::git_ops::configure_user(&store, "Test", "test@example.com").unwrap();
    let path = store.join("registry/blocks.yaml");
    let mut registry = BlockRegistry::default();
    registry.blocks = vec![block("gone", "TASK", 1), block("gone", "TASK", 11)];
    registry.save(&path).unwrap();
    let confirmed = HashSet::from([("gone".into(), "TASK".into(), 1, 10)]);
    assert_eq!(
        doctor_tombstone_stale_blocks_in_store(&store, Some(&confirmed)).unwrap(),
        1
    );
    let saved = BlockRegistry::load(&path).unwrap();
    assert!(saved.blocks[0].is_exhausted());
    assert_eq!(saved.blocks[1].next, 11);
    assert_eq!(
        doctor_tombstone_stale_blocks_in_store(&store, None).unwrap(),
        1
    );
}

#[test]
fn task_1981_counter_migration_refuses_unconfirmed_active_blocks_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let store = project.join(".aida-store");
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    let config = project.join(".aida/config.toml");
    std::fs::write(&config, "[id_format]\ncounter_scope = \"per_type\"\n").unwrap();
    aida_core::git_ops::init(&store).unwrap();
    let path = store.join("registry/blocks.yaml");
    let mut registry = BlockRegistry::default();
    registry.blocks = vec![block("n1", "TASK", 1), block("n1", "BUG", 11)];
    registry.save(&path).unwrap();
    let old_blocks = std::fs::read(&path).unwrap();
    let old_config = std::fs::read(&config).unwrap();
    let confirmed = HashSet::from([("TASK".into(), 1, 10)]);
    let err = doctor_migrate_counter_scope_in_store(project, &store, "n1", 100, Some(&confirmed))
        .unwrap_err();
    assert!(
        err.to_string().contains("changed since confirmation"),
        "{err:#}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), old_blocks);
    assert_eq!(std::fs::read(&config).unwrap(), old_config);
}
