//! TASK-2823: a lane launch naming `--branch` and an existing `--cwd` (a
//! dispatch `take` on a PR branch already checked out in a linked worktree)
//! re-enters that worktree instead of a `git worktree add` git refuses; a
//! tree another session's lease records is refused. TASK-2822: a re-entered
//! lease on another branch than the lane's is refused.
//! trace:TASK-2823 TASK-2822 | ai:claude

use super::tests::{fixture, git_ok};
use crate::session_start::PathReentry;
use std::path::Path;

fn start(
    owns: &str,
    branch: Option<&str>,
    path: Option<&Path>,
    reentry: PathReentry,
) -> anyhow::Result<()> {
    crate::session_start::session_start_with_receipt(
        owns,
        branch,
        None,
        false,
        path.map(|p| p.to_str().unwrap()),
        None,
        "auto",
        false,
        None,
        false,
        None,
        None,
        false,
        None,
        /* force_claim */ false,
        Some(false),
        /* self_claim */ true,
        /* review_coexistence */ None,
        /* contained_seat */ false,
        /* rollback_failed_setup */ false,
        reentry,
    )
    .map(|mut receipt| receipt.disarm())
}

fn lease_for(main: &Path, scope: &str) -> crate::SessionLease {
    crate::list_leases(main)
        .into_iter()
        .find(|l| l.scope == scope)
        .unwrap_or_else(|| panic!("no lease for {scope}"))
}

fn canon(p: &Path) -> std::path::PathBuf {
    p.canonicalize().unwrap()
}

// trace:TASK-2823 | ai:claude
#[test]
fn a_lane_reenters_a_free_worktree_already_on_its_branch() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    git_ok(&fx.main, &["init", "-q", ".aida-store"]);
    let tree = fx.root.join("pr-checkout");
    git_ok(
        &fx.main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "lane/x",
            tree.to_str().unwrap(),
        ],
    );
    std::fs::write(tree.join("wip.txt"), "kept\n").unwrap();
    let scope = "TASK-928231";

    // Without lane re-entry an existing path is refused, as before.
    let err = format!(
        "{:#}",
        start(scope, Some("lane/x"), Some(&tree), PathReentry::None).unwrap_err()
    );
    assert!(err.contains("already exists"), "{err}");

    start(
        scope,
        Some("lane/x"),
        Some(&tree),
        PathReentry::LaneWorktree,
    )
    .unwrap();
    let lease = lease_for(&fx.main, scope);
    assert_eq!(canon(&lease.worktree_path), canon(&tree));
    assert_eq!(lease.branch, "lane/x");
    assert_eq!(
        std::fs::read_to_string(tree.join("wip.txt")).unwrap(),
        "kept\n",
        "the tree's work is kept"
    );
}

// trace:TASK-2823 | ai:claude
#[test]
fn a_lane_does_not_reenter_a_tree_another_session_records() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    git_ok(&fx.main, &["init", "-q", ".aida-store"]);
    start("TASK-928232", None, None, PathReentry::None).unwrap();
    let held = lease_for(&fx.main, "TASK-928232");

    let err = format!(
        "{:#}",
        start(
            "TASK-928233",
            Some(&held.branch),
            Some(&held.worktree_path),
            PathReentry::LaneWorktree,
        )
        .unwrap_err()
    );
    assert!(err.contains("still records it"), "{err}");
    assert!(err.contains(&held.id), "{err}");
    assert!(crate::list_leases(&fx.main)
        .iter()
        .all(|l| l.scope != "TASK-928233"));
}

/// TASK-2822: re-entering a spec's lease keeps its branch, so a launch that
/// named another lane branch is refused, naming the lease and its remedy;
/// the lease's own branch, or no branch, re-enters.
// trace:TASK-2822 | ai:claude
#[test]
fn reentering_a_lease_on_another_branch_than_the_lane_is_refused() {
    use crate::agent_launch::{ensure_reentered_lease_on_lane_branch, LaneBranch};
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    git_ok(&fx.main, &["init", "-q", ".aida-store"]);
    start("TASK-928234", None, None, PathReentry::None).unwrap();
    let lease = lease_for(&fx.main, "TASK-928234");

    let err = ensure_reentered_lease_on_lane_branch(
        &lease,
        &LaneBranch {
            branch: Some("lane/elsewhere".into()),
            base: None,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("refusing to launch on branch `lane/elsewhere`"),
        "{err}"
    );
    assert!(err.contains(&format!("branch `{}`", lease.branch)), "{err}");
    assert!(
        err.contains(&format!("aida session end {}", lease.id)),
        "{err}"
    );

    ensure_reentered_lease_on_lane_branch(
        &lease,
        &LaneBranch {
            branch: Some(lease.branch.clone()),
            base: Some("origin/main".into()),
        },
    )
    .unwrap();
    ensure_reentered_lease_on_lane_branch(&lease, &LaneBranch::default()).unwrap();
}
