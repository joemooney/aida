//! BUG-2565: a no-op store pull is cheap, and concurrent pulls in one
//! checkout run once.
// trace:BUG-2565 | ai:claude

use super::*;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

fn git(repo: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo(dir: &Path, branch: &str) {
    std::fs::create_dir_all(dir).unwrap();
    git(
        dir,
        &["init", &format!("--initial-branch={branch}"), "--quiet"],
    );
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn commit(dir: &Path, file: &str) {
    std::fs::write(dir.join(file), file).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-m", &format!("chore: {file}"), "--quiet"]);
}

/// A code repo on `main` with a git-canonical store under `.aida-store`.
fn project() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("project");
    init_repo(&root, "main");
    std::fs::write(root.join(".gitignore"), ".aida-store/\n.aida/\n").unwrap();
    commit(&root, "README.md");
    let store = root.join(".aida-store");
    init_repo(&store, "aida-store");
    // The real store's runtime ignores, so cache/dispenser files stay out.
    std::fs::write(
        store.join(".gitignore"),
        "*.sqlite3\n.aida/*.lock\n.aida/cache.db*\n.aida/dispenser.toml\n.aida/node.toml\n",
    )
    .unwrap();
    std::fs::create_dir_all(store.join("objects")).unwrap();
    commit(&store, "objects/.keep");
    (tmp, root, store)
}

/// Acceptance (3): with 1000+ gate-ledger spec dirs in the checkout, the
/// second pull with nothing new upstream runs no code-derived reconcile and
/// finishes well inside the no-op budget; a new commit runs it again.
#[test]
fn noop_pull_skips_reconcile_with_large_gate_ledger() {
    let (_tmp, root, store) = project();
    let ledger = root.join(".aida").join("gate-ledger");
    for n in 0..1200 {
        let dir = ledger.join(format!("STORY-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("events.jsonl"), "").unwrap();
    }
    let runs = reconcile_runs();

    crate::store_push_pull::run_deferred_code_reconcile(&root, &store, None, true, true);
    assert_eq!(reconcile_runs(), runs + 1, "the first pull reconciles");
    assert!(reconcile_is_current(&root, &store));

    let start = Instant::now();
    crate::store_push_pull::run_deferred_code_reconcile(&root, &store, None, true, true);
    assert_eq!(
        reconcile_runs(),
        runs + 1,
        "a pull with neither HEAD moved must not reconcile again"
    );
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "no-op reconcile took {:?}",
        start.elapsed()
    );

    // Upstream code moved: the watermark no longer applies.
    commit(&root, "next.txt");
    assert!(!reconcile_is_current(&root, &store));
    crate::store_push_pull::run_deferred_code_reconcile(&root, &store, None, true, true);
    assert_eq!(reconcile_runs(), runs + 2);

    // So does a store move, and a dirty store never matches.
    commit(&store, "objects/a.yaml");
    assert!(!reconcile_is_current(&root, &store));
    record_reconcile(&root, &store);
    assert!(reconcile_is_current(&root, &store));
    std::fs::write(store.join("objects/b.yaml"), "x").unwrap();
    assert!(!reconcile_is_current(&root, &store));
}

/// A scan base (new commits landed) always reconciles, watermark or not.
#[test]
fn scan_base_bypasses_watermark() {
    let (_tmp, root, store) = project();
    record_reconcile(&root, &store);
    let runs = reconcile_runs();
    let base = git(&root, &["rev-parse", "HEAD"]);
    crate::store_push_pull::run_deferred_code_reconcile(&root, &store, Some(&base), true, true);
    assert_eq!(reconcile_runs(), runs + 1);
}

#[test]
fn watermark_parses_and_rejects_garbage() {
    assert_eq!(
        parse_watermark("abc def 42"),
        Some(("abc".into(), "def".into(), 42))
    );
    assert_eq!(parse_watermark("abc def"), None);
    assert_eq!(parse_watermark("abc def soon"), None);
}

/// Hold the pull lock in one thread and start `waiters` more; returns how
/// many ran the pull and how many reused it.
fn race(store: &Path, root: &Path, waiters: usize, first_ok: bool) -> (usize, usize) {
    let ran = Arc::new(AtomicUsize::new(0));
    let reused = Arc::new(AtomicUsize::new(0));
    let holding = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let first = {
        let (store, root) = (store.to_path_buf(), root.to_path_buf());
        let (ran, holding, release) = (ran.clone(), holding.clone(), release.clone());
        std::thread::spawn(move || {
            coalesce(
                &store,
                &root,
                "test",
                || true,
                || {
                    ran.fetch_add(1, Ordering::SeqCst);
                    holding.wait();
                    release.wait();
                    if first_ok {
                        Ok(())
                    } else {
                        Err(anyhow::anyhow!("network down"))
                    }
                },
            )
        })
    };
    holding.wait();
    let others: Vec<_> = (0..waiters)
        .map(|_| {
            let (store, root) = (store.to_path_buf(), root.to_path_buf());
            let (ran, reused) = (ran.clone(), reused.clone());
            std::thread::spawn(move || {
                let turn = coalesce(
                    &store,
                    &root,
                    "test",
                    || true,
                    || {
                        ran.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    },
                )
                .unwrap();
                if turn == Turn::Reused {
                    reused.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();
    // Let every waiter read the state and block on the lock.
    std::thread::sleep(Duration::from_millis(600));
    release.wait();
    let _ = first.join().unwrap();
    for t in others {
        t.join().unwrap();
    }
    (ran.load(Ordering::SeqCst), reused.load(Ordering::SeqCst))
}

/// Acceptance (2): N concurrent pulls run one pull.
#[test]
fn concurrent_pulls_coalesce_into_one() {
    let (_tmp, root, store) = project();
    let (ran, reused) = race(&store, &root, 4, true);
    assert_eq!((ran, reused), (1, 4));
}

/// A failed in-flight pull is never reused: a waiter pulls itself.
#[test]
fn failed_pull_is_not_reused() {
    let (_tmp, root, store) = project();
    let (ran, reused) = race(&store, &root, 1, false);
    assert_eq!((ran, reused), (2, 0));
}

/// An uncontended pull always runs, even right after another finished.
#[test]
fn sequential_pulls_both_run() {
    let (_tmp, root, store) = project();
    for _ in 0..2 {
        let turn = coalesce(&store, &root, "test", || true, || Ok(())).unwrap();
        assert_eq!(turn, Turn::Ran);
    }
}

/// Acceptance (1): origin holding nothing new is detected without a fetch.
#[test]
fn remote_head_containment() {
    let (tmp, _root, store) = project();
    let bare = tmp.path().join("store.git");
    git(
        tmp.path(),
        &[
            "clone",
            "--bare",
            "--quiet",
            store.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    git(&store, &["remote", "add", "origin", bare.to_str().unwrap()]);
    assert!(store_already_has_remote_head(&store, "aida-store"));
    commit(&store, "objects/local.yaml");
    assert!(
        store_already_has_remote_head(&store, "aida-store"),
        "unpushed local commits still contain origin"
    );
    let other = tmp.path().join("other");
    git(
        tmp.path(),
        &[
            "clone",
            "--quiet",
            "--branch",
            "aida-store",
            bare.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    git(&other, &["config", "user.email", "test@example.com"]);
    git(&other, &["config", "user.name", "Test"]);
    commit(&other, "objects/remote.yaml");
    git(&other, &["push", "--quiet", "origin", "aida-store"]);
    assert!(!store_already_has_remote_head(&store, "aida-store"));
}
