//! trace:TASK-1981 | ai:codex
// trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
use super::*;
use aida_core::DatabaseBackend;
use std::time::{Duration, Instant};

#[test]
fn task_1981_retirement_waits_and_preserves_concurrent_edit() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    aida_core::git_ops::init(&store).unwrap();
    let backend = crate::test_env::home_fixture_backend(&store).unwrap();
    aida_core::git_ops::configure_user(&store, "Test", "test@example.com").unwrap();
    let mut req = aida_core::Requirement::new("before".into(), "desc".into());
    req.spec_id = Some("TASK-1-001".into());
    req.agreed_id = Some("TASK-101".into());
    backend.add_requirement(req).unwrap();
    let cache = dir.path().join("cache.db");
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let contender = store.clone();
    let child = std::thread::spawn(move || {
        let cached = crate::test_env::home_fixture_cached_backend(&contender, &cache).unwrap();
        ready_tx.send(()).unwrap();
        go_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        done_tx
            .send(handle_retire_legacy_ids(&cached, &contender, false))
            .unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let before = aida_core::db::store_write_lock_contended_for(&store);
    aida_core::db::with_store_write_lock(&store, || {
        go_tx.send(()).unwrap();
        let start = Instant::now();
        while aida_core::db::store_write_lock_contended_for(&store) == before {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "retirement never contended"
            );
            assert!(done_rx.try_recv().is_err(), "retirement escaped lock");
            std::thread::sleep(Duration::from_millis(5));
        }
        backend.update_atomically(|store| {
            store.requirements[0].title = "concurrent edit".into();
        })?;
        Ok(())
    })
    .unwrap();
    done_rx
        .recv_timeout(Duration::from_secs(20))
        .unwrap()
        .unwrap();
    child.join().unwrap();
    let saved = backend.load().unwrap();
    assert_eq!(saved.requirements.len(), 1);
    assert_eq!(saved.requirements[0].spec_id.as_deref(), Some("TASK-101"));
    assert_eq!(saved.requirements[0].title, "concurrent edit");
    assert!(saved.requirements[0].agreed_id.is_none());
    assert!(aida_core::object_store::read_object(&store.join("objects"), "TASK-1-001").is_err());
}
