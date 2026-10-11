//! BUG-1941: `aida ps` regressions on the main checkout.
//! (a) no Salvageable reading for a worktree a live process occupies, and
//!     untracked-only scratch on the main checkout is not salvageable;
//! (b) the started and elapsed columns share one anchor;
//! (c) the backing pid is chosen deterministically and main-checkout
//!     placeholder leases are not labelled `subagent`;
//! (d) a no-process row whose dispatch reads `moving` says so in the human
//!     view, and a fresh branch commit counts as movement.
//
// trace:BUG-1941 | ai:claude

use super::*;
use crate::dispatch_health_ps::{
    apply_commit_movement, apply_main_checkout_untracked, parse_commit_time_and_subject,
    pid_liveness_with_occupant, records_are_untracked_only, DispatchState, WorktreeGitProbe,
    DEFAULT_DIRTY_MOVEMENT_FRESH_SECS,
};
use std::path::{Path, PathBuf};

/// A pid that is never alive on a test host.
const DEAD_PID: u32 = 999_999_999;

fn lease(id: &str, scope: &str, worktree: PathBuf, branch: &str) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: worktree,
        branch: branch.to_string(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
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
    }
}

/// A legacy `harness-worktree` lease on the main checkout, as BUG-1941 saw
/// them: placeholder role, worktree == project root, branch main.
fn main_checkout_lease(id: &str, root: &Path) -> SessionLease {
    let mut l = lease(id, "harness-worktree", root.to_path_buf(), "main");
    l.role = Some(tail_cmd::HARNESS_AGENT_TYPE.into());
    l.parent_project_root = Some(root.to_path_buf());
    l
}

fn live_at(pid: u32, cwd: &Path) -> process_probe::LiveSession {
    process_probe::LiveSession {
        pid,
        cwd: cwd.to_path_buf(),
        jsonl: None,
        stale_cwd: false,
    }
}

fn rows_for(
    leases: &[SessionLease],
    live: &[process_probe::LiveSession],
    probe: WorktreeGitProbe,
) -> Vec<PsRow> {
    build_running_work(
        &[],
        leases,
        live,
        chrono::Utc::now(),
        move |_| probe.clone(),
        |_| None,
        |_| None,
        |_| Some("advisor".into()),
        |_| None,
        |_| MailIdentityStatus::Unknown,
        |_, _| SeatActivity::Unknown,
    )
    .0
}

fn dirty_probe() -> WorktreeGitProbe {
    WorktreeGitProbe {
        dirty: true,
        last_commit_subject: Some("fix: something".into()),
        ..Default::default()
    }
}

// ── (a) ────────────────────────────────────────────────────────────────

#[test]
fn bug_1941_dead_lease_in_occupied_worktree_is_not_salvageable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut l = main_checkout_lease("dead-pid-lease", root);
    l.active_pid = Some(DEAD_PID);
    // Tracked changes, so only the live occupant can keep this off Salvageable.
    let rows = rows_for(&[l], &[live_at(std::process::id(), root)], dirty_probe());
    assert_eq!(rows[0].state, LeaseState::Stale, "its own pid is dead");
    let d = rows[0].dispatch.as_ref().unwrap();
    assert_ne!(
        d.state,
        DispatchState::Salvageable,
        "a worktree with a live process in it is never salvageable"
    );
    assert!(!d.hint.as_deref().unwrap_or("").contains("dead process"));
}

#[test]
fn bug_1941_untracked_only_on_main_checkout_is_not_salvageable() {
    let tmp = tempfile::tempdir().unwrap();
    let mut l = main_checkout_lease("dead-pid-lease", tmp.path());
    l.active_pid = Some(DEAD_PID);
    let probe = WorktreeGitProbe {
        untracked_records_only: true,
        ..dirty_probe()
    };
    let rows = rows_for(&[l.clone()], &[], probe);
    assert_eq!(
        rows[0].dispatch.as_ref().unwrap().state,
        DispatchState::Stalled,
        "root-level scratch files on main are not work at risk"
    );

    // Control: a tracked change on main stays salvageable (inspect-only hint).
    let rows = rows_for(&[l], &[], dirty_probe());
    assert_eq!(
        rows[0].dispatch.as_ref().unwrap().state,
        DispatchState::Salvageable
    );
}

#[test]
fn bug_1941_untracked_only_on_feature_branch_stays_salvageable() {
    let tmp = tempfile::tempdir().unwrap();
    let mut l = lease("feat", "TASK-1", tmp.path().to_path_buf(), "task-1-work");
    l.active_pid = Some(DEAD_PID);
    let probe = WorktreeGitProbe {
        untracked_records_only: true,
        ..dirty_probe()
    };
    let rows = rows_for(&[l], &[], probe);
    assert_eq!(
        rows[0].dispatch.as_ref().unwrap().state,
        DispatchState::Salvageable,
        "new files in a spec worktree are session work"
    );
}

#[test]
fn bug_1941_pure_salvage_rules() {
    assert_eq!(pid_liveness_with_occupant(Some(false), true), Some(true));
    assert_eq!(pid_liveness_with_occupant(Some(false), false), Some(false));
    assert_eq!(pid_liveness_with_occupant(None, true), None);
    assert_eq!(
        apply_main_checkout_untracked(DispatchState::Salvageable, true, true),
        DispatchState::Stalled
    );
    assert_eq!(
        apply_main_checkout_untracked(DispatchState::Salvageable, false, true),
        DispatchState::Salvageable
    );
    assert_eq!(
        apply_main_checkout_untracked(DispatchState::Salvageable, true, false),
        DispatchState::Salvageable
    );
    let rec = |xy: &str| (xy.to_string(), PathBuf::from("ci.log"));
    assert!(records_are_untracked_only(&[rec("??"), rec("??")]));
    assert!(!records_are_untracked_only(&[rec("??"), rec(" M")]));
    assert!(!records_are_untracked_only(&[]));
}

// ── (b) ────────────────────────────────────────────────────────────────

#[test]
fn bug_1941_started_and_elapsed_share_the_live_process_anchor() {
    let now = chrono::Utc::now();
    let lease_born = now - chrono::Duration::hours(2551);
    let pid_up = now - chrono::Duration::hours(4);
    let anchor = ps_session_started_at(lease_born, Some(pid_up));
    assert_eq!(anchor, pid_up, "adopted lease: started is the live process");
    assert_eq!(
        ps_elapsed_secs(lease_born, Some(pid_up), now),
        now.signed_duration_since(anchor).num_seconds() as u64,
        "elapsed is measured from the same anchor"
    );
    // Not adopted (pid born with the lease) and no pid: the lease's start.
    assert_eq!(ps_session_started_at(lease_born, None), lease_born);
    let with_lease = lease_born + chrono::Duration::seconds(10);
    assert_eq!(
        ps_session_started_at(lease_born, Some(with_lease)),
        lease_born
    );
    // The lease age survives in the note only.
    let note = ps_adopted_note(lease_born, Some(pid_up), now).unwrap();
    assert!(note.contains("lease born 2551h"), "{note}");
}

// ── (c) ────────────────────────────────────────────────────────────────

#[test]
fn bug_1941_backing_pid_is_deterministic_across_probe_orders() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let sub = root.join("sub");
    let a = vec![live_at(300, root), live_at(100, &sub), live_at(200, root)];
    let mut b = a.clone();
    b.reverse();
    let pick = |v: &[process_probe::LiveSession]| ps_live_in_worktree(v, root).map(|s| s.pid);
    assert_eq!(
        pick(&a),
        Some(200),
        "exact cwd beats subdirectory, then lowest pid"
    );
    assert_eq!(pick(&a), pick(&b));

    let l = main_checkout_lease("legacy", root);
    let row_a = rows_for(std::slice::from_ref(&l), &a, WorktreeGitProbe::default());
    let row_b = rows_for(&[l], &b, WorktreeGitProbe::default());
    assert_eq!(row_a[0].pid, row_b[0].pid);
}

#[test]
fn bug_1941_main_checkout_placeholder_lease_is_not_a_subagent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let l = main_checkout_lease("legacy", root);
    assert!(ps_lease_is_main_checkout(&l));
    let rows = rows_for(
        &[l],
        &[live_at(std::process::id(), root)],
        Default::default(),
    );
    assert_ne!(rows[0].role.as_deref(), Some(PS_SUBAGENT_ROLE));
    // The shared-cwd transcript scan is not trusted there.
    assert_eq!(rows[0].role, None);

    let harness = Some(tail_cmd::HARNESS_AGENT_TYPE);
    assert_eq!(
        ps_row_role(harness, Some("advisor"), None, true).as_deref(),
        Some("advisor"),
        "a manifest-joined role is still shown"
    );
    // An isolation-worktree fan-out lease keeps BUG-1681's subagent label.
    assert_eq!(
        ps_row_role(harness, None, Some("advisor"), false).as_deref(),
        Some(PS_SUBAGENT_ROLE)
    );
    // A real recorded role is untouched on the main checkout too.
    assert_eq!(
        ps_row_role(Some("implementer"), None, None, true).as_deref(),
        Some("implementer")
    );
}

// ── (d) ────────────────────────────────────────────────────────────────

#[test]
fn bug_1941_fresh_branch_commit_is_movement_without_a_pid() {
    let fresh = DEFAULT_DIRTY_MOVEMENT_FRESH_SECS;
    assert_eq!(
        apply_commit_movement(DispatchState::Stalled, Some(false), 3, Some(8 * 60), fresh),
        DispatchState::Moving
    );
    assert_eq!(
        apply_commit_movement(DispatchState::Stalled, Some(false), 3, Some(3600), fresh),
        DispatchState::Stalled,
        "an old commit is not movement"
    );
    assert_eq!(
        apply_commit_movement(DispatchState::Stalled, Some(false), 0, Some(60), fresh),
        DispatchState::Stalled,
        "a fresh main tip (nothing ahead) is not this session's movement"
    );
    assert_eq!(
        apply_commit_movement(DispatchState::Stopped, Some(false), 3, Some(60), fresh),
        DispatchState::Stopped
    );
    assert_eq!(
        apply_commit_movement(DispatchState::Unknown, None, 3, Some(60), fresh),
        DispatchState::Unknown
    );

    let tmp = tempfile::tempdir().unwrap();
    let l = lease(
        "relay",
        "TASK-1722",
        tmp.path().to_path_buf(),
        "task-1722-work",
    );
    let probe = WorktreeGitProbe {
        ahead_of_main: 2,
        head_commit_age_secs: Some(8 * 60),
        ..Default::default()
    };
    let rows = rows_for(&[l], &[], probe);
    assert_ne!(rows[0].state, LeaseState::Live);
    let ds = rows[0].dispatch.as_ref().map(|d| d.state);
    assert_eq!(ds, Some(DispatchState::Moving));
    assert!(
        ps_unattributed_movement_note(rows[0].state, ds).is_some(),
        "the human view must carry the same moving verdict"
    );
}

#[test]
fn bug_1941_movement_note_only_for_no_process_moving_rows() {
    assert!(
        ps_unattributed_movement_note(LeaseState::Dormant, Some(DispatchState::Moving)).is_some()
    );
    assert!(
        ps_unattributed_movement_note(LeaseState::Stale, Some(DispatchState::Moving)).is_some()
    );
    assert!(ps_unattributed_movement_note(LeaseState::Live, Some(DispatchState::Moving)).is_none());
    assert!(
        ps_unattributed_movement_note(LeaseState::Dormant, Some(DispatchState::Stalled)).is_none()
    );
    assert!(ps_unattributed_movement_note(LeaseState::Dormant, None).is_none());
}

#[test]
fn bug_1941_commit_time_parse() {
    assert_eq!(
        parse_commit_time_and_subject("1000\tfix: a\tb", 1600),
        (Some("fix: a\tb".to_string()), Some(600))
    );
    assert_eq!(
        parse_commit_time_and_subject("2000\tfuture", 1000),
        (Some("future".to_string()), None),
        "a future commit time is unknown, never fresh"
    );
    assert_eq!(
        parse_commit_time_and_subject("x\ty", 1000),
        (Some("y".into()), None)
    );
}
