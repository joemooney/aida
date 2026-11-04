//! BUG-2428: a `queue work` launch refused before its agent starts must not
//! leave its worktree (`aida-pr-N`) or lease behind, or every later attempt on
//! that PR or spec refuses with "branch ... is held by worktree ... before
//! pickup setup". Two halves: the keyring probe runs before any setup, and a
//! refusal at child-grant mint rolls back the setup this invocation created.
// trace:BUG-2428 | ai:claude

use super::launch_refused_before_agent;
use crate::gate::seat_authority::{preflight_sandboxed_child_key, ChildGrantRefused};

fn handle_queue_work_source() -> &'static str {
    include_str!("../queue_cmd.rs")
        .split_once("pub(crate) fn handle_queue_work(")
        .expect("handle_queue_work exists")
        .1
}

#[test]
fn a_refused_child_grant_is_a_launch_refused_before_any_agent() {
    crate::grant_keyring::FORCE_INSTALL_ERRNO.with(|e| e.set(Some(libc::EDQUOT)));
    let err = preflight_sandboxed_child_key("reviewer").unwrap_err();
    assert!(ChildGrantRefused::is_in(&err), "{err:#}");
    assert!(launch_refused_before_agent(&Err(err)));
}

#[test]
fn other_outcomes_keep_the_setup() {
    assert!(!launch_refused_before_agent(&Ok(())));
    assert!(!launch_refused_before_agent(&Err(anyhow::anyhow!(
        "the reviewer exited without a verdict"
    ))));
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
fn a_launching_pickup_arms_rollback_and_drops_it_only_on_refusal() {
    let work = handle_queue_work_source();
    let setup = work
        .find("crate::session_start::session_start_with_receipt(")
        .expect("setup boundary");
    assert!(
        work[setup..].contains("/* rollback_failed_setup */\n            !no_launch,"),
        "a launching pickup arms the setup rollback"
    );
    let rollback = work
        .find("if launch_refused_before_agent(&setup_result) && setup_receipt.owns_setup()")
        .expect("refusal rolls back the owned setup");
    let disarm = work.find("setup_receipt.disarm();").expect("disarm");
    assert!(setup < rollback && rollback < disarm);
}
