//! BUG-2162: every best-effort store refresh that a hypothesis evidence
//! refusal reaches stops before it commits or publishes, while an
//! unreachable store keeps the old best-effort fallback.
// trace:BUG-2162 | ai:claude
use super::*;
use aida_core::git_ops::{head_sha, is_evidence_refusal};
use aida_core::models::{
    HypothesisLock, Relationship, RelationshipType, Requirement, RequirementStatus, RequirementType,
};
use sha2::Digest;

fn git(cwd: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn write(store: &Path, row: &Requirement) {
    aida_core::object_store::write_object(&store.join("objects"), row).unwrap();
}

fn commit(store: &Path, msg: &str) {
    git(store, &["add", "-A"]);
    git(store, &["commit", "-q", "-m", msg]);
}

fn identify(store: &Path) {
    // trace:BUG-2017 | ai:codex — explicit positive evidence-fixture clone.
    crate::test_env::enroll_synthetic_home(store).unwrap();
    for (k, v) in [("user.email", "t@example.invalid"), ("user.name", "T")] {
        git(store, &["config", k, v]);
    }
}

/// Clone `b` holds an offline rewrite of a prediction that clone `a` has
/// meanwhile locked by completing its result on the hub, so every refresh of
/// `b` is an evidence refusal. Before that, `b` published a lease claim and
/// a solo lock claim of its own while the store was still clean.
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
    let mut h = Requirement::new("prediction".into(), "before".into());
    h.spec_id = Some("SPIKE-1".into());
    h.req_type = RequirementType::Spike;
    h.tags.insert("hypothesis".into());
    let mut r = Requirement::new("experiment".into(), "run".into());
    r.spec_id = Some("TASK-2".into());
    h.relationships.push(Relationship {
        rel_type: RelationshipType::References,
        target_id: r.id,
        created_at: None,
        created_by: None,
    });
    std::fs::write(a.join("metadata.yaml"), "name: test\n").unwrap();
    write(&a, &h);
    write(&a, &r);
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
    let locked = acquire_lock_claim(&b, LockKind::Solo, dir.path(), "solo", 1800, false).unwrap();
    assert!(
        matches!(locked, LockAcquireOutcome::Acquired),
        "got {locked:?}"
    );
    // One clone completes the result and locks the prediction; the other
    // rewrites the prediction offline.
    git(&a, &["pull", "-q", "--rebase", "origin", "aida-store"]);
    r.status = RequirementStatus::Completed;
    write(&a, &r);
    let mut stamped = h.clone();
    stamped.hypothesis_lock = Some(HypothesisLock {
        locked_at: Utc::now(),
        description_sha256: format!("{:x}", sha2::Sha256::digest(h.description.as_bytes())),
    });
    write(&a, &stamped);
    commit(&a, "result");
    git(&a, &["push", "-q", "origin", "aida-store"]);
    let mut edited = h.clone();
    edited.description = "moved goalposts".into();
    write(&b, &edited);
    commit(&b, "offline edit");
    Refused { dir, b }
}

#[test]
fn bug_2162_best_effort_callers_stop_on_evidence_refusal() {
    let Refused { dir, b } = refused();
    let err = acquire_claim(&b, "FR-3", dir.path(), "claude", false, false).unwrap_err();
    assert!(is_evidence_refusal(&err), "{err:#}");
    let err =
        acquire_lock_claim(&b, LockKind::Drain, dir.path(), "drain", 1800, false).unwrap_err();
    assert!(is_evidence_refusal(&err), "{err:#}");
    assert!(release_stale_foreign_claim(&b, "FR-1", "x", "y").is_err());
    assert!(
        !lock_claim_path(&b, LockKind::Drain).exists(),
        "no claim may be written after a refusal"
    );

    // Unreachable: the same calls stay best-effort.
    git(&b, &["remote", "set-url", "origin", "/nonexistent/hub.git"]);
    let out = acquire_claim(&b, "FR-3", dir.path(), "claude", false, false).unwrap();
    assert!(matches!(out, AcquireOutcome::Unavailable(_)), "got {out:?}");
}

/// The void cleanup paths (`release_claim`, `heartbeat_lock_claim`,
/// `release_lock_claim`) stop at the refusal: the claim stays on disk
/// untouched and no commit lands.
#[test]
fn bug_2162_void_cleanup_paths_stop_on_evidence_refusal() {
    let Refused { dir, b } = refused();
    let head = head_sha(&b).unwrap();
    let lease = claim_path(&b, "FR-1");
    let lock = lock_claim_path(&b, LockKind::Solo);
    let lock_before = std::fs::read_to_string(&lock).unwrap();

    release_claim(&b, "FR-1", dir.path());
    assert!(
        lease.exists(),
        "release_claim must not delete after a refusal"
    );
    heartbeat_lock_claim(&b, LockKind::Solo, dir.path());
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap(),
        lock_before,
        "heartbeat_lock_claim must not rewrite after a refusal"
    );
    release_lock_claim(&b, LockKind::Solo, dir.path());
    assert!(
        lock.exists(),
        "release_lock_claim must not delete after a refusal"
    );
    assert_eq!(head_sha(&b).unwrap(), head, "no commit after a refusal");

    // Control: once the hub is unreachable the same paths are best-effort
    // again and the local release still happens.
    git(&b, &["remote", "set-url", "origin", "/nonexistent/hub.git"]);
    release_lock_claim(&b, LockKind::Solo, dir.path());
    assert!(
        !lock.exists(),
        "an unreachable hub keeps the best-effort release"
    );
}

/// Both block-claim retry loops refresh through `pull_unless_refused` after
/// a rejected push (`b` is behind the hub): a refusal stops the claim with
/// the refusal itself instead of retrying or reporting a push failure.
#[test]
fn bug_2162_block_claim_retries_stop_on_evidence_refusal() {
    let Refused { dir: _dir, b } = refused();
    let err = crate::db_block::handle_block_command(
        &crate::cli::BlockCommand::Claim {
            r#type: "task".into(),
            size: 10,
        },
        &b,
    )
    .unwrap_err();
    assert!(is_evidence_refusal(&err), "{err:#}");

    let err = crate::db_block::auto_allocate_block_inner(
        &b,
        "node-b",
        "host",
        None,
        "TASK",
        10,
        crate::db_block::BlockClaimReason::OnThresholdCross,
    )
    .unwrap_err();
    assert!(is_evidence_refusal(&err), "{err:#}");
    assert!(
        !b.join(".git/rebase-merge").exists() && !b.join(".git/rebase-apply").exists(),
        "a refusal leaves no rebase behind"
    );
}

/// The id-allocation pull returns the refusal on its first attempt: it is
/// not retried and not rewrapped as a failed pull with rebase-abort advice.
#[test]
fn bug_2162_id_allocation_pull_returns_the_refusal() {
    let Refused { dir, b } = refused();
    let err = crate::store_sync::pull_store_before_id_allocation(&b, dir.path()).unwrap_err();
    assert!(is_evidence_refusal(&err), "{err:#}");
    assert!(!format!("{err:#}").contains("rebase --abort"), "{err:#}");
}
