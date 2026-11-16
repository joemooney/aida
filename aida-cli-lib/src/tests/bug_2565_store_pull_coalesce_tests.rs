//! BUG-2565: a no-op store pull is cheap, and concurrent pulls in one
//! checkout run once.
// trace:BUG-2565 | ai:claude

use super::*;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    std::fs::create_dir_all(dir.join(file).parent().unwrap()).unwrap();
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
    let started = Arc::new(AtomicUsize::new(0));
    let others: Vec<_> = (0..waiters)
        .map(|_| {
            let (store, root) = (store.to_path_buf(), root.to_path_buf());
            let (ran, reused, started) = (ran.clone(), reused.clone(), started.clone());
            std::thread::spawn(move || {
                started.fetch_add(1, Ordering::SeqCst);
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
    // Let every waiter start (a loaded host can delay a spawn past any fixed
    // sleep) and then see the pull in flight before it ends.
    while started.load(Ordering::SeqCst) < waiters {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(1500));
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

// trace:BUG-2565 | ai:codex
fn add_origin(tmp: &Path, store: &Path) -> std::path::PathBuf {
    let bare = tmp.join("origin.git");
    git(
        tmp,
        &[
            "clone",
            "--bare",
            "--quiet",
            store.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    git(store, &["remote", "add", "origin", bare.to_str().unwrap()]);
    bare
}

#[test]
fn db_sync_noop_remote_skips_strict_scans_with_large_ledger() {
    let (tmp, root, store) = project();
    // A strict conflict or collision scan would refuse this object. A clean,
    // unchanged remote needs neither scan.
    commit(&store, "objects/bug/00/invalid.yaml");
    add_origin(tmp.path(), &store);
    for n in 0..1200 {
        let dir = root.join(".aida/gate-ledger").join(format!("STORY-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("events.jsonl"), "").unwrap();
    }
    let start = Instant::now();
    let result = crate::git_backend_cmd::db_sync_local_window(&store, true, false, None).unwrap();
    assert!(result.conflicts.unwrap().is_empty());
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
    assert!(
        !store.join(".git/FETCH_HEAD").exists(),
        "fast path must not fetch"
    );
}

#[test]
fn db_sync_checks_pending_and_locally_committed_objects() {
    for pending in [false, true] {
        let (tmp, _root, store) = project();
        add_origin(tmp.path(), &store);
        if pending {
            std::fs::create_dir_all(store.join("objects/bug/00")).unwrap();
            std::fs::write(store.join("objects/bug/00/invalid.yaml"), "invalid").unwrap();
        } else {
            commit(&store, "objects/bug/00/invalid.yaml");
        }
        assert!(
            crate::git_backend_cmd::db_sync_local_window(&store, true, false, None).is_err(),
            "local canonical changes must be scanned (pending={pending})"
        );
    }
}

#[test]
fn waiter_refuses_pull_when_remote_moves_in_flight() {
    moving_remote_waiter(false);
}

#[test]
fn waiter_refuses_pull_when_code_remote_moves_in_flight() {
    moving_remote_waiter(true);
}

// Exercise both production reuse predicates while an older successful pull
// still owns the lock. The store stays current when only the code remote moves.
// trace:BUG-2565 | ai:codex
fn moving_remote_waiter(code_remote: bool) {
    let (tmp, root, store) = project();
    add_origin(tmp.path(), &store);
    let repo = if code_remote { &root } else { &store };
    let branch = if code_remote { "main" } else { "aida-store" };
    let bare = if code_remote {
        let dir = tmp.path().join("code-origin");
        std::fs::create_dir_all(&dir).unwrap();
        add_origin(&dir, &root)
    } else {
        tmp.path().join("origin.git")
    };
    let fetched = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let first = {
        let (store, root, repo) = (store.clone(), root.clone(), repo.clone());
        let (fetched, release) = (fetched.clone(), release.clone());
        std::thread::spawn(move || {
            coalesce(
                &store,
                &root,
                "moving",
                || true,
                || {
                    git(&repo, &["fetch", "origin"]);
                    fetched.wait();
                    release.wait();
                    Ok(())
                },
            )
            .unwrap()
        })
    };
    fetched.wait();
    let other = tmp.path().join("writer");
    git(
        tmp.path(),
        &[
            "clone",
            "--quiet",
            bare.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    git(&other, &["config", "user.email", "test@example.com"]);
    git(&other, &["config", "user.name", "Test"]);
    commit(&other, "remote.txt");
    git(&other, &["push", "--quiet", "origin", branch]);
    let checked_reuse = Arc::new(AtomicBool::new(false));
    let waiter = {
        let (store, root, repo) = (store.clone(), root.clone(), repo.clone());
        let checked_reuse = checked_reuse.clone();
        std::thread::spawn(move || {
            coalesce(
                &store,
                &root,
                "moving",
                || {
                    checked_reuse.store(true, Ordering::SeqCst);
                    if code_remote {
                        pull_reusable(&store, &root, false, false)
                    } else {
                        store_pull_reusable(&store)
                    }
                },
                || {
                    git(&repo, &["pull", "--ff-only", "origin", branch]);
                    Ok(())
                },
            )
            .unwrap()
        })
    };
    std::thread::sleep(Duration::from_millis(600));
    release.wait();
    assert_eq!(first.join().unwrap(), Turn::Ran);
    assert_eq!(waiter.join().unwrap(), Turn::Ran);
    assert!(
        checked_reuse.load(Ordering::SeqCst),
        "the waiter must actually exercise the reuse veto"
    );
    assert_eq!(
        git(repo, &["rev-parse", "HEAD"]),
        git(&other, &["rev-parse", "HEAD"])
    );
    assert!(store_pull_reusable(&store));
    if code_remote {
        assert!(pull_reusable(&store, &root, false, false));
    }
}

#[test]
fn noop_remote_still_refuses_local_spec_id_collisions() {
    for (pending, store_leg) in [(false, false), (true, false), (false, true), (true, true)] {
        let (tmp, _root, store) = project();
        add_origin(tmp.path(), &store);
        std::fs::create_dir_all(store.join("objects/bug/00")).unwrap();
        for title in ["first", "second"] {
            let mut req = aida_core::Requirement::new(title.to_string(), String::new());
            req.spec_id = Some("BUG-123".to_string());
            std::fs::write(
                store
                    .join("objects/bug/00")
                    .join(format!("{}.yaml", req.id)),
                serde_yaml::to_string(&req).unwrap(),
            )
            .unwrap();
        }
        if !pending {
            git(&store, &["add", "-A"]);
            git(&store, &["commit", "-qm", "local duplicates"]);
        }
        let error = if store_leg {
            crate::store_push_pull::pull_store_leg_locked(&store, true, true, 0)
                .err()
                .expect("store pull collision must refuse")
        } else {
            crate::git_backend_cmd::db_sync_local_window(&store, true, false, None)
                .err()
                .expect("db sync collision must refuse")
                .to_string()
        };
        assert!(error.to_string().contains("BUG-123"), "{error:#}");
    }
}

#[test]
fn operator_controls_disable_coalescing_and_watermark() {
    let (_tmp, root, store) = project();
    let environment = Environment::from_pairs(
        [
            ("AIDA_PULL_COALESCE", "0"),
            ("AIDA_RECONCILE_WATERMARK_TTL_SECS", "0"),
            ("AIDA_PULL_COALESCE_WAIT_SECS", "1"),
        ]
        .map(|(key, value)| (key.into(), value.into())),
    );
    let (lock_path, _) = coalesce_paths(&store, &root, "disabled");
    std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    let lock = std::fs::File::create(lock_path).unwrap();
    lock.lock_exclusive().unwrap();
    assert_eq!(
        coalesce_with_environment(
            &store,
            &root,
            "disabled",
            &environment,
            || panic!("reuse disabled"),
            || Ok(())
        )
        .unwrap(),
        Turn::Ran
    );
    record_reconcile(&root, &store);
    assert!(!reconcile_is_current_with_environment(
        &root,
        &store,
        &environment
    ));
    assert_eq!(
        duration_setting(&environment, "AIDA_PULL_COALESCE_WAIT_SECS", WAIT_BUDGET),
        Duration::from_secs(1)
    );
}

#[test]
fn code_remote_freshness_independently_vetoes_reuse() {
    let (tmp, root, store) = project();
    add_origin(tmp.path(), &store);
    let bare = tmp.path().join("code.git");
    git(
        tmp.path(),
        &[
            "clone",
            "--bare",
            "--quiet",
            root.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    git(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
    assert!(pull_reusable(&store, &root, false, false));
    let writer = tmp.path().join("code-writer");
    git(
        tmp.path(),
        &[
            "clone",
            "--quiet",
            bare.to_str().unwrap(),
            writer.to_str().unwrap(),
        ],
    );
    git(&writer, &["config", "user.email", "test@example.com"]);
    git(&writer, &["config", "user.name", "Test"]);
    commit(&writer, "new-code.txt");
    git(&writer, &["push", "--quiet", "origin", "main"]);
    assert!(!pull_reusable(&store, &root, false, false));
    assert!(pull_reusable(&store, &root, false, true));
}

// On a branch other than the default, the code leg pulls that branch: a
// waiter must not reuse a pull because main is contained while the checked
// out branch moved on origin.
// trace:BUG-2565 | ai:claude
#[test]
fn code_freshness_is_asked_of_the_checked_out_branch() {
    let (tmp, root, store) = project();
    add_origin(tmp.path(), &store);
    let bare = tmp.path().join("code.git");
    git(
        tmp.path(),
        &[
            "clone",
            "--bare",
            "--quiet",
            root.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    git(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&root, &["checkout", "--quiet", "-b", "integration"]);
    git(&root, &["push", "--quiet", "origin", "integration"]);
    assert!(pull_reusable(&store, &root, false, false));
    let writer = tmp.path().join("code-writer");
    git(
        tmp.path(),
        &[
            "clone",
            "--quiet",
            "--branch",
            "integration",
            bare.to_str().unwrap(),
            writer.to_str().unwrap(),
        ],
    );
    git(&writer, &["config", "user.email", "test@example.com"]);
    git(&writer, &["config", "user.name", "Test"]);
    commit(&writer, "integration-only.txt");
    git(&writer, &["push", "--quiet", "origin", "integration"]);
    // main on origin is still contained in the local HEAD; integration is not.
    assert!(
        aida_core::git_ops::is_ancestor(&root, &git(&bare, &["rev-parse", "main"]), "HEAD")
            .unwrap()
    );
    assert!(!pull_reusable(&store, &root, false, false));
    assert!(pull_reusable(&store, &root, false, true));
}

#[test]
fn remote_head_query_distinguishes_absent_branch_and_failure() {
    let (tmp, _root, store) = project();
    add_origin(tmp.path(), &store);
    assert_eq!(
        aida_core::git_ops::remote_branch_head_sha_checked(&store, "origin", "absent").unwrap(),
        None
    );
    assert!(aida_core::git_ops::remote_branch_head_sha_checked(
        &store,
        "missing-remote",
        "aida-store"
    )
    .is_err());
    assert_eq!(
        aida_core::git_ops::remote_branch_head_sha_checked(&store, "origin", "aida-store").unwrap(),
        Some(git(&store, &["rev-parse", "HEAD"]))
    );
}

// ---- the window: nothing of the coalescing is locked or open in it ---------

/// The `.lock` files under `store` this process has open. The descriptor
/// table is readable only on Linux; elsewhere the lock probe in the test is
/// the evidence.
#[cfg(not(target_os = "linux"))]
fn open_lock_files(_store: &Path) -> Vec<String> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn open_lock_files(store: &Path) -> Vec<String> {
    let store = std::fs::canonicalize(store).unwrap();
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .flatten()
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .filter(|target| {
            target.starts_with(&store) && target.extension().is_some_and(|ext| ext == "lock")
        })
        .map(|target| target.display().to_string())
        .collect()
}

/// Whether `lock_path` can be locked within 2 s. Another test thread's child,
/// between fork and exec, can briefly hold a copy of a descriptor this
/// module has already closed; a lock this module kept would never come free
/// while the caller waits inside the pull.
fn lock_comes_free(lock_path: &Path) -> bool {
    let probe = std::fs::File::open(lock_path).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if probe.try_lock_exclusive().is_ok() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// While the pull runs, the lock file is neither locked nor open in this
// process, and the marker names this process. Afterwards the marker is gone.
// trace:BUG-2565 | ai:claude
#[test]
fn the_pull_runs_with_no_coalesce_lock_held_or_open() {
    let (_tmp, root, store) = project();
    let (lock_path, state_path) = coalesce_paths(&store, &root, "window");
    let run_path = runner_path(&lock_path);
    for ok in [true, false] {
        let result = coalesce(
            &store,
            &root,
            "window",
            || true,
            || {
                assert_eq!(open_lock_files(&store), Vec::<String>::new());
                assert!(
                    lock_comes_free(&lock_path),
                    "the lock is free while the pull runs"
                );
                let runner = read_runner(&run_path).expect("a marker for the pull in flight");
                assert_eq!(runner.pid, std::process::id());
                if ok {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("network down"))
                }
            },
        );
        assert_eq!(result.is_ok(), ok);
        assert!(!run_path.exists(), "the marker goes when the pull ends");
        assert_eq!(open_lock_files(&store), Vec::<String>::new());
        // The outcome is recorded either way; only a success is reusable.
        assert_eq!(read_generation(&state_path).is_some(), ok);
    }
}

// A hook of the pull in flight that starts a pull must not wait for the
// pull that is running it. The marker here names this test's parent.
// trace:BUG-2565 | ai:claude
#[test]
fn a_pull_started_by_the_pull_in_flight_runs_at_once() {
    let (_tmp, root, store) = project();
    let (lock_path, state_path) = coalesce_paths(&store, &root, "nested");
    let run_path = runner_path(&lock_path);
    std::fs::create_dir_all(run_path.parent().unwrap()).unwrap();
    let parent = std::os::unix::process::parent_id();
    let id = aida_core::proc_identity::ProcIdentity::capture(parent)
        .expect("the parent's process identity");
    let holder = Runner {
        pid: parent,
        start: id.legacy().start,
        ns: id.pidns_inode,
        token: "the-pull-in-flight".into(),
    };
    assert!(write_runner(&run_path, &holder));
    let ran = AtomicUsize::new(0);
    let started = std::time::Instant::now();
    let turn = coalesce(
        &store,
        &root,
        "nested",
        || panic!("there is nothing to reuse: the pull in flight has not ended"),
        || {
            ran.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(turn, Turn::Ran);
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "it did not wait: {:?}",
        started.elapsed()
    );
    // The pull in flight keeps its marker, and its outcome is its own to write.
    assert_eq!(read_runner(&run_path), Some(holder));
    assert_eq!(read_generation(&state_path), None);
}

// A marker left by a pull that crashed names a process that is gone: the
// next pull runs at once and replaces it.
// trace:BUG-2565 | ai:claude
#[test]
fn a_marker_of_a_dead_pull_is_not_waited_for() {
    let (_tmp, root, store) = project();
    let (lock_path, _) = coalesce_paths(&store, &root, "dead");
    let run_path = runner_path(&lock_path);
    std::fs::create_dir_all(run_path.parent().unwrap()).unwrap();
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    let ns = aida_core::proc_identity::ProcIdentity::capture(std::process::id())
        .expect("this process's identity")
        .pidns_inode;
    for (start, ns) in [
        ("linux:00000000-0000-0000-0000-000000000000:1", ns),
        ("", 0),
    ] {
        assert!(write_runner(
            &run_path,
            &Runner {
                pid,
                start: start.into(),
                ns,
                token: "crashed".into(),
            }
        ));
        let started = std::time::Instant::now();
        let turn = coalesce(&store, &root, "dead", || true, || Ok(())).unwrap();
        assert_eq!(turn, Turn::Ran);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!run_path.exists());
    }
}

// A marker naming this PID but left by an earlier process that had it (PID
// reuse, reboot, another namespace) names no call of this process: the
// pull runs at once and its own marker replaces it.
// trace:BUG-2565 | ai:claude
#[test]
fn a_stale_marker_with_this_pid_is_not_waited_for() {
    let (_tmp, root, store) = project();
    let (lock_path, _) = coalesce_paths(&store, &root, "reused");
    let run_path = runner_path(&lock_path);
    std::fs::create_dir_all(run_path.parent().unwrap()).unwrap();
    let me = std::process::id();
    let here =
        aida_core::proc_identity::ProcIdentity::capture(me).expect("this process's identity");
    let stale = [
        // Another boot.
        (
            format!(
                "linux:00000000-0000-0000-0000-000000000000:{}",
                here.starttime
            ),
            here.pidns_inode,
        ),
        // The same boot, another start time.
        (
            format!("linux:{}:{}", here.boot_id, here.starttime + 1),
            here.pidns_inode,
        ),
        // Another pid namespace.
        (here.legacy().start, here.pidns_inode + 1),
        // The very identity of this process, from a call that is not running.
        (here.legacy().start, here.pidns_inode),
    ];
    for (start, ns) in stale {
        assert!(write_runner(
            &run_path,
            &Runner {
                pid: me,
                start: start.clone(),
                ns,
                token: "an-earlier-process".into(),
            }
        ));
        let seen = std::sync::Mutex::new(None);
        let started = std::time::Instant::now();
        let turn = coalesce_with_environment(
            &store,
            &root,
            "reused",
            &wait_secs("30"),
            || panic!("nothing to reuse"),
            || {
                *seen.lock().unwrap() = read_runner(&run_path);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(turn, Turn::Ran, "{start} ns {ns}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{start} ns {ns}: waited {:?}",
            started.elapsed()
        );
        let replaced = seen.into_inner().unwrap().expect("a marker in flight");
        assert_ne!(replaced.token, "an-earlier-process", "{start} ns {ns}");
        assert!(!run_path.exists(), "{start} ns {ns}");
    }
}

fn wait_secs(secs: &str) -> Environment {
    Environment::from_pairs([("AIDA_PULL_COALESCE_WAIT_SECS".into(), secs.into())])
}

// A caller that holds the decision lock and stalls (a probe that hangs, a
// stopped process) costs a waiter its own wait budget and no more: zero
// means one try, then the caller pulls without it.
// trace:BUG-2565 | ai:claude
#[test]
fn a_stalled_decision_lock_holder_costs_at_most_the_wait_budget() {
    let (_tmp, root, store) = project();
    let (lock_path, _) = coalesce_paths(&store, &root, "stalled");
    std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    let stalled = std::fs::File::create(&lock_path).unwrap();
    stalled.lock_exclusive().unwrap();
    for (secs, within) in [("0", Duration::from_secs(1)), ("1", Duration::from_secs(3))] {
        let ran = AtomicUsize::new(0);
        let started = std::time::Instant::now();
        let turn = coalesce_with_environment(
            &store,
            &root,
            "stalled",
            &wait_secs(secs),
            || panic!("nothing finished: there is nothing to reuse"),
            || {
                ran.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(turn, Turn::Ran, "WAIT_SECS={secs}");
        assert_eq!(ran.load(Ordering::SeqCst), 1, "WAIT_SECS={secs}");
        assert!(
            started.elapsed() < within,
            "WAIT_SECS={secs} waited {:?}",
            started.elapsed()
        );
    }
    drop(stalled);
}

// The freshness probe of a waiter talks to the remote, so it runs with the
// decision lock free; the result is reused only after a revalidation.
// trace:BUG-2565 | ai:claude
#[test]
fn the_freshness_probe_runs_with_the_decision_lock_free() {
    let (_tmp, root, store) = project();
    let (lock_path, state_path) = coalesce_paths(&store, &root, "probe");
    let run_path = runner_path(&lock_path);
    std::fs::create_dir_all(run_path.parent().unwrap()).unwrap();
    // A live process that is not an ancestor stands in for the pull in flight.
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let id = aida_core::proc_identity::ProcIdentity::capture(child.id()).unwrap();
    assert!(write_runner(
        &run_path,
        &Runner {
            pid: child.id(),
            start: id.legacy().start,
            ns: id.pidns_inode,
            token: "in-flight".into(),
        }
    ));
    let finisher = {
        let (state_path, run_path) = (state_path.clone(), run_path.clone());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            write_state(&state_path, 1, true);
            std::fs::remove_file(&run_path).unwrap();
        })
    };
    let probed = AtomicBool::new(false);
    let turn = coalesce_with_environment(
        &store,
        &root,
        "probe",
        &wait_secs("30"),
        || {
            assert!(
                lock_comes_free(&lock_path),
                "the probe runs under the decision lock"
            );
            probed.store(true, Ordering::SeqCst);
            true
        },
        || panic!("the finished pull is reusable"),
    )
    .unwrap();
    finisher.join().unwrap();
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(turn, Turn::Reused);
    assert!(probed.load(Ordering::SeqCst));
}

/// Pending host writes and empty bookkeeping commits must not defeat reuse.
/// The run counter guards entry to both forge probes and the history walk.
// trace:BUG-2593 | ai:codex
#[test]
fn unchanged_dirty_store_skips_reconcile() {
    let (_tmp, root, store) = project();
    std::fs::write(store.join("pending-note"), "host-deferred write").unwrap();
    let runs = reconcile_runs();
    crate::store_push_pull::run_deferred_code_reconcile(&root, &store, None, true, true);
    assert!(reconcile_is_current(&root, &store));
    crate::store_push_pull::run_deferred_code_reconcile(&root, &store, None, true, true);
    assert_eq!(reconcile_runs(), runs + 1);
    std::fs::write(store.join("pending-note"), "changed input").unwrap();
    assert!(!reconcile_is_current(&root, &store));
    record_reconcile(&root, &store);
    git(&store, &["add", "-A"]);
    git(&store, &["commit", "-qm", "pending writes"]);
    assert!(reconcile_is_current(&root, &store));
    git(&store, &["commit", "--allow-empty", "-qm", "bookkeeping"]);
    assert!(reconcile_is_current(&root, &store));
    std::fs::write(store.join("objects/.keep"), "tracked pending write").unwrap();
    assert!(!reconcile_is_current(&root, &store));
    record_reconcile(&root, &store);
    assert!(reconcile_is_current(&root, &store));
}

/// Committing the same effective contents must preserve the dirty watermark.
/// No counter increment means neither forge probes nor history walks are entered.
// trace:BUG-2593 | ai:codex
#[test]
fn dirty_record_then_commit_same_contents_skips_reconcile() {
    for file in ["objects/.keep", "pending-note"] {
        let (_tmp, root, store) = project();
        std::fs::write(store.join(file), "pending contents").unwrap();
        let runs = reconcile_runs();
        crate::store_push_pull::run_deferred_code_reconcile(&root, &store, None, true, true);
        assert_eq!(reconcile_runs(), runs + 1, "{file}: initial reconcile");
        assert!(reconcile_is_current(&root, &store));
        let index_before = git(&store, &["diff", "--cached", "--name-only"]);
        assert!(
            index_before.is_empty(),
            "fingerprinting must not stage writes"
        );
        git(&store, &["add", "-A"]);
        git(
            &store,
            &["commit", "-qm", "commit identical pending contents"],
        );
        // Deliberately do not record again after committing.
        assert!(
            reconcile_is_current(&root, &store),
            "{file}: identical snapshot"
        );
        crate::store_push_pull::run_deferred_code_reconcile(&root, &store, None, true, true);
        assert_eq!(
            reconcile_runs(),
            runs + 1,
            "{file}: no reconcile, forge or history entry"
        );
        std::fs::write(store.join(file), "different contents").unwrap();
        assert!(
            !reconcile_is_current(&root, &store),
            "{file}: changed contents"
        );
    }
}

/// The primary's tracked self-store link made v3 keys unavailable forever.
/// Entry counter is before BOTH waiting-verdict reconciliation and git log:
/// no second entry proves neither expensive operation runs on an idle pull.
/// This fixture intentionally commits pending orphan writes before pull #2.
// trace:BUG-2593 | ai:codex
#[test]
fn bug_2593_idle_primary_layout_skips_waiting_and_wide_scan() {
    let (_tmp, root, store) = project();
    std::os::unix::fs::symlink(
        "/unavailable/primary/.aida-store",
        store.join(".aida-store"),
    )
    .unwrap();
    git(&store, &["add", ".aida-store"]);
    git(&store, &["commit", "-qm", "primary store layout"]);
    std::fs::write(store.join("pending-note"), "pending orphan change").unwrap();
    let runs = reconcile_runs();
    crate::store_push_pull::run_deferred_code_reconcile(&root, &store, None, true, true);
    assert_eq!(
        reconcile_runs(),
        runs + 1,
        "initial scan enters waiting + history"
    );
    git(&store, &["add", "-A"]);
    git(
        &store,
        &["commit", "-qm", "commit pending orphan changes before pull"],
    );
    // No watermark reset after committing the exact same effective contents.
    crate::store_push_pull::run_deferred_code_reconcile(&root, &store, None, true, true);
    assert_eq!(
        reconcile_runs(),
        runs + 1,
        "idle pull must enter neither waiting reconciliation nor wide scan"
    );
    assert!(
        reconcile_is_current(&root, &store),
        "successful scan persisted a usable key"
    );
    let verdicts = root.join(".aida/review-verdicts");
    std::fs::create_dir_all(&verdicts).unwrap();
    std::fs::write(verdicts.join("PR-7.json"), "new waiting verdict").unwrap();
    assert!(
        !reconcile_is_current(&root, &store),
        "new local verdict invalidates reuse"
    );
    record_reconcile(&root, &store);
    std::fs::write(verdicts.join("PR-7.json"), "replacement waiting verdict").unwrap();
    assert!(
        !reconcile_is_current(&root, &store),
        "edited local verdict invalidates reuse"
    );
    record_reconcile(&root, &store);
    std::fs::remove_file(store.join(".aida-store")).unwrap();
    std::os::unix::fs::symlink("/different/target", store.join(".aida-store")).unwrap();
    assert!(
        !reconcile_is_current(&root, &store),
        "link value participates in key"
    );

    // Narrow waiting evidence survives unrelated pending writes, even when
    // those writes invalidate the wide completion-scan watermark.
    let (_other_tmp, other_root, other_store) = project();
    let wide_runs = reconcile_runs();
    let waiting_runs = waiting_reconcile_runs();
    crate::store_push_pull::run_deferred_code_reconcile(
        &other_root,
        &other_store,
        None,
        true,
        true,
    );
    assert_eq!(waiting_reconcile_runs(), waiting_runs + 1);
    std::fs::write(other_store.join("pending-note"), "unrelated pending write").unwrap();
    crate::store_push_pull::run_deferred_code_reconcile(
        &other_root,
        &other_store,
        None,
        true,
        true,
    );
    assert_eq!(
        reconcile_runs(),
        wide_runs + 2,
        "changed effective inputs require a wide scan"
    );
    assert_eq!(
        waiting_reconcile_runs(),
        waiting_runs + 1,
        "same code/store HEAD and verdict inventory skip waiting work independently"
    );
    let verdicts = other_root.join(".aida/review-verdicts");
    std::fs::create_dir_all(&verdicts).unwrap();
    std::fs::write(verdicts.join("PR-7.json"), "new waiting record").unwrap();
    crate::store_push_pull::run_deferred_code_reconcile(
        &other_root,
        &other_store,
        None,
        true,
        true,
    );
    assert_eq!(
        waiting_reconcile_runs(),
        waiting_runs + 2,
        "new verdict evidence requires waiting reconciliation without a store commit"
    );
}
