//! BUG-HOME-024: finished sessions must not leave leases that block the next
//! review of the same PR.
//!
//!   * a dead lease whose worktree was deleted is released directly, with its
//!     stale git registration cleared, and no store write;
//!   * a live, unrecorded-owner, or still-present-worktree lease is kept;
//!   * a standalone reviewer's lease is released once the reviewer exits;
//!   * a session id `aida session leases` prints is accepted by `aida session
//!     end`, and a missing one is reported as a session id, not a branch.
//!
//! trace:BUG-b6b1278890774ef993d2ebc327862470-024 | ai:claude
use crate::*;

fn lease_at(
    id: &str,
    scope: &str,
    worktree: &std::path::Path,
    branch: &str,
    creator_pid: Option<u32>,
) -> SessionLease {
    SessionLease {
        tree_kind: Default::default(),
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_lowercase(),
        owner: "u".into(),
        worktree_path: worktree.to_path_buf(),
        branch: branch.into(),
        started_at: chrono::Utc::now() - chrono::Duration::seconds(120),
        hostname: "h".into(),
        role: Some("reviewer".into()),
        creator_pid,
        creator_pid_start_time: None,
        active_pid: None,
        active_pid_start_time: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        ending_retained_at: None,
        ending_retained_reason: None,
        driver: None,
        child_grant_id: None,
    }
}

/// A pid verifiably absent from the process table: a reaped child.
fn reaped_pid() -> u32 {
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .expect("spawn sh");
    let pid = child.id();
    let _ = child.wait();
    pid
}

fn git(repo: &std::path::Path, args: &[&str]) -> std::process::Output {
    crate::test_git::git_command()
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git")
}

fn committed_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    assert!(git(dir.path(), &["init", "-q"]).status.success());
    std::fs::write(dir.path().join("README.md"), "seed\n").unwrap();
    assert!(git(dir.path(), &["add", "README.md"]).status.success());
    assert!(git(
        dir.path(),
        &[
            "-c",
            "user.name=AIDA Test",
            "-c",
            "user.email=aida@example.test",
            "commit",
            "-qm",
            "seed",
        ],
    )
    .status
    .success());
    dir
}

fn write_lease(project_root: &std::path::Path, lease: &SessionLease) {
    std::fs::create_dir_all(leases_dir(project_root)).unwrap();
    std::fs::write(
        lease_path(project_root, &lease.id),
        toml::to_string_pretty(lease).unwrap(),
    )
    .unwrap();
}

/// A linked worktree on `branch`, deleted by hand (git still lists it).
fn deleted_worktree(
    repo: &std::path::Path,
    holder: &std::path::Path,
    branch: &str,
) -> std::path::PathBuf {
    let path = holder.join(format!("wt-{branch}"));
    let added = git(
        repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            branch,
            path.to_str().unwrap(),
        ],
    );
    assert!(added.status.success(), "{added:?}");
    std::fs::remove_dir_all(&path).unwrap();
    path
}

#[test]
fn dead_lease_with_deleted_worktree_is_released_and_its_branch_freed() {
    let repo = committed_repo();
    let holder = tempfile::tempdir().unwrap();
    let path = deleted_worktree(repo.path(), holder.path(), "pr-3106");
    let lease = lease_at(
        "01a189c10000",
        "PR-3106",
        &path,
        "pr-3106",
        Some(reaped_pid()),
    );
    write_lease(repo.path(), &lease);

    assert!(crate::session_reap::release_gone_worktree_lease(
        repo.path(),
        &lease
    ));
    assert!(!lease_path(repo.path(), &lease.id).exists());
    // The branch is no longer held by the missing tree, so a new review can
    // check it out again.
    let again = holder.path().join("wt-again");
    let readd = git(
        repo.path(),
        &["worktree", "add", "-q", again.to_str().unwrap(), "pr-3106"],
    );
    assert!(readd.status.success(), "{readd:?}");
}

#[test]
fn lease_whose_owner_is_alive_is_kept_even_with_its_worktree_gone() {
    let repo = committed_repo();
    let holder = tempfile::tempdir().unwrap();
    let path = deleted_worktree(repo.path(), holder.path(), "pr-1");
    let lease = lease_at(
        "01a189c20000",
        "PR-1",
        &path,
        "pr-1",
        Some(std::process::id()),
    );
    write_lease(repo.path(), &lease);

    assert!(!crate::session_reap::release_gone_worktree_lease(
        repo.path(),
        &lease
    ));
    assert!(lease_path(repo.path(), &lease.id).exists());
}

#[test]
fn lease_with_no_recorded_owner_is_kept() {
    let repo = committed_repo();
    let holder = tempfile::tempdir().unwrap();
    let path = deleted_worktree(repo.path(), holder.path(), "pr-2");
    let lease = lease_at("01a189c30000", "PR-2", &path, "pr-2", None);
    write_lease(repo.path(), &lease);

    assert!(!crate::session_reap::release_gone_worktree_lease(
        repo.path(),
        &lease
    ));
    assert!(lease_path(repo.path(), &lease.id).exists());
}

#[test]
fn sweep_releases_only_dead_leases_whose_worktree_is_gone() {
    let repo = committed_repo();
    let holder = tempfile::tempdir().unwrap();
    let gone = deleted_worktree(repo.path(), holder.path(), "pr-3");
    let present = holder.path().join("present");
    std::fs::create_dir_all(&present).unwrap();
    let dead_gone = lease_at("01a189c40000", "PR-3", &gone, "pr-3", Some(reaped_pid()));
    let dead_present = lease_at("01a189c50000", "PR-4", &present, "pr-4", Some(reaped_pid()));
    let advisory = lease_at(
        "01a189c60000",
        "PR-5",
        std::path::Path::new(""),
        "main",
        Some(reaped_pid()),
    );
    for l in [&dead_gone, &dead_present, &advisory] {
        write_lease(repo.path(), l);
    }

    let released = crate::session_reap::release_gone_worktree_leases(repo.path());
    assert_eq!(
        released,
        vec![("01a189c40000".to_string(), "PR-3".to_string())]
    );
    assert!(lease_path(repo.path(), &dead_present.id).exists());
    assert!(lease_path(repo.path(), &advisory.id).exists());
}

#[test]
fn exited_reviewer_releases_only_its_own_lease() {
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("aida-pr-3107");
    std::fs::create_dir_all(&worktree).unwrap();
    let other_tree = root.path().join("elsewhere");
    let own = lease_at("01a18a640000", "PR-3107", &worktree, "pr-3107", None);
    let other_scope = lease_at("01a18a650000", "PR-3105", &worktree, "pr-3105", None);
    let other_tree_lease = lease_at("01a18a660000", "PR-3107", &other_tree, "pr-3107", None);
    for l in [&own, &other_scope, &other_tree_lease] {
        write_lease(root.path(), l);
    }

    let released =
        crate::queue_cmd::release_exited_reviewer_lease(root.path(), "pr-3107", &worktree);
    assert_eq!(released, vec![own.id.clone()]);
    assert!(!lease_path(root.path(), &own.id).exists());
    assert!(lease_path(root.path(), &other_scope.id).exists());
    assert!(lease_path(root.path(), &other_tree_lease.id).exists());
    // The worktree stays for recovery and reuse by the next review.
    assert!(worktree.exists());
}

#[test]
fn every_id_session_leases_prints_is_accepted_by_session_end() {
    // Two ids sharing an 8-char prefix print at 9 characters.
    let leases = vec![
        lease_at(
            "01a18a648aaa",
            "PR-1",
            std::path::Path::new("/tmp/a"),
            "pr-1",
            None,
        ),
        lease_at(
            "01a18a64d782",
            "PR-2",
            std::path::Path::new("/tmp/b"),
            "pr-2",
            None,
        ),
        lease_at(
            "01a18b000000",
            "PR-3",
            std::path::Path::new("/tmp/c"),
            "pr-3",
            None,
        ),
    ];
    let ids: Vec<&str> = leases.iter().map(|l| l.id.as_str()).collect();
    for l in &leases {
        let shown = &l.id[..unique_prefix_len(&l.id, &ids, 8)];
        let got = resolve_session_to_end(Some(shown), None, None, &leases, false).unwrap();
        assert_eq!(got.id, l.id, "printed id `{shown}`");
    }
}

#[test]
fn missing_session_id_is_reported_as_an_id_not_a_branch() {
    let leases = vec![lease_at(
        "01a18a64d782",
        "PR-2",
        std::path::Path::new("/tmp/b"),
        "pr-2",
        None,
    )];
    let err = resolve_session_to_end(Some("01a18a648"), None, None, &leases, false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("No session with id `01a18a648`"), "{err}");
    assert!(!err.contains("branch"), "{err}");
    assert!(err.contains("aida session leases"), "{err}");

    // A branch-shaped miss keeps the branch error.
    let err = resolve_session_to_end(Some("feature-x"), None, None, &leases, false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("No lease found for branch"), "{err}");
}

#[test]
fn lease_id_shape() {
    assert!(crate::session_end::looks_like_lease_id("01a18a648"));
    assert!(crate::session_end::looks_like_lease_id("019e71db-48e0"));
    assert!(!crate::session_end::looks_like_lease_id("01a18a6"));
    assert!(!crate::session_end::looks_like_lease_id("feature-x"));
    assert!(!crate::session_end::looks_like_lease_id("-01a18a648"));
}
