//! Compaction serializes history rewrites, but never holds the AIDA lock for GC.
//! trace:TASK-1981 | ai:codex
use super::*;
use std::time::{Duration, Instant};

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn seed(repo: &std::path::Path) {
    aida_core::git_ops::init(repo).unwrap();
    aida_core::git_ops::configure_user(repo, "Test", "test@example.com").unwrap();
    git(repo, &["config", "gc.auto", "0"]);
    git(repo, &["config", "commit.gpgSign", "false"]);
    for name in ["one", "two"] {
        std::fs::write(repo.join(name), name).unwrap();
        git(repo, &["add", name]);
        git(repo, &["commit", "-q", "-m", name]);
    }
}

#[test]
fn task_1981_compact_squash_waits_and_includes_peer_commit() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("store");
    seed(&repo);
    let before = aida_core::db::store_write_lock_contended_for(&repo);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut child = None;
    let mut peer_head = String::new();
    aida_core::db::with_store_write_lock(&repo, || {
        let contender = repo.clone();
        child = Some(std::thread::spawn(move || {
            tx.send(store_compact_at(&contender, true, true)).unwrap();
        }));
        let start = Instant::now();
        while aida_core::db::store_write_lock_contended_for(&repo) == before {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "compact never contended"
            );
            assert!(rx.try_recv().is_err(), "compact escaped lock");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(git(&repo, &["rev-list", "--count", "HEAD"]), "2");
        std::fs::write(repo.join("peer"), "peer data").unwrap();
        git(&repo, &["add", "peer"]);
        git(&repo, &["commit", "-q", "-m", "peer"]);
        peer_head = git(&repo, &["rev-parse", "HEAD"]);
        Ok(())
    })
    .unwrap();
    rx.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
    child.unwrap().join().unwrap();
    assert_eq!(git(&repo, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(git(&repo, &["show", "HEAD:peer"]), "peer data");
    let backup = git(
        &repo,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "refs/heads/aida-store-pre-squash-*",
        ],
    );
    assert_eq!(backup, peer_head);
    let counts = aida_core::git_ops::count_objects(&repo).unwrap();
    assert_eq!(
        counts.packs, 1,
        "post-squash GC must actually run after unlocking"
    );
    assert_eq!(counts.loose, 0);
}

#[test]
fn task_1981_compact_gc_does_not_wait_for_store_writer() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("store");
    seed(&repo);
    let (tx, rx) = std::sync::mpsc::channel();
    aida_core::db::with_store_write_lock(&repo, || {
        let contender = repo.clone();
        let child = std::thread::spawn(move || {
            tx.send(store_compact_at(&contender, false, true)).unwrap();
        });
        rx.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
        child.join().unwrap();
        assert_eq!(git(&repo, &["rev-list", "--count", "HEAD"]), "2");
        Ok(())
    })
    .unwrap();
}
