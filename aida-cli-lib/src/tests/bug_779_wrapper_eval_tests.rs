//! BUG-779: the `aida()` shell wrapper eval'd an eval-list subcommand's stdout
//! unconditionally. When the command FAILED, stdout carried its human-readable
//! error text (the wrapper's own capture makes stdout a pipe, which is what
//! routes the message there) — so every failure became a burst of
//! `command not found` and the real message was invisible.
//!
//! Two halves, both covered here:
//!   1. a failing eval-list subcommand exits NON-ZERO
//!   2. the wrapper evals ONLY on exit 0, printing the output to stderr and
//!      returning the status otherwise — while benign no-op successes (exit 0,
//!      empty stdout) and real shell payloads keep working.
// trace:BUG-779 | ai:claude

use crate::session_end_explicit_target;

// ── half 1: the exit code ────────────────────────────────────────────────────

/// A bare `aida session end` with zero active leases is a genuine no-op: there
/// is nothing to end, and reporting failure would be wrong (the wrapper would
/// stop eval'ing a payload that is legitimately empty).
// trace:BUG-779 | ai:claude
#[test]
fn bare_session_end_names_no_target() {
    assert_eq!(session_end_explicit_target(None, None, None), None);
    // Blank strings are not a target either — an empty `--spec ""` is noise.
    assert_eq!(
        session_end_explicit_target(Some("   "), Some(""), None),
        None
    );
}

/// An EXPLICIT target — positional id, `--spec`, or `--branch` — that cannot
/// resolve is a failed request, so the caller must be able to distinguish it
/// from the no-op and exit non-zero.
// trace:BUG-779 | ai:claude
#[test]
fn explicit_session_end_target_is_reported() {
    assert_eq!(
        session_end_explicit_target(Some("019f8357"), None, None).as_deref(),
        Some("019f8357")
    );
    assert_eq!(
        session_end_explicit_target(None, Some("BUG-779"), None).as_deref(),
        Some("BUG-779")
    );
    assert_eq!(
        session_end_explicit_target(None, None, Some("bug-779-work")).as_deref(),
        Some("bug-779-work")
    );
    // First non-blank wins, mirroring the resolution precedence.
    assert_eq!(
        session_end_explicit_target(Some(" 019f8357 "), Some("BUG-779"), None).as_deref(),
        Some("019f8357")
    );
}

// ── half 2: the wrapper ──────────────────────────────────────────────────────

/// Drive the real `SHELL_HELPERS` wrapper through bash with a stub `aida` on
/// PATH, so the eval-vs-error decision is exercised as shell rather than
/// eyeballed. `stub` is the body of the fake binary; `body` runs after the
/// helpers are sourced.
///
/// These assertions are about the exit-code discipline, which is shell-agnostic
/// — the multi-shell matrix (bash + zsh) lives with the eval-channel tests.
// trace:BUG-779 | ai:claude
// trace:TASK-1174 | ai:claude
fn run_wrapper(stub: &str, body: &str) -> (String, String, Option<i32>) {
    crate::shell_wrapper_harness::run_wrapper_in("bash", stub, body)
}

/// The regression itself: a failing `session end` whose error text lands on
/// stdout must NOT be eval'd. The message reaches the operator on stderr, the
/// backticked lease id is never run as a command, and the status propagates.
// trace:BUG-779 | ai:claude
#[test]
fn failing_eval_list_subcommand_is_not_evaled() {
    let stub = "#!/usr/bin/env bash\n\
                printf 'error: \"No lease found for branch `019f8357`\"\\nhelp: aida session leases\\n'\n\
                exit 1\n";
    let (stdout, stderr, code) =
        run_wrapper(stub, "aida session end 019f8357 --yes\necho \"rc:$?\"");

    assert!(
        stdout.contains("rc:1"),
        "wrapper returns the failure status; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("No lease found for branch"),
        "the real error reaches the operator on stderr:\n{stderr}"
    );
    // The pre-fix symptom: eval'ing the error text ran the backticked lease id
    // and the `error:` / `help:` words as commands.
    assert!(
        !stderr.contains("command not found"),
        "error text must not be eval'd as shell:\n{stderr}"
    );
    assert_eq!(code, Some(0), "the outer script itself completes");
}

/// The success half must be untouched: exit 0 still evals the payload into the
/// calling shell, so `role enter` / `dev activate` / `worktree enter` keep
/// mutating it.
// trace:BUG-779 | ai:claude
#[test]
fn successful_eval_list_subcommand_still_evals_its_payload() {
    let stub = "#!/usr/bin/env bash\n\
                echo \"# aida role enter\"\n\
                echo \"export AIDA_BUG779_MARKER=applied\"\n\
                exit 0\n";
    let (stdout, stderr, _) = run_wrapper(
        stub,
        "aida role enter advisor\n\
         echo \"rc:$?\"\n\
         echo \"marker:${AIDA_BUG779_MARKER-unset}\"",
    );

    assert!(
        stdout.contains("marker:applied"),
        "exit 0 still evals the shell payload; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("rc:0"), "{stdout}");
}

/// A benign no-op success — `worktree exit` outside a scoped worktree reports
/// on stderr, emits no shell, and exits 0 — stays a success: the wrapper evals
/// the (empty) payload and returns 0 rather than treating it as a failure.
// trace:BUG-779 | ai:claude
#[test]
fn benign_noop_success_stays_exit_zero() {
    let stub = "#!/usr/bin/env bash\n\
                echo 'Not inside a scoped worktree. Nothing to step out of.' >&2\n\
                exit 0\n";
    let (stdout, stderr, _) = run_wrapper(stub, "aida worktree exit\necho \"rc:$?\"");

    assert!(
        stdout.contains("rc:0"),
        "a benign no-op is success; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stderr.contains("Nothing to step out of"), "{stderr}");
}

/// Every subcommand the wrapper auto-evals must go through the guarded branch —
/// a new verb added to the case list without the guard would reintroduce the
/// bug for that verb.
// trace:BUG-779 | ai:claude
#[test]
fn every_eval_list_subcommand_is_guarded() {
    let stub = "#!/usr/bin/env bash\n\
                echo \"error: \\\"boom\\\"\"\n\
                exit 3\n";
    for cmd in [
        "dev activate",
        "dev deactivate",
        "role enter x",
        "role end",
        "role add x",
        "session start",
        "session end x",
        "worktree enter x",
        "worktree exit",
    ] {
        let (stdout, stderr, _) = run_wrapper(stub, &format!("aida {cmd}\necho \"rc:$?\""));
        assert!(
            stdout.contains("rc:3"),
            "`aida {cmd}` must propagate the failure status; stdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            !stderr.contains("command not found"),
            "`aida {cmd}` must not eval its error text:\n{stderr}"
        );
    }
}

/// Source-level guard: the unconditional `eval "$(command aida ...)"` that
/// caused this bug must not come back.
// trace:BUG-779 | ai:claude
#[test]
fn wrapper_has_no_unconditional_eval() {
    let helpers = crate::dev_cmd::SHELL_HELPERS;
    assert!(
        !helpers.contains("eval \"$(command aida \"$@\")\""),
        "the eval-list branch must capture the output and check the status first"
    );
    assert!(
        helpers.contains("_aida_rc"),
        "the eval-list branch must record and branch on the exit status"
    );
}
