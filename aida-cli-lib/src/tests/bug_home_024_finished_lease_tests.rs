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
use crate::{lease_path, leases_dir, resolve_session_to_end, unique_prefix_len, SessionLease};

fn lease_at(
    id: &str,
    scope: &str,
    worktree: &std::path::Path,
    branch: &str,
    creator_pid: Option<u32>,
) -> SessionLease {
    SessionLease {
        holder_record: None,
        pid_namespace: crate::agent_registry::current_pid_namespace(),
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
        creator_pid_ns: crate::gate::review_authority::origin_pid_ns(),
        active_pid_ns: crate::gate::review_authority::origin_pid_ns(),
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
        vendor_session_id: None,
        lane_jobs: None,
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

    let ended = crate::queue_cmd::end_finished_review(&root, "pr-3107", &review, &lease.id, true);
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
    let ended = crate::queue_cmd::end_finished_review(&root, "PR-3108", &review, &lease.id, false);
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

    let ended = crate::queue_cmd::end_finished_review(&root, "PR-3109", &review, &lease.id, true);
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

/// BUG-2684: a finished reviewer's stale lease whose tree is gone is
/// released by the review pickup with no prompt only when its holder is
/// positively dead. Unknown liveness (no recorded process, a record naming a
/// replacement holder, a live legacy creator), a live owner, or a present
/// tree keeps it.
// trace:BUG-2684 | ai:claude
#[test]
fn bug_2684_stale_review_lease_with_gone_tree_is_released_without_prompt() {
    use crate::ownership::{dual, store::Expect, Ownership, Phase, WorkKey};
    let repo = committed_repo();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(repo.path(), "reviewer", &[]);
    let holder = tempfile::tempdir().unwrap();
    let head = String::from_utf8(git(repo.path(), &["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .to_string();
    let path = deleted_worktree(repo.path(), holder.path(), "pr-4354");
    let mut lease = lease_at("01a1f76fcee1", "PR-4354", &path, "pr-4354", None);
    lease.review_verb = true;
    lease.active_pid = Some(reaped_pid());
    lease.pr_head_sha = Some(head.clone());
    write_lease(repo.path(), &lease);
    let key = WorkKey::Review {
        change: dual::change(4354),
        head: head.clone(),
    };
    let mut owner = dual::holder_for_lease(&lease, &dual::WriterFacts::here(), chrono::Utc::now());
    owner.witnesses.clear();
    crate::ownership::store::OwnershipStore::for_project(repo.path())
        .unwrap()
        .unwrap()
        .update(&key, "fixture", Expect::Any, |_| {
            Some((
                Ownership {
                    key: key.clone(),
                    generation: 0,
                    phase: Phase::Reviewing { head: head.clone() },
                    holder: Some(owner.clone()),
                },
                None,
            ))
        })
        .unwrap();

    // Fail closed first. Witness-less and pid-less: no positive proof.
    let mut pidless = lease.clone();
    pidless.active_pid = None;
    write_lease(repo.path(), &pidless);
    assert!(!crate::session_start::release_dead_gone_tree_conflict(
        repo.path(),
        &pidless
    ));
    assert!(lease_path(repo.path(), &lease.id).exists());
    // Every recorded process dead, but a replacement holder owns the key.
    write_lease(repo.path(), &lease);
    let mut replacement = owner.clone();
    replacement.session = "01a1f76fcee9".into();
    let set_holder = |holder: crate::ownership::Holder| {
        crate::ownership::store::OwnershipStore::for_project(repo.path())
            .unwrap()
            .unwrap()
            .update(&key, "fixture", Expect::Any, |_| {
                Some((
                    Ownership {
                        key: key.clone(),
                        generation: 0,
                        phase: Phase::Reviewing { head: head.clone() },
                        holder: Some(holder),
                    },
                    None,
                ))
            })
            .unwrap();
    };
    set_holder(replacement);
    let why = crate::ownership::reader::review_holder_proven_dead(repo.path(), &lease).unwrap_err();
    assert!(why.contains("different session"), "{why}");
    assert!(!crate::session_start::release_dead_gone_tree_conflict(
        repo.path(),
        &lease
    ));
    assert!(lease_path(repo.path(), &lease.id).exists());
    set_holder(owner);

    // The observed stale reviewer lease: witness-less, its recorded worker
    // positively gone. Its holder is proven dead, so the review pickup
    // releases it without asking.
    assert!(crate::ownership::reader::review_holder_proven_dead(repo.path(), &lease).is_ok());
    assert!(!crate::ownership::reader::lease_retained(
        repo.path(),
        &lease
    ));
    assert!(crate::session_start::release_dead_gone_tree_conflict(
        repo.path(),
        &lease
    ));
    assert!(!lease_path(repo.path(), &lease.id).exists());
    let again = holder.path().join("wt-again");
    let readd = git(
        repo.path(),
        &["worktree", "add", "-q", again.to_str().unwrap(), "pr-4354"],
    );
    assert!(readd.status.success(), "{readd:?}");

    // A live owner keeps its lease even with its tree gone.
    let gone = deleted_worktree(repo.path(), holder.path(), "pr-8");
    let mut live = lease_at(
        "01a1f76fcee3",
        "PR-8",
        &gone,
        "pr-8",
        Some(std::process::id()),
    );
    live.review_verb = true;
    write_lease(repo.path(), &live);
    assert!(!crate::session_start::release_dead_gone_tree_conflict(
        repo.path(),
        &live
    ));
    assert!(lease_path(repo.path(), &live.id).exists());

    // A legacy lease (no ownership record) whose worker is dead but whose
    // creator is alive outside the tree is not finished.
    let gone_creator = deleted_worktree(repo.path(), holder.path(), "pr-9");
    let mut creator = lease_at(
        "01a1f76fcee5",
        "PR-9",
        &gone_creator,
        "pr-9",
        Some(std::process::id()),
    );
    creator.review_verb = true;
    creator.active_pid = Some(reaped_pid());
    write_lease(repo.path(), &creator);
    assert!(
        crate::ownership::reader::lease_record(repo.path(), &creator)
            .unwrap()
            .is_none()
    );
    let why =
        crate::ownership::reader::review_holder_proven_dead(repo.path(), &creator).unwrap_err();
    assert!(why.contains("creator process"), "{why}");
    assert!(!crate::session_start::release_dead_gone_tree_conflict(
        repo.path(),
        &creator
    ));
    assert!(lease_path(repo.path(), &creator.id).exists());

    // A provably dead lease whose tree still exists is not released here.
    let present = holder.path().join("wt-present");
    std::fs::create_dir_all(&present).unwrap();
    let mut kept = lease_at("01a1f76fcee2", "PR-7", &present, "pr-7", Some(reaped_pid()));
    kept.review_verb = true;
    write_lease(repo.path(), &kept);
    assert!(!crate::session_start::release_dead_gone_tree_conflict(
        repo.path(),
        &kept
    ));
    assert!(lease_path(repo.path(), &kept.id).exists());

    // A dead implementer lease keeps the guarded `session end` path.
    let implementer_tree = deleted_worktree(repo.path(), holder.path(), "feature-z");
    let mut implementer = lease_at(
        "01a1f76fcee4",
        "BUG-9",
        &implementer_tree,
        "feature-z",
        Some(reaped_pid()),
    );
    implementer.role = Some("implementer".into());
    write_lease(repo.path(), &implementer);
    assert!(!crate::session_start::release_dead_gone_tree_conflict(
        repo.path(),
        &implementer
    ));
    assert!(lease_path(repo.path(), &implementer.id).exists());
}

/// BUG-2684: a reviewer lease whose holder cannot be verified is never
/// proven dead. A pid-less lease in a pid namespace this host cannot observe
/// is kept whatever its age, a live recorded holder naming this session is
/// kept, and an unreadable ownership record is kept; none is released even
/// with its tree gone.
// trace:BUG-2684 | ai:claude
#[test]
fn bug_2684_unverifiable_review_holder_is_never_proven_dead() {
    use crate::ownership::{dual, store::Expect, Ownership, Phase, WorkKey};
    let repo = committed_repo();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(repo.path(), "reviewer", &[]);
    let holder = tempfile::tempdir().unwrap();
    let head = String::from_utf8(git(repo.path(), &["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .to_string();
    let kept = |lease: &SessionLease, expect: &str| {
        let why =
            crate::ownership::reader::review_holder_proven_dead(repo.path(), lease).unwrap_err();
        assert!(why.contains(expect), "{why}");
        assert!(!crate::ownership::reader::stale_review_lease_ended(
            repo.path(),
            lease
        ));
        assert!(!crate::session_start::release_dead_gone_tree_conflict(
            repo.path(),
            lease
        ));
        assert!(lease_path(repo.path(), &lease.id).exists());
    };
    let record = |key: &WorkKey, lease: &SessionLease| {
        let owner = dual::holder_for_lease(lease, &dual::WriterFacts::here(), chrono::Utc::now());
        crate::ownership::store::OwnershipStore::for_project(repo.path())
            .unwrap()
            .unwrap()
            .update(key, "fixture", Expect::Any, |_| {
                Some((
                    Ownership {
                        key: key.clone(),
                        generation: 0,
                        phase: Phase::Reviewing { head: head.clone() },
                        holder: Some(owner.clone()),
                    },
                    None,
                ))
            })
            .unwrap();
    };

    // A pid-less legacy lease stamped in a foreign pid namespace, a month
    // old: the age backstop alone calls its owner gone, and that is not proof.
    let gone = deleted_worktree(repo.path(), holder.path(), "pr-4401");
    let mut foreign = lease_at("01a1f76fce01", "PR-4401", &gone, "pr-4401", None);
    foreign.review_verb = true;
    foreign.pid_namespace =
        Some(crate::agent_registry::current_pid_namespace().map_or(7, |ns| ns ^ 1));
    foreign.started_at = chrono::Utc::now() - chrono::Duration::days(30);
    write_lease(repo.path(), &foreign);
    assert!(
        crate::ownership::reader::lease_record(repo.path(), &foreign)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        crate::leases::lease_owner_gone(&foreign),
        Some(true),
        "the age backstop alone presumes this owner gone"
    );
    kept(&foreign, "pid namespace this host cannot observe");

    // A record naming this session whose admitted worker is alive.
    let gone = deleted_worktree(repo.path(), holder.path(), "pr-4403");
    let mut live = lease_at("01a1f76fce03", "PR-4403", &gone, "pr-4403", None);
    live.review_verb = true;
    live.pr_head_sha = Some(head.clone());
    let mut command = std::process::Command::new("sleep");
    command.arg("30");
    let capture = aida_core::proc_identity::prepare_command(&mut command).unwrap();
    let mut child = command.spawn().unwrap();
    let admission = capture.finish(&mut child);
    live.active_pid = Some(child.id());
    live.active_pid_start_time = crate::process_probe::process_start_identity(child.id());
    live.hostname = crate::coordination::hostname();
    live.pid_namespace = dual::WriterFacts::here().pid_ns;
    live.holder_record = Some(serde_json::to_string(&admission).unwrap());
    write_lease(repo.path(), &live);
    let live_key = WorkKey::Review {
        change: dual::change(4403),
        head: head.clone(),
    };
    record(&live_key, &live);
    let verdict = crate::ownership::reader::lease_owner(repo.path(), &live);
    let why = crate::ownership::reader::review_holder_proven_dead(repo.path(), &live);
    let released = crate::session_start::release_dead_gone_tree_conflict(repo.path(), &live);
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        matches!(verdict, crate::ownership::OwnerVerdict::Alive { .. }),
        "{verdict:?}"
    );
    let why = why.unwrap_err();
    assert!(why.contains("still alive"), "{why}");
    assert!(!released);
    assert!(lease_path(repo.path(), &live.id).exists());

    // An unreadable ownership record: every recorded process is positively
    // dead, but the evidence of who holds the key cannot be read.
    let gone = deleted_worktree(repo.path(), holder.path(), "pr-4402");
    let mut unreadable = lease_at("01a1f76fce02", "PR-4402", &gone, "pr-4402", None);
    unreadable.review_verb = true;
    unreadable.active_pid = Some(reaped_pid());
    unreadable.pr_head_sha = Some(head.clone());
    write_lease(repo.path(), &unreadable);
    let key = WorkKey::Review {
        change: dual::change(4402),
        head: head.clone(),
    };
    record(&key, &unreadable);
    assert!(crate::ownership::reader::lease_record(repo.path(), &unreadable).is_ok());
    let store = crate::ownership::store::OwnershipStore::for_project(repo.path())
        .unwrap()
        .unwrap();
    std::fs::write(store.path_for(&key), "{ not an ownership record").unwrap();
    assert!(crate::ownership::reader::lease_record(repo.path(), &unreadable).is_err());
    kept(&unreadable, "ownership evidence unreadable");
}

/// BUG-2684: the review lease-conflict prompt is never offered to a
/// `--no-human` pickup, a headless environment, or a caller with no terminal
/// on stdin; an attended terminal may prompt.
// trace:BUG-2684 | ai:claude
#[test]
fn bug_2684_no_human_review_pickup_never_prompts() {
    use crate::session_start::review_conflict_cannot_prompt_from as cannot;
    assert!(
        !cannot(false, false, true),
        "an attended terminal may prompt"
    );
    assert!(cannot(true, false, true), "--no-human never prompts");
    assert!(cannot(false, true, true), "AIDA_HEADLESS never prompts");
    assert!(cannot(false, false, false), "no TTY never prompts");
    // A no-human decision for a live reviewer lease refuses; a dead one ends.
    assert_eq!(
        crate::leases::review_source_branch_conflict_decision(
            cannot(true, false, true),
            true,
            Some("reviewer"),
            Some("reviewer"),
            None,
        ),
        crate::leases::LeaseConflictDecision::Refuse
    );
    assert_eq!(
        crate::leases::review_source_branch_conflict_decision(
            cannot(true, false, true),
            false,
            Some("reviewer"),
            Some("reviewer"),
            None,
        ),
        crate::leases::LeaseConflictDecision::AutoEnd
    );
}
