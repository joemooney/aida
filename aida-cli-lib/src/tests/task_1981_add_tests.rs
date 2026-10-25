//! Agreed-ID add versus sync lock ordering. trace:TASK-1981 | ai:codex
use super::*;
use aida_core::DatabaseBackend;
use std::time::{Duration, Instant};

#[test]
fn task_1981_run_add_waits_for_sync_before_taking_blocks_lock_and_reloads_pointer() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let store = project.join(".aida-store");
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/config.toml"),
        "[id_format]\npolicy = \"blocks-only\"\n[block_allocation]\nauto_claim = false\n",
    )
    .unwrap();
    std::env::set_current_dir(project).unwrap();
    aida_core::git_ops::init(&store).unwrap();
    aida_core::git_ops::configure_user(&store, "Test", "test@example.com").unwrap();
    for (key, value) in [
        ("maintenance.auto", "false"),
        ("gc.auto", "0"),
        ("commit.gpgSign", "false"),
    ] {
        assert!(aida_core::git_ops::host_git_command(&store)
            .args(["config", key, value])
            .status()
            .unwrap()
            .success());
    }
    crate::test_env::enroll_synthetic_home(&store).unwrap();
    let backend = aida_core::GitBackend::new(&store).unwrap();
    backend.save(&aida_core::RequirementsStore::new()).unwrap();
    let blocks_path = store.join("registry/blocks.yaml");
    let mut registry = aida_core::BlockRegistry::default();
    registry.blocks.push(aida_core::AgreedIdBlock {
        node_id: "1".into(),
        owner: "test".into(),
        hostname: "host".into(),
        type_prefix: "TASK".into(),
        range_start: 100,
        range_end: 199,
        next: 100,
        allocated_at: chrono::Utc::now(),
    });
    registry.save(&blocks_path).unwrap();
    aida_core::git_ops::add(&store, &["registry/blocks.yaml"]).unwrap();
    aida_core::git_ops::commit(&store, "seed agreed-id block").unwrap();
    let cache = project.join("cache.db");
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let contender = store.clone();
    let child = std::thread::spawn(move || {
        let cached = aida_core::CachedGitBackend::open(&contender, &cache).unwrap();
        let matches = (ADD.command)()
            .try_get_matches_from(["add", "--title", "barrier add", "--type", "task"])
            .unwrap();
        let ctx = crate::command_registry::DispatchCtx {
            globals: None,
            store: Some(crate::command_registry::StoreCtx::Git {
                store_path: &contender,
                backend: &cached,
            }),
        };
        ready_tx.send(()).unwrap();
        go_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        done_tx.send(run_add(&matches, &ctx).map(|_| ())).unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let before = aida_core::db::store_write_lock_contended_for(&store);
    aida_core::db::with_store_write_lock(&store, || {
        aida_core::BlockRegistry::with_dispense_lock(&blocks_path, || {
            let head = aida_core::git_ops::head_sha(&store)?;
            go_tx.send(()).unwrap();
            let started = Instant::now();
            while aida_core::db::store_write_lock_contended_for(&store) == before {
                assert!(
                    started.elapsed() < Duration::from_secs(10),
                    "add did not reach the outer store lock while sync held blocks.lock"
                );
                assert!(
                    done_rx.try_recv().is_err(),
                    "add escaped the sync transaction"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(aida_core::git_ops::head_sha(&store)?, head);
            assert_eq!(
                aida_core::BlockRegistry::load(&blocks_path)?.blocks[0].next,
                100
            );
            assert!(backend.load()?.requirements.is_empty());
            // A sync consumes one ID while add waits; add must see the new pointer.
            assert_eq!(registry.dispense("1", "TASK").unwrap().0, "TASK-100");
            registry.save(&blocks_path)?;
            aida_core::git_ops::add(&store, &["registry/blocks.yaml"])?;
            aida_core::git_ops::commit(&store, "sync advances allocation")?;
            Ok(())
        })
    })
    .unwrap();
    done_rx
        .recv_timeout(Duration::from_secs(20))
        .unwrap()
        .unwrap();
    child.join().unwrap();
    assert_eq!(
        aida_core::BlockRegistry::load(&blocks_path).unwrap().blocks[0].next,
        102
    );
    let saved = backend.load().unwrap();
    assert_eq!(saved.requirements.len(), 1);
    assert_eq!(saved.requirements[0].spec_id.as_deref(), Some("TASK-101"));
    assert_eq!(saved.requirements[0].agreed_id.as_deref(), Some("TASK-101"));
}
