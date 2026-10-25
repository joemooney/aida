//! Regression coverage for locked heals and confirmed repair sets.
//! trace:TASK-1981 | ai:codex
// trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
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
    // trace:BUG-2017 | ai:codex — explicit positive global-ID repair fixture.
    crate::test_env::enroll_synthetic_home(&store).unwrap();
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
    // trace:BUG-2017 | ai:codex — explicit positive global-ID repair fixture.
    crate::test_env::enroll_synthetic_home(&store).unwrap();
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
    // trace:BUG-2017 | ai:codex — explicit positive global-ID repair fixture.
    crate::test_env::enroll_synthetic_home(&store).unwrap();
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

// Observe real contention before changing the fixture. Without the outer heal
// lock this catches the backend save lock too; the peer write below then proves
// the heal reloads after acquiring, rather than saving its stale preview.
// trace:TASK-1981 | ai:codex
#[test]
fn task_1981_parent_tag_heal_waits_and_reloads_peer_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let store_path = root.join(".aida-store");
    std::fs::create_dir_all(&store_path).unwrap();
    let storage = crate::test_env::home_fixture_storage(&store_path);
    let mut store = aida_core::RequirementsStore::default();
    let mut req = Requirement::new("original".into(), "".into());
    req.spec_id = Some("TASK-1".into());
    req.tags.insert("parent:EPIC-OLD".into());
    store.requirements.push(req);
    storage.save(&store).unwrap();
    let before = aida_core::db::store_write_lock_contended_for(&store_path);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut child = None;
    aida_core::db::with_store_write_lock(&store_path, || {
        let root = root.clone();
        child = Some(std::thread::spawn(move || {
            let finding = DoctorFinding {
                category: "parent-tag-drift".into(),
                id: "TASK-1".into(),
                summary: "stale parent tag".into(),
                action: "heal".into(),
                safe_heal: true,
            };
            tx.send(heal_doctor_parent_tag_drift(&root, &finding))
                .unwrap();
        }));
        let start = Instant::now();
        while aida_core::db::store_write_lock_contended_for(&store_path) == before {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "heal never contended"
            );
            assert!(rx.try_recv().is_err(), "heal escaped lock");
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut peer = storage.load()?;
        let mut parent = Requirement::new("new parent".into(), "".into());
        parent.spec_id = Some("EPIC-2".into());
        let source = peer.get_requirement_by_spec_id("TASK-1").unwrap().id;
        let req = peer.get_requirement_by_id_mut(&source).unwrap();
        req.title = "peer updated".into();
        req.relationships.push(Relationship {
            rel_type: RelationshipType::Child,
            target_id: parent.id,
            created_at: None,
            created_by: None,
        });
        peer.requirements.push(parent);
        storage.save(&peer)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(20))
            .unwrap()
            .unwrap()
            .status,
        "healed"
    );
    child.unwrap().join().unwrap();
    let saved = storage.load().unwrap();
    let req = saved.get_requirement_by_spec_id("TASK-1").unwrap();
    assert_eq!(req.title, "peer updated");
    assert!(req.tags.contains("parent:EPIC-2"));
    assert!(!req.tags.contains("parent:EPIC-OLD"));
}

// trace:TASK-1981 | ai:codex
#[test]
fn task_1981_orphan_queue_heal_waits_and_rechecks_removal() {
    let _env = crate::test_env::env_lock();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let store_path = root.join(".aida-store");
    std::fs::create_dir_all(&store_path).unwrap();
    let storage = crate::test_env::home_fixture_storage(&store_path);
    storage
        .save(&aida_core::RequirementsStore::default())
        .unwrap();
    let user = current_user_id(None);
    let orphan = uuid::Uuid::new_v4();
    storage
        .queue_add(aida_core::models::QueueEntry {
            user_id: user.clone(),
            requirement_id: orphan,
            position: 1,
            added_by: user.clone(),
            note: None,
            added_at: chrono::Utc::now(),
            for_role: Some("reviewer".into()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        })
        .unwrap();
    let before = aida_core::db::store_write_lock_contended_for(&store_path);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut child = None;
    aida_core::db::with_store_write_lock(&store_path, || {
        let root = root.clone();
        child = Some(std::thread::spawn(move || {
            let finding = DoctorFinding {
                category: "orphan-queue-entries".into(),
                id: orphan.to_string(),
                summary: "orphan".into(),
                action: "remove".into(),
                safe_heal: true,
            };
            tx.send(heal_doctor_orphan_queue_entry(&root, &finding))
                .unwrap();
        }));
        let start = Instant::now();
        while aida_core::db::store_write_lock_contended_for(&store_path) == before {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "heal never contended"
            );
            assert!(rx.try_recv().is_err(), "heal escaped lock");
            std::thread::sleep(Duration::from_millis(5));
        }
        storage.queue_remove(&user, &orphan)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(20))
            .unwrap()
            .unwrap()
            .status,
        "skipped"
    );
    child.unwrap().join().unwrap();
    assert!(storage.queue_list(&user, false).unwrap().is_empty());
}

#[test]
fn task_1981_runtime_untracking_waits_and_commits_after_peer() {
    use aida_core::git_ops;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let store = root.join(".aida-store");
    git_ops::init(&store).unwrap();
    git_ops::configure_user(&store, "Test", "test@example.com").unwrap();
    let git = |args: &[&str]| {
        let output = git_ops::host_git_command(&store)
            .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    for (key, value) in [
        ("maintenance.auto", "false"),
        ("gc.auto", "0"),
        ("commit.gpgSign", "false"),
    ] {
        git(&["config", key, value]);
    }
    std::fs::create_dir_all(store.join(".aida")).unwrap();
    let runtime = store.join(".aida/node.toml");
    std::fs::write(&runtime, "node_id = 'n1'\n").unwrap();
    git(&["add", ".aida/node.toml"]);
    git(&["commit", "-qm", "seed tracked runtime"]);
    let head = git_ops::head_sha(&store).unwrap();
    let index = std::fs::read(store.join(".git/index")).unwrap();
    let before = aida_core::db::store_write_lock_contended_for(&store);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut child = None;
    let mut peer = String::new();
    aida_core::db::with_store_write_lock(&store, || {
        let root = root.clone();
        child = Some(std::thread::spawn(move || {
            let finding = DoctorFinding {
                category: "store-tracked-runtime".into(),
                id: ".aida/node.toml".into(),
                summary: "runtime".into(),
                action: "untrack".into(),
                safe_heal: true,
            };
            tx.send(heal_doctor_store_tracked_runtime(&root, &finding))
                .unwrap();
        }));
        let started = Instant::now();
        while aida_core::db::store_write_lock_contended_for(&store) == before {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "heal never contended"
            );
            assert!(rx.try_recv().is_err(), "heal escaped writer lock");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(git_ops::head_sha(&store)?, head);
        assert_eq!(std::fs::read(store.join(".git/index"))?, index);
        assert!(!store.join(".gitignore").exists());
        std::fs::write(store.join("peer.txt"), "peer")?;
        git(&["add", "peer.txt"]);
        git(&["commit", "-qm", "peer"]);
        peer = git_ops::head_sha(&store)?;
        Ok(())
    })
    .unwrap();
    let result = rx.recv_timeout(Duration::from_secs(20)).unwrap().unwrap();
    child.unwrap().join().unwrap();
    assert_eq!(result.status, "healed");
    assert_eq!(git(&["rev-parse", "HEAD^"]), peer);
    assert_eq!(git(&["show", "HEAD:peer.txt"]), "peer");
    assert!(git(&["ls-files", ".aida/node.toml"]).is_empty());
    assert_eq!(
        std::fs::read_to_string(&runtime).unwrap(),
        "node_id = 'n1'\n"
    );
    assert!(git(&["show", "HEAD:.gitignore"]).contains(".aida/node.toml"));
    assert!(git(&["status", "--porcelain"]).is_empty());
}
