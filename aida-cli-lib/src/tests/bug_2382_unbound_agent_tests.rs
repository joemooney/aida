//! BUG-2382: `aida ps` called a live hand-started Codex session dead and
//! salvageable, because the lease records only the exited `aida session
//! start` pid and the live-session probe saw claude processes only.
//! (1) a codex session in the clone makes the row live and not salvageable;
//! (2) any other live process in the clone turns the salvage recipe into an
//!     "unbound agent in clone" note naming the pid;
//! (3) with nothing running the salvageable output is unchanged.
//
// trace:BUG-2382 | ai:claude

use super::*;
use crate::dispatch_health_ps::{DispatchState, WorktreeGitProbe};
use aida_core::liveness::{
    agent_sessions_from_agent_processes, AgentKind, CwdOccupant, LiveAgentProcess,
};
use std::path::{Path, PathBuf};

/// A pid that is never alive on a test host.
const DEAD_PID: u32 = 999_999_999;

/// A lease as `aida session start --owns` writes it for a hand-started
/// agent: only the (now exited) creator pid, no active pid.
fn hand_started_lease(worktree: PathBuf) -> SessionLease {
    SessionLease {
        tree_kind: Default::default(),
        id: "01a194b1bcfb".into(),
        scope: "TASK-2631".into(),
        slug: "task-2631".into(),
        owner: "tester".into(),
        worktree_path: worktree,
        branch: "task-2631".into(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: Some(DEAD_PID),
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

fn rows_for(leases: &[SessionLease], live: &[process_probe::LiveSession]) -> Vec<PsRow> {
    build_running_work(
        &[],
        leases,
        live,
        chrono::Utc::now(),
        |_| WorktreeGitProbe {
            dirty: true,
            last_commit_subject: Some("wip".into()),
            ..Default::default()
        },
        |_| None,
        |_| None,
        |_| None,
        |_| None,
        |_| MailIdentityStatus::Unknown,
        |_, _| SeatActivity::Unknown,
    )
    .0
}

/// A real, live process that is not this test process (the inspector is
/// excluded from attribution for a lease with no durable owner).
fn spawn_sleeper(cwd: &Path) -> std::process::Child {
    std::process::Command::new("sleep")
        .arg("30")
        .current_dir(cwd)
        .spawn()
        .expect("spawn `sleep 30`")
}

fn codex_process(pid: u32, cwd: &Path) -> LiveAgentProcess {
    LiveAgentProcess {
        pid,
        agent: AgentKind::Codex,
        cwd: cwd.to_path_buf(),
        stale_cwd: false,
        start_time: None,
        tty: None,
    }
}

#[test]
fn bug_2382_codex_processes_are_live_sessions() {
    let cwd = PathBuf::from("/tmp/aida-seat-clone");
    let live = agent_sessions_from_agent_processes(&[codex_process(4242, &cwd)]);
    assert_eq!(live.len(), 1, "a codex process is a live agent session");
    assert_eq!(live[0].pid, 4242);
    assert_eq!(live[0].cwd, cwd);
    assert!(live[0].jsonl.is_none(), "codex has no claude transcript");
}

#[cfg(unix)]
#[test]
fn bug_2382_exited_creator_plus_live_codex_in_clone_reads_live() {
    let tmp = tempfile::tempdir().unwrap();
    let clone = tmp.path().to_path_buf();
    let mut child = spawn_sleeper(&clone);
    let live = agent_sessions_from_agent_processes(&[codex_process(child.id(), &clone)]);
    let rows = rows_for(&[hand_started_lease(clone)], &live);
    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(
        rows[0].state,
        LeaseState::Live,
        "the codex in the clone is the agent"
    );
    assert_eq!(rows[0].pid, Some(child.id()), "the row names the codex pid");
    let d = rows[0].dispatch.as_ref().unwrap();
    assert_ne!(d.state, DispatchState::Salvageable);
    let hint = d.hint.as_deref().unwrap_or("");
    assert!(!hint.contains("salvage"), "{hint}");
    assert!(!hint.contains("dead process"), "{hint}");
}

#[test]
fn bug_2382_no_agent_in_clone_keeps_salvageable_output() {
    let tmp = tempfile::tempdir().unwrap();
    let mut rows = rows_for(&[hand_started_lease(tmp.path().to_path_buf())], &[]);
    assert_ne!(rows[0].state, LeaseState::Live);
    let mut d = rows.remove(0).dispatch.unwrap();
    assert_eq!(d.state, DispatchState::Salvageable);
    let hint = d.hint.as_deref().unwrap_or("");
    assert!(hint.contains("dead process, uncommitted work"), "{hint}");
    assert!(hint.contains("salvage-commit"), "{hint}");

    // The backstop with no occupant leaves the row exactly as it was.
    let before = d.hint.clone();
    apply_unbound_occupant(&mut d, tmp.path(), None);
    assert_eq!(d.state, DispatchState::Salvageable);
    assert_eq!(d.hint, before);
}

#[test]
fn bug_2382_unbound_process_in_clone_withholds_salvage_and_names_pid() {
    let tmp = tempfile::tempdir().unwrap();
    let mut rows = rows_for(&[hand_started_lease(tmp.path().to_path_buf())], &[]);
    let mut d = rows.remove(0).dispatch.unwrap();
    assert_eq!(d.state, DispatchState::Salvageable);
    let occ = CwdOccupant {
        pid: 1_271_645,
        name: "claude".into(),
        agent: Some(AgentKind::Claude),
    };
    apply_unbound_occupant(&mut d, tmp.path(), Some(&occ));
    assert_eq!(d.state, DispatchState::Unknown);
    let hint = d.hint.as_deref().unwrap();
    assert!(hint.contains("unbound agent in clone"), "{hint}");
    assert!(hint.contains("pid 1271645"), "{hint}");
    assert!(hint.contains("claude agent"), "{hint}");
    assert!(!hint.contains("git -C"), "no salvage recipe: {hint}");
    assert!(!hint.contains("aida queue work"), "no re-dispatch: {hint}");
}

#[test]
fn bug_2382_unbound_occupant_leaves_other_states_alone() {
    let mut d = PsDispatch {
        state: DispatchState::Stalled,
        hint: Some("stalled".into()),
        dirty: false,
        ahead_of_main: 0,
        untracked_only: false,
        commit_movement_from: None,
    };
    let occ = CwdOccupant {
        pid: 7,
        name: "bash".into(),
        agent: None,
    };
    apply_unbound_occupant(&mut d, Path::new("/x"), Some(&occ));
    assert_eq!(d.state, DispatchState::Stalled);
    assert_eq!(d.hint.as_deref(), Some("stalled"));
}

#[cfg(target_os = "linux")]
#[test]
fn bug_2382_cwd_scan_finds_any_process_in_the_clone() {
    let tmp = tempfile::tempdir().unwrap();
    let sub = tmp.path().join("crate");
    std::fs::create_dir(&sub).unwrap();
    let mut child = spawn_sleeper(&sub);
    let none = std::collections::HashSet::new();
    let found = process_probe::live_process_with_cwd_in(tmp.path(), &none);
    let excluded: std::collections::HashSet<u32> = [child.id()].into_iter().collect();
    let after_exclude = process_probe::live_process_with_cwd_in(tmp.path(), &excluded);
    let _ = child.kill();
    let _ = child.wait();

    let found = found.expect("the sleeper's cwd is inside the clone");
    assert_eq!(found.pid, child.id());
    assert_eq!(found.name, "sleep");
    assert_eq!(found.agent, None);
    assert!(
        after_exclude.is_none(),
        "an excluded pid is not an occupant"
    );
}
