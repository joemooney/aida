//! BUG-2017: an allocation refusal travels through every best-effort store
//! refresh exactly like an evidence refusal: nothing is committed, written
//! or published after it, while an unreachable store stays best-effort.
// trace:BUG-2017 | ai:claude
use super::*;
use aida_core::git_ops::{head_sha, is_sync_refusal, AllocationRefusal};
use aida_core::models::Requirement;

fn git(cwd: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn write(store: &Path, spec: &str) {
    let mut row = Requirement::new("title".into(), "body".into());
    row.spec_id = Some(spec.into());
    aida_core::object_store::write_object(&store.join("objects"), &row).unwrap();
}

fn commit(store: &Path, msg: &str) {
    git(store, &["add", "-A"]);
    git(store, &["commit", "-q", "-m", msg]);
}

fn identify(store: &Path) {
    crate::test_env::enroll_synthetic_home(store).unwrap();
    for (k, v) in [("user.email", "t@example.invalid"), ("user.name", "T")] {
        git(store, &["config", k, v]);
    }
}

fn is_allocation_refusal(err: &anyhow::Error) -> bool {
    is_sync_refusal(err) && err.chain().any(|e| e.is::<AllocationRefusal>())
}

/// Clone `a` publishes FR-50 while clone `b` issued FR-50 to a different
/// object offline, so every refresh of `b` is an allocation refusal. Before
/// that, `b` published a lease claim while the store was still clean.
struct Refused {
    dir: tempfile::TempDir,
    b: PathBuf,
}

fn refused() -> Refused {
    let dir = tempfile::tempdir().unwrap();
    let hub = dir.path().join("hub.git");
    git(dir.path(), &["init", "-q", "--bare", hub.to_str().unwrap()]);
    git(&hub, &["symbolic-ref", "HEAD", "refs/heads/aida-store"]);
    let a = dir.path().join("a").join(".aida-store");
    std::fs::create_dir_all(&a).unwrap();
    git(&a, &["init", "-q", "-b", "aida-store"]);
    identify(&a);
    std::fs::write(a.join("metadata.yaml"), "name: test\n").unwrap();
    write(&a, "FR-1");
    commit(&a, "seed");
    git(&a, &["remote", "add", "origin", hub.to_str().unwrap()]);
    git(&a, &["push", "-q", "-u", "origin", "aida-store"]);
    let b = dir.path().join("b").join(".aida-store");
    git(
        dir.path(),
        &["clone", "-q", hub.to_str().unwrap(), b.to_str().unwrap()],
    );
    identify(&b);
    let claimed = acquire_claim(&b, "FR-1", dir.path(), "claude", false, false).unwrap();
    assert!(
        matches!(claimed, AcquireOutcome::Acquired),
        "got {claimed:?}"
    );
    git(&a, &["pull", "-q", "--rebase", "origin", "aida-store"]);
    write(&a, "FR-50");
    commit(&a, "a issues FR-50");
    git(&a, &["push", "-q", "origin", "aida-store"]);
    write(&b, "FR-50");
    commit(&b, "b issues FR-50 offline");
    Refused { dir, b }
}

#[test]
fn bug_2017_best_effort_callers_stop_on_allocation_refusal() {
    let Refused { dir, b } = refused();
    let head = head_sha(&b).unwrap();
    let err = acquire_claim(&b, "FR-3", dir.path(), "claude", false, false).unwrap_err();
    assert!(is_allocation_refusal(&err), "{err:#}");
    let err =
        acquire_lock_claim(&b, LockKind::Drain, dir.path(), "drain", 1800, false).unwrap_err();
    assert!(is_allocation_refusal(&err), "{err:#}");
    assert!(
        !lock_claim_path(&b, LockKind::Drain).exists(),
        "no claim may be written after a refusal"
    );
    let lease = claim_path(&b, "FR-1");
    release_claim(&b, "FR-1", dir.path());
    assert!(
        lease.exists(),
        "release_claim must not delete after a refusal"
    );
    assert_eq!(head_sha(&b).unwrap(), head, "no commit after a refusal");

    // Unreachable: the same calls stay best-effort.
    git(&b, &["remote", "set-url", "origin", "/nonexistent/hub.git"]);
    let out = acquire_claim(&b, "FR-3", dir.path(), "claude", false, false).unwrap();
    assert!(matches!(out, AcquireOutcome::Unavailable(_)), "got {out:?}");
}

#[test]
fn bug_2017_block_claim_and_id_allocation_pulls_return_the_refusal() {
    let Refused { dir, b } = refused();
    let err = crate::db_block::handle_block_command(
        &crate::cli::BlockCommand::Claim {
            r#type: "task".into(),
            size: 10,
        },
        &b,
    )
    .unwrap_err();
    assert!(is_allocation_refusal(&err), "{err:#}");
    let err = crate::store_sync::pull_store_before_id_allocation(&b, dir.path()).unwrap_err();
    assert!(is_allocation_refusal(&err), "{err:#}");
    assert!(!format!("{err:#}").contains("rebase --abort"), "{err:#}");
    assert!(
        !b.join(".git/rebase-merge").exists() && !b.join(".git/rebase-apply").exists(),
        "a refusal leaves no rebase behind"
    );
}
