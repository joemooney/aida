use super::*;
use process_probe::LiveSession;

fn lease_at(id: &str, worktree: &std::path::Path) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: id.to_string(),
        slug: id.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: worktree.to_path_buf(),
        branch: id.to_ascii_lowercase(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
        active_pid: None,
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
        claim_verb: true,
    }
}

fn live_session_at(cwd: &std::path::Path) -> LiveSession {
    LiveSession {
        pid: 4242,
        cwd: cwd.to_path_buf(),
        jsonl: None,
        stale_cwd: false,
    }
}

/// Many leases, ONE shared snapshot: every lease whose existing worktree is
/// covered by the single `live_sessions` slice classifies Live. The probe is
/// performed zero times by the function (it takes the snapshot by ref), so
/// liveness is O(leases) in-memory comparisons — never O(leases) probes.
#[test]
fn classifies_many_leases_from_single_snapshot() {
    let now = chrono::Utc::now();
    // Each lease points at a real, existing worktree dir (so
    // `worktree_exists` is true and classification turns on liveness only).
    let dirs: Vec<tempfile::TempDir> = (0..6).map(|_| tempfile::tempdir().unwrap()).collect();
    let leases: Vec<SessionLease> = dirs
        .iter()
        .enumerate()
        .map(|(i, d)| lease_at(&format!("TASK-{i}"), d.path()))
        .collect();

    // One snapshot covering only the FIRST three worktrees.
    let snapshot: Vec<LiveSession> = dirs
        .iter()
        .take(3)
        .map(|d| live_session_at(d.path()))
        .collect();

    let live = live_lease_worktrees(now, &leases, &snapshot);
    assert_eq!(
        live.len(),
        3,
        "exactly the leases covered by the single snapshot are Live"
    );
    for d in dirs.iter().take(3) {
        assert!(live.iter().any(|p| p == d.path()));
    }
}

/// An empty snapshot means no lease is Live (recent leases with existing
/// worktrees but no live claude classify Dormant, not Live).
#[test]
fn empty_snapshot_yields_no_live_leases() {
    let now = chrono::Utc::now();
    let dir = tempfile::tempdir().unwrap();
    let leases = vec![lease_at("TASK-1", dir.path())];
    let live = live_lease_worktrees(now, &leases, &[]);
    assert!(
        live.is_empty(),
        "no live sessions in the snapshot → no live leases"
    );
}

/// A stale-cwd (deleted-worktree) session never marks a lease Live, even if
/// its recorded cwd matches the lease path.
#[test]
fn stale_cwd_session_does_not_mark_live() {
    let now = chrono::Utc::now();
    let dir = tempfile::tempdir().unwrap();
    let leases = vec![lease_at("TASK-1", dir.path())];
    let stale = LiveSession {
        pid: 99,
        cwd: dir.path().to_path_buf(),
        jsonl: None,
        stale_cwd: true,
    };
    let live = live_lease_worktrees(now, &leases, &[stale]);
    assert!(live.is_empty(), "a stale-cwd session must not read Live");
}
