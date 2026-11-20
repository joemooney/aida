//! BUG-2428: a `queue work` launch refused before its agent starts must not
//! leave its worktree (`aida-pr-N`) or lease behind, or every later attempt on
//! that PR or spec refuses with "branch ... is held by worktree ... before
//! pickup setup". Two halves: the keyring probe runs before any setup, and a
//! launch that fails before an agent can start rolls back the setup this
//! invocation created. TASK-2807: once an agent can have run, the rollback is
//! disarmed, so neither an error nor a panic after it removes the agent's tree.
//! The behavioral half (a real setup rolled back or kept) is in
//! `tests/task_2807_pickup_launch_rollback_tests.rs`.
// trace:BUG-2428 TASK-2807 | ai:claude

use super::launch_refused_before_agent;
use crate::gate::seat_authority::{preflight_sandboxed_child_key, ChildGrantRefused};

fn handle_queue_work_source() -> &'static str {
    include_str!("../queue_cmd/work.rs")
        .split_once("pub(crate) fn handle_queue_work(")
        .expect("handle_queue_work exists")
        .1
        .split_once("\n}\n")
        .expect("handle_queue_work ends")
        .0
}

fn refused_mint() -> anyhow::Error {
    crate::grant_keyring::FORCE_INSTALL_ERRNO.with(|e| e.set(Some(libc::EDQUOT)));
    preflight_sandboxed_child_key("reviewer").unwrap_err()
}

#[test]
fn a_refused_child_grant_is_a_launch_refused_before_any_agent() {
    let err = refused_mint();
    assert!(ChildGrantRefused::is_in(&err), "{err:#}");
    // Wherever it surfaces: a grant is minted before its child spawns.
    assert!(launch_refused_before_agent(&Err(err), true));
}

/// TASK-2808: every pre-agent failure rolls back, not only a refused grant.
#[test]
fn any_failure_before_an_agent_can_start_is_a_pre_agent_refusal() {
    for why in [
        "session_start completed but no lease for scope `TASK-1` is visible",
        "failed to chdir into <worktree>",
        "failed to exec codex: No such file or directory",
        "failed to spawn codex: No such file or directory",
        "a watch launch refused: the PR head moved",
    ] {
        assert!(
            launch_refused_before_agent(&Err(anyhow::anyhow!(why)), false),
            "{why}"
        );
    }
}

#[test]
fn outcomes_after_an_agent_can_have_run_keep_the_setup() {
    assert!(!launch_refused_before_agent(&Ok(()), false));
    assert!(!launch_refused_before_agent(&Ok(()), true));
    assert!(!launch_refused_before_agent(
        &Err(anyhow::anyhow!("the reviewer exited without a verdict")),
        true
    ));
}

#[test]
fn the_keyring_probe_runs_before_the_worktree_and_lease_exist() {
    let work = handle_queue_work_source();
    let setup = work
        .find("crate::session_start::session_start_with_receipt(")
        .expect("setup boundary");
    let reviewer_probe = work
        .find("refuse_unpresentable_contained_reviewer(contained)?")
        .expect("reviewer keyring probe");
    let child_probe = work
        .find("crate::gate::seat_authority::preflight_sandboxed_child_key(&child_seat)?")
        .expect("contained child keyring probe");
    assert!(reviewer_probe < setup, "reviewer probe must precede setup");
    assert!(child_probe < setup, "child probe must precede setup");
}

#[test]
fn a_launching_pickup_runs_its_launch_under_the_rollback() {
    let work = handle_queue_work_source();
    let setup = work
        .find("crate::session_start::session_start_with_receipt(")
        .expect("setup boundary");
    assert!(
        work[setup..]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("/* rollback_failed_setup */ !no_launch,"),
        "a launching pickup arms the setup rollback"
    );
    let launch = work
        .find("run_pickup_launch(&project_root, setup_receipt,")
        .expect("the launch runs under run_pickup_launch");
    assert!(setup < launch);
    assert!(
        !work.contains("setup_receipt.disarm()"),
        "only run_pickup_launch disarms the setup"
    );
}

/// TASK-2807: exec-style headless launch is marked first. Standalone,
/// resume and reviewer runners mark from successful spawn callbacks, and
/// the bare execs are not marked (a failed exec launched nobody).
#[test]
fn every_spawning_launch_is_marked_and_no_exec_is() {
    let work = handle_queue_work_source();
    let start = work
        .find("let launch_agent = |agent: &AgentLaunchMark|")
        .expect("the launch closure");
    let end = work
        .find("run_pickup_launch(&project_root, setup_receipt, launch_agent)")
        .expect("the launch runs under run_pickup_launch");
    let body = &work[start..end];
    for runner in ["session::exec_vendor_headless("] {
        let at = body.find(runner).unwrap_or_else(|| panic!("{runner}"));
        let marked = body[..at].rfind("agent.starting();").expect("a mark");
        assert!(
            !body[marked..at].contains('}'),
            "{runner} is marked in its own block"
        );
    }
    // The standalone implementer marks from its actual spawn hook, too.
    // trace:STORY-2690 | ai:codex
    assert!(body.contains("Some(&|| agent.starting())"));
    let standalone = include_str!("../standalone_implementer.rs");
    assert!(standalone.contains("if let Some(on_started) = on_started"));
    // Counted at the spawn, not pre-marked: a failed spawn rolls back.
    let resume = body
        .find("run_queue_resume_to_exit(")
        .expect("the implementer resume");
    assert!(
        !body[..resume].contains("agent.starting();"),
        "standalone and resume arms mark only from actual spawn callbacks"
    );
    // The publication wrapper must leave counting to the actual spawn.
    // trace:STORY-2475 TASK-2807 | ai:codex
    let resume_wrapper = include_str!("../queue_cmd/work.rs")
        .split_once("pub(crate) fn run_queue_resume_to_exit(")
        .expect("resume publication wrapper exists")
        .1
        .split_once("\n}\n")
        .expect("resume publication wrapper ends")
        .0;
    assert!(resume_wrapper.contains("session::spawn_vendor_headless_resume("));
    assert!(
        !resume_wrapper.contains("agent.starting()"),
        "the resume wrapper must leave counting to the spawn"
    );
    let reviewer = include_str!("../queue_cmd/work.rs")
        .split_once("pub(crate) fn run_standalone_reviewer(")
        .expect("run_standalone_reviewer exists")
        .1
        .split_once("\n}\n")
        .expect("run_standalone_reviewer ends")
        .0;
    assert!(
        !reviewer.contains("agent.starting()"),
        "the reviewer is counted at its spawn"
    );
    for exec in [
        "session::exec_codex_resume(",
        "session::exec_codex_session(",
        "session::build_claude_resume_exec_command(",
        "session::build_claude_exec_command(",
    ] {
        let at = body.find(exec).unwrap_or_else(|| panic!("{exec}"));
        let before = &body[..at];
        if let Some(last_mark) = before.rfind("agent.starting();") {
            assert!(
                before[last_mark..].contains('}'),
                "{exec} is not marked: a failed exec launched nobody"
            );
        }
    }
}
