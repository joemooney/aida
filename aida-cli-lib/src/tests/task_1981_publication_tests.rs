// Shared bounded fixtures for the two store-publication wrappers.
// trace:TASK-1981 | ai:codex
use aida_core::{git_ops, DatabaseBackend};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn git_at(repo: &Path, args: &[&str]) -> String {
    let out = git_ops::host_git_command(repo)
        .args([
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "-c",
            "commit.gpgSign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn configure(repo: &Path) {
    for (key, value) in [
        ("maintenance.auto", "false"),
        ("gc.auto", "0"),
        ("commit.gpgSign", "false"),
        ("rebase.autoStash", "false"),
    ] {
        git_at(repo, &["config", key, value]);
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: PathBuf,
    bare: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("store");
        let bare = dir.path().join("origin.git");
        std::fs::create_dir_all(&bare).unwrap();
        git_at(&bare, &["init", "--bare", "-q", "-b", "main"]);
        git_ops::init(&store).unwrap();
        configure(&store);
        configure(&bare);
        git_ops::configure_user(&store, "Test", "test@example.com").unwrap();
        git_at(&store, &["checkout", "-B", "main"]);
        git_at(&store, &["remote", "add", "origin", bare.to_str().unwrap()]);
        let backend = aida_core::GitBackend::new(&store).unwrap();
        backend.save(&aida_core::RequirementsStore::new()).unwrap();
        std::fs::create_dir_all(store.join("objects")).unwrap();
        Self {
            _dir: dir,
            store,
            bare,
        }
    }
    fn advance(&self, name: &str) -> String {
        std::fs::write(self.store.join(name), name).unwrap();
        git_ops::add(&self.store, &[name]).unwrap();
        git_ops::commit(&self.store, name).unwrap();
        git_ops::head_sha(&self.store).unwrap()
    }
    fn remote_head(&self) -> String {
        git_at(&self.bare, &["rev-parse", "refs/heads/main"])
    }
    fn remote_advance(&self) {
        self.remote_advance_with_file("remote.txt", "remote");
    }
    fn remote_advance_with_file(&self, name: &str, contents: &str) {
        let peer = self._dir.path().join("peer");
        git_at(
            self._dir.path(),
            &[
                "clone",
                "-q",
                "--branch",
                "main",
                self.bare.to_str().unwrap(),
                peer.to_str().unwrap(),
            ],
        );
        configure(&peer);
        git_ops::configure_user(&peer, "Peer", "peer@example.com").unwrap();
        std::fs::create_dir_all(peer.join(name).parent().unwrap()).unwrap();
        std::fs::write(peer.join(name), contents).unwrap();
        git_ops::add(&peer, &[name]).unwrap();
        git_ops::commit(&peer, "remote advance").unwrap();
        assert!(git_ops::push(&peer, "origin", "main").unwrap());
    }
}

#[test]
fn task_1981_capture_waits_for_writer_and_returns_locked_peer_head() {
    let fixture = Fixture::new();
    let store = &fixture.store;
    let before_head = git_ops::head_sha(store).unwrap();
    let before_metadata = std::fs::read(store.join("metadata.yaml")).unwrap();
    let before_index = std::fs::read(store.join(".git/index")).unwrap();
    let contended = aida_core::db::store_write_lock_contended_for(store);
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let mut child = None;
    aida_core::db::with_store_write_lock(store, || {
        let contender = store.clone();
        child = Some(std::thread::spawn(move || {
            done_tx.send(capture_snapshot(&contender)).unwrap();
        }));
        let start = Instant::now();
        while aida_core::db::store_write_lock_contended_for(store) == contended {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "capture never contended"
            );
            assert!(done_rx.try_recv().is_err(), "capture escaped writer lock");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(git_ops::head_sha(store)?, before_head);
        assert_eq!(std::fs::read(store.join("metadata.yaml"))?, before_metadata);
        assert_eq!(std::fs::read(store.join(".git/index"))?, before_index);
        assert!(!store.join("objects/.gitkeep").exists());
        fixture.advance("peer.txt");
        Ok(())
    })
    .unwrap();
    let (branch, captured) = done_rx
        .recv_timeout(Duration::from_secs(20))
        .unwrap()
        .unwrap();
    child.unwrap().join().unwrap();
    assert_eq!(branch, "main");
    assert_eq!(captured, git_ops::head_sha(store).unwrap());
    assert_eq!(git_at(store, &["show", "HEAD:peer.txt"]), "peer.txt");
}

#[test]
fn task_1981_publication_does_not_publish_a_later_local_commit() {
    let fixture = Fixture::new();
    let (branch, captured) = capture_snapshot(&fixture.store).unwrap();
    let later = fixture.advance("late-peer.txt");
    assert!(!publish_snapshot(&fixture.store, &branch, &captured).unwrap());
    assert_eq!(fixture.remote_head(), captured);
    assert_eq!(git_ops::head_sha(&fixture.store).unwrap(), later);
}

#[test]
fn task_1981_publication_rebases_and_publishes_retry_snapshot() {
    let fixture = Fixture::new();
    let (branch, captured) = capture_snapshot(&fixture.store).unwrap();
    assert!(!publish_snapshot(&fixture.store, &branch, &captured).unwrap());
    fixture.remote_advance();
    let local = fixture.advance("local.txt");
    assert!(publish_snapshot(&fixture.store, &branch, &local).unwrap());
    assert_eq!(
        fixture.remote_head(),
        git_ops::head_sha(&fixture.store).unwrap()
    );
    assert_eq!(
        git_at(&fixture.bare, &["show", "main:remote.txt"]),
        "remote"
    );
    assert_eq!(
        git_at(&fixture.bare, &["show", "main:local.txt"]),
        "local.txt"
    );
}

#[test]
fn task_1981_retry_rejection_remains_an_error() {
    let fixture = Fixture::new();
    let (branch, captured) = capture_snapshot(&fixture.store).unwrap();
    assert!(!publish_snapshot(&fixture.store, &branch, &captured).unwrap());
    fixture.remote_advance();
    let remote_before = fixture.remote_head();
    let local = fixture.advance("local.txt");
    let hook = fixture.bare.join("hooks/pre-receive");
    crate::test_exec::write_executable(
        &hook,
        "#!/bin/sh\necho task-1981-refuse-retry >&2\nexit 1\n",
    );
    let error = publish_snapshot(&fixture.store, &branch, &local).unwrap_err();
    assert!(
        format!("{error:#}").contains("task-1981-refuse-retry"),
        "{error:#}"
    );
    assert_eq!(fixture.remote_head(), remote_before);
}

#[test]
fn task_1981_capture_preserves_stage_errors_without_advancing_head() {
    let fixture = Fixture::new();
    let before = git_ops::head_sha(&fixture.store).unwrap();
    // A directory at index.lock cannot be reaped as an abandoned regular
    // lock. Preserve the refusal instead of treating a failed stage as a no-op.
    let lock = fixture.store.join(".git/index.lock");
    std::fs::create_dir(&lock).unwrap();
    // Trigger the release wrapper's checked change path too.
    std::fs::write(fixture.store.join("objects/pending.txt"), "pending").unwrap();
    let error = capture_snapshot(&fixture.store).unwrap_err();
    assert!(format!("{error:#}").contains("index.lock"), "{error:#}");
    assert_eq!(git_ops::head_sha(&fixture.store).unwrap(), before);
    assert!(lock.exists());
}

#[test]
fn task_1981_retry_waits_for_writer_and_captures_rebased_peer() {
    let fixture = Fixture::new();
    let (branch, captured) = capture_snapshot(&fixture.store).unwrap();
    assert!(!publish_snapshot(&fixture.store, &branch, &captured).unwrap());
    fixture.remote_advance();
    let before_head = fixture.advance("local.txt");
    let store = &fixture.store;
    let before = aida_core::db::store_write_lock_contended_for(store);
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let mut child = None;
    aida_core::db::with_store_write_lock(store, || {
        let contender = store.clone();
        child = Some(std::thread::spawn(move || {
            done_tx.send(retry_snapshot(&contender, &branch)).unwrap();
        }));
        let started = Instant::now();
        while aida_core::db::store_write_lock_contended_for(store) == before {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "retry never contended"
            );
            assert!(done_rx.try_recv().is_err(), "retry escaped the writer lock");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(git_ops::head_sha(store)?, before_head);
        assert!(!store.join("remote.txt").exists());
        fixture.advance("peer.txt");
        Ok(())
    })
    .unwrap();
    let sha = done_rx
        .recv_timeout(Duration::from_secs(20))
        .unwrap()
        .unwrap();
    child.unwrap().join().unwrap();
    assert_eq!(git_ops::head_sha(store).unwrap(), sha);
    assert_eq!(git_at(store, &["show", "HEAD:remote.txt"]), "remote");
    assert_eq!(git_at(store, &["show", "HEAD:local.txt"]), "local.txt");
    assert_eq!(git_at(store, &["show", "HEAD:peer.txt"]), "peer.txt");
}

#[test]
fn task_1981_capture_preserves_commit_error_without_publication() {
    let fixture = Fixture::new();
    let before = git_ops::head_sha(&fixture.store).unwrap();
    std::fs::write(fixture.store.join("objects/pending.txt"), "pending").unwrap();
    let metadata = fixture.store.join("metadata.yaml");
    let mut content = std::fs::read_to_string(&metadata).unwrap();
    content.push_str("\n# pending metadata\n");
    std::fs::write(&metadata, content).unwrap();
    // Store roots intentionally reject fixture hooks. Fail signing using
    // ordinary repo config instead, with no process environment mutation.
    git_at(&fixture.store, &["config", "commit.gpgSign", "true"]);
    git_at(&fixture.store, &["config", "gpg.format", "openpgp"]);
    git_at(&fixture.store, &["config", "gpg.program", "/bin/false"]);
    let error = capture_snapshot(&fixture.store).unwrap_err();
    assert!(
        format!("{error:#}").contains("git commit failed"),
        "{error:#}"
    );
    assert_eq!(git_ops::head_sha(&fixture.store).unwrap(), before);
    assert!(git_at(&fixture.bare, &["for-each-ref", "refs/heads/"]).is_empty());
    assert!(git_at(&fixture.store, &["diff", "--cached", "--name-only"]).contains("metadata.yaml"));
}

#[test]
fn task_1981_retry_preserves_evidence_refusal_without_rebase_or_push() {
    let fixture = Fixture::new();
    let (branch, captured) = capture_snapshot(&fixture.store).unwrap();
    assert!(!publish_snapshot(&fixture.store, &branch, &captured).unwrap());
    // A remote canonical object with unreadable hypothesis evidence must
    // refuse before rebase. This exercises the real typed evidence boundary.
    fixture.remote_advance_with_file(
        "objects/TASK/000/TASK-1.yaml",
        "hypothesis: [invalid yaml\n",
    );
    let remote_before = fixture.remote_head();
    let local = fixture.advance("local.txt");
    let marker = fixture._dir.path().join("retry-reached");
    let hook = fixture.bare.join("hooks/pre-receive");
    crate::test_exec::write_executable(
        &hook,
        format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display()),
    );
    let error = publish_snapshot(&fixture.store, &branch, &local).unwrap_err();
    assert!(git_ops::is_evidence_refusal(&error), "{error:#}");
    assert_eq!(git_ops::head_sha(&fixture.store).unwrap(), local);
    assert_eq!(fixture.remote_head(), remote_before);
    assert!(!marker.exists(), "retry push ran despite evidence refusal");
    assert!(!fixture.store.join("registry/nodes.toml").exists());
    assert!(!fixture.store.join(".git/rebase-merge").exists());
    assert!(!fixture.store.join(".git/rebase-apply").exists());
}

#[test]
fn task_1981_initial_remote_refusal_does_not_retry() {
    let fixture = Fixture::new();
    let (branch, captured) = capture_snapshot(&fixture.store).unwrap();
    let marker = fixture._dir.path().join("push-attempts");
    let hook = fixture.bare.join("hooks/pre-receive");
    crate::test_exec::write_executable(
        &hook,
        format!(
            "#!/bin/sh\necho reached >> '{}'\necho task-1981-first-refusal >&2\nexit 1\n",
            marker.display(),
        ),
    );
    let error = publish_snapshot(&fixture.store, &branch, &captured).unwrap_err();
    assert!(
        format!("{error:#}").contains("task-1981-first-refusal"),
        "{error:#}"
    );
    assert!(!format!("{error:#}").contains("after rebase"), "{error:#}");
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "reached\n");
    assert_eq!(git_ops::head_sha(&fixture.store).unwrap(), captured);
    assert!(git_at(&fixture.bare, &["for-each-ref", "refs/heads/"]).is_empty());
}
