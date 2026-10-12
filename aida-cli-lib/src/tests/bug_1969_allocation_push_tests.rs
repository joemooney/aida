//! BUG-1969: `push_store_after_id_allocation` fails only for positively
//! identified persistent push causes (permission, protected ref, server
//! hook), with the real git message and the recovery hint; an unreachable
//! origin and any unclassified failure still defer.
// trace:BUG-1969 | ai:claude

use super::{allocation_push_step, AllocationPushStep};
use aida_core::git_ops::{PushFailure, PushOutcome};

fn failed(kind: PushFailure, stderr: &str) -> anyhow::Result<PushOutcome> {
    Ok(PushOutcome::Failed {
        kind,
        stderr: stderr.to_string(),
    })
}

fn fail_message(step: AllocationPushStep) -> String {
    match step {
        AllocationPushStep::Fail(message) => message,
        other => panic!("expected Fail, got {other:?}"),
    }
}

#[test]
fn pushed_publishes() {
    assert_eq!(
        allocation_push_step(Ok(PushOutcome::Pushed), "TASK-9", "aida-store"),
        AllocationPushStep::Published
    );
}

#[test]
fn non_fast_forward_still_retries() {
    let step = allocation_push_step(
        failed(
            PushFailure::NonFastForward,
            " ! [rejected] x -> aida-store (fetch first)",
        ),
        "TASK-9",
        "aida-store",
    );
    assert_eq!(step, AllocationPushStep::Retry);
}

#[test]
fn unreachable_origin_still_defers_as_offline() {
    let step = allocation_push_step(
        failed(
            PushFailure::Unreachable,
            "ssh: Could not resolve hostname github.com: Temporary failure in name resolution",
        ),
        "TASK-9",
        "aida-store",
    );
    assert_eq!(step, AllocationPushStep::Defer { detail: None });
}

#[test]
fn permission_denied_fails_with_git_message() {
    let msg = fail_message(allocation_push_step(
        failed(
            PushFailure::PermissionDenied,
            "git@github.com: Permission denied (publickey).",
        ),
        "TASK-9",
        "aida-store",
    ));
    assert!(
        msg.contains("missing credentials or no write permission"),
        "{msg}"
    );
    assert!(msg.contains("Permission denied (publickey)"), "{msg}");
    assert!(msg.contains("Retrying will not help"), "{msg}");
    assert!(msg.contains("aida db sync --push"), "{msg}");
    assert!(!msg.contains("unreachable"), "{msg}");
}

#[test]
fn protected_ref_fails_naming_the_branch() {
    let msg = fail_message(allocation_push_step(
        failed(
            PushFailure::ProtectedRef,
            "remote: error: GH006: Protected branch update failed",
        ),
        "TASK-9",
        "aida-store",
    ));
    assert!(msg.contains("branch `aida-store` is protected"), "{msg}");
    assert!(msg.contains("GH006"), "{msg}");
    assert!(msg.contains("Retrying will not help"), "{msg}");
    assert!(!msg.contains("unreachable"), "{msg}");
}

#[test]
fn hook_rejection_fails_with_hook_output() {
    let msg = fail_message(allocation_push_step(
        failed(
            PushFailure::RemoteDeclined,
            "remote: policy says no\n ! [remote rejected] x -> aida-store (pre-receive hook declined)",
        ),
        "TASK-9",
        "aida-store",
    ));
    assert!(
        msg.contains("server-side hook or policy declined it"),
        "{msg}"
    );
    assert!(msg.contains("policy says no"), "{msg}");
    assert!(!msg.contains("unreachable"), "{msg}");
}

#[test]
fn unclassified_failure_defers_with_git_message() {
    // Round-1 review: proxy / 5xx / dropped-ssh wording git may grow must not
    // turn offline filing into a failed `aida add`.
    let step = allocation_push_step(
        failed(PushFailure::Other, "error: failed to push some refs"),
        "TASK-9",
        "aida-store",
    );
    assert_eq!(
        step,
        AllocationPushStep::Defer {
            detail: Some("error: failed to push some refs".to_string())
        }
    );
}

#[test]
fn push_that_cannot_run_fails_instead_of_deferring() {
    let msg = fail_message(allocation_push_step(
        Err(anyhow::anyhow!("Failed to run: git push")),
        "TASK-9",
        "aida-store",
    ));
    assert!(msg.contains("the store push could not run"), "{msg}");
    assert!(msg.contains("Failed to run: git push"), "{msg}");
    assert!(!msg.contains("Retrying will not help"), "{msg}");
}
