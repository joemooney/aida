//! BUG-HOME-024: finished sessions must not leave leases that block the next
//! review of the same PR.
//!
//!   * a dead lease whose worktree was deleted is released directly, with its
//!     stale git registration cleared, and no store write;
//!   * a live, unrecorded-owner, or still-present-worktree lease is kept;
//!   * a standalone reviewer's finished review (tree, lease, branch) is
//!     reaped once it exits with its verdict collected; one kept at exit
//!     keeps its lease, so the reap reclaims the tree after the merge;
//!   * a review pickup releases a dead gone-tree lease before its conflict
//!     check;
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
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
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

/// The gone-tree release, judged by the same verdict `aida queue work`
/// computes for a blocking lease.
fn release_gone(project_root: &std::path::Path, lease: &SessionLease) -> bool {
    let recovery = crate::leases::stale_lease_recovery_for_lease(lease).verdict;
    crate::session_reap::release_gone_worktree_lease(project_root, lease, &recovery)
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

    assert!(release_gone(repo.path(), &lease));
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

    assert!(!release_gone(repo.path(), &lease));
    assert!(lease_path(repo.path(), &lease.id).exists());
}

#[test]
fn lease_with_no_recorded_owner_is_kept() {
    let repo = committed_repo();
    let holder = tempfile::tempdir().unwrap();
    let path = deleted_worktree(repo.path(), holder.path(), "pr-2");
    let lease = lease_at("01a189c30000", "PR-2", &path, "pr-2", None);
    write_lease(repo.path(), &lease);

    assert!(!release_gone(repo.path(), &lease));
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

/// The automatic path: the quiet reap the drive tick runs releases a dead
/// gone-tree lease and reports it, without touching a live one.
#[test]
fn quiet_reap_releases_and_reports_a_dead_gone_tree_lease() {
    let repo = committed_repo();
    let holder = tempfile::tempdir().unwrap();
    let gone = deleted_worktree(repo.path(), holder.path(), "pr-6");
    let dead = lease_at("01a189c70000", "PR-6", &gone, "pr-6", Some(reaped_pid()));
    let live_tree = deleted_worktree(repo.path(), holder.path(), "pr-7");
    let live = lease_at(
        "01a189c80000",
        "PR-7",
        &live_tree,
        "pr-7",
        Some(std::process::id()),
    );
    for l in [&dead, &live] {
        write_lease(repo.path(), l);
    }

    let reap = crate::session_reap::reap_quiet_outcomes(repo.path());
    let reported: Vec<&str> = reap.leases.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(reported, vec![dead.id.as_str()], "{:?}", reap.leases);
    assert!(
        reap.leases[0].1.contains("worktree is gone"),
        "{:?}",
        reap.leases
    );
    assert!(!lease_path(repo.path(), &dead.id).exists());
    assert!(lease_path(repo.path(), &live.id).exists());
}

/// A primary checkout plus a clean review worktree `aida-pr-<pr>` on
/// `review/pr-<pr>`, held by a dead reviewer's lease that recorded the tree's
/// HEAD as the PR head. Returns `(tempdir, root, review tree, lease)`.
fn finished_review(
    pr: u32,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    SessionLease,
) {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let root = base.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.name", "AIDA Test"],
        &["config", "user.email", "aida@example.test"],
        &["config", "commit.gpgsign", "false"],
    ] {
        assert!(git(&root, args).status.success(), "git {args:?}");
    }
    std::fs::write(root.join("README.md"), "seed\n").unwrap();
    assert!(git(&root, &["add", "README.md"]).status.success());
    assert!(git(&root, &["commit", "-qm", "seed"]).status.success());
    // Like the real clone: per-checkout `.aida/` state is not tracked.
    std::fs::write(root.join(".git/info/exclude"), "/.aida/\n").unwrap();
    let review = base.join(format!("aida-pr-{pr}"));
    let branch = format!("review/pr-{pr}");
    let added = git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &branch,
            review.to_str().unwrap(),
        ],
    );
    assert!(added.status.success(), "{added:?}");
    let head = String::from_utf8_lossy(&git(&review, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();
    let mut lease = lease_at(
        &format!("01a18a6{pr:05}"),
        &format!("PR-{pr}"),
        &review,
        &branch,
        Some(reaped_pid()),
    );
    lease.review_verb = true;
    lease.pr_head_sha = Some(head);
    write_lease(&root, &lease);
    (tmp, root, review, lease)
}

fn local_branch_exists(root: &std::path::Path, branch: &str) -> bool {
    git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .status
    .success()
}

#[cfg(unix)]
#[test]
fn finished_review_with_a_collected_verdict_is_reaped_at_exit() {
    let (_tmp, root, review, lease) = finished_review(3107);
    // Another review sharing nothing with this one is left alone.
    let other = lease_at(
        "01a18a650000",
        "PR-3105",
        &root.join("aida-pr-3105"),
        "review/pr-3105",
        None,
    );
    write_lease(&root, &other);

    let ended = crate::queue_cmd::end_finished_review(&root, "pr-3107", &review, true);
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert_eq!(ended[0].0, lease.id);
    assert!(ended[0].1.is_ok(), "{ended:?}");
    // Tree, lease, and review branch go together: nothing blocks the next
    // review of the PR, and nothing is left behind for after the merge.
    assert!(!review.exists());
    assert!(!lease_path(&root, &lease.id).exists());
    assert!(!local_branch_exists(&root, "review/pr-3107"));
    assert!(lease_path(&root, &other.id).exists());
    // The TASK-1955 reap has nothing left to do for this review, even with
    // the PR merged.
    let report = crate::session_reap::scan_reapable_with(&root, |_| Some(true));
    assert!(
        report.reapable.iter().all(|r| r.session != lease.id),
        "{report:?}"
    );
}

#[cfg(unix)]
#[test]
fn review_kept_at_exit_keeps_its_lease_and_is_reaped_after_merge() {
    let (_tmp, root, review, lease) = finished_review(3108);

    // No verdict collected: the recovery text points at the tree.
    let ended = crate::queue_cmd::end_finished_review(&root, "PR-3108", &review, false);
    assert!(ended[0].1.is_err(), "{ended:?}");
    assert!(review.exists());
    assert!(lease_path(&root, &lease.id).exists());

    // The kept lease is the dead-reviewer reap's handle: once the PR is
    // merged, the tree is reclaimed.
    let report = crate::session_reap::scan_reapable_with(&root, |pr| Some(pr == 3108));
    let row = report
        .reapable
        .iter()
        .find(|r| r.session == lease.id)
        .unwrap_or_else(|| panic!("kept review is reapable after merge: {report:?}"));
    let outcome = crate::session_reap::reap_one(&root, &lease, row.branch_tip.as_deref());
    assert!(!review.exists(), "review tree reclaimed: {outcome}");
    assert!(!lease_path(&root, &lease.id).exists());
}

#[cfg(unix)]
#[test]
fn dirty_review_tree_is_never_reaped_at_exit() {
    let (_tmp, root, review, lease) = finished_review(3109);
    std::fs::write(review.join("notes.txt"), "unsaved\n").unwrap();

    let ended = crate::queue_cmd::end_finished_review(&root, "PR-3109", &review, true);
    let why = ended[0].1.as_ref().unwrap_err();
    assert!(why.contains("uncommitted"), "{why}");
    assert!(review.join("notes.txt").exists());
    assert!(lease_path(&root, &lease.id).exists());
}

/// The `aida queue work PR-N --role reviewer` pickup's branch-conflict check
/// (BUG-HOME-034): a dead lease on the PR's source branch whose tree was
/// deleted by hand is released before the check, with no `session end` and
/// so no store write to contend on; a live one still blocks.
#[test]
fn review_pickup_releases_a_dead_gone_tree_lease_before_the_conflict_check() {
    let repo = committed_repo();
    let holder = tempfile::tempdir().unwrap();
    let dead_tree = deleted_worktree(repo.path(), holder.path(), "feature-x");
    let mut dead = lease_at(
        "01a18a700000",
        "BUG-7",
        &dead_tree,
        "feature-x",
        Some(reaped_pid()),
    );
    dead.role = Some("implementer".into());
    let live = lease_at(
        "01a18a710000",
        "BUG-8",
        &holder.path().join("wt-live"),
        "feature-x",
        Some(std::process::id()),
    );
    let elsewhere = lease_at(
        "01a18a720000",
        "BUG-9",
        &holder.path().join("wt-other"),
        "feature-y",
        Some(reaped_pid()),
    );
    for l in [&dead, &live, &elsewhere] {
        write_lease(repo.path(), l);
    }

    let conflicting = crate::session_start::review_branch_conflicts(repo.path(), "feature-x");
    let ids: Vec<&str> = conflicting.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(ids, vec![live.id.as_str()]);
    assert!(!lease_path(repo.path(), &dead.id).exists());
    assert!(lease_path(repo.path(), &live.id).exists());
    assert!(lease_path(repo.path(), &elsewhere.id).exists());
}

#[test]
fn every_id_session_leases_prints_is_accepted_by_session_end() {
    // Two ids sharing an 8-char prefix print at 9 characters.
    let leases = vec![
        lease_at(
            "01a18a648aaa",
            "PR-1",
            std::path::Path::new("wt-a"),
            "pr-1",
            None,
        ),
        lease_at(
            "01a18a64d782",
            "PR-2",
            std::path::Path::new("wt-b"),
            "pr-2",
            None,
        ),
        lease_at(
            "01a18b000000",
            "PR-3",
            std::path::Path::new("wt-c"),
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
        std::path::Path::new("wt-b"),
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
