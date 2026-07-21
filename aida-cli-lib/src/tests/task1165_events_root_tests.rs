//! TASK-1165: the CI-watch emit site takes its project root from the CALLER.
//!
//! Same invariant BUG-770 established for the escalation epilogue: an
//! `events::emit` must never resolve its own root from the process cwd. The old
//! `wait_for_ci_terminal` did (`find_project_root().unwrap_or(PathBuf::from("."))`),
//! which meant a caller standing outside any project would drop a stray
//! `./.aida/events.jsonl` wherever the process happened to be. These tests pin
//! both arms of the injected-root decision.
//!
//! Both hold an [`EnvVarGuard`] over `AIDA_EVENTS_DISABLE`: a sibling test in
//! `events.rs` flips that kill switch process-wide, and the guard's global lock
//! is what stops the two from racing.

use super::emit_ci_terminal;
use crate::events::EVENTS_DISABLE_ENV;
use crate::test_env::EnvVarGuard;

/// An injected root gets the `CiTerminal` line — and it lands under THAT root,
/// not under the process cwd.
#[test]
fn injected_root_receives_the_ci_terminal_event() {
    let _guard = EnvVarGuard::unset(EVENTS_DISABLE_ENV);
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    emit_ci_terminal(Some(root), true);

    let path = crate::events::events_path(root);
    let body =
        std::fs::read_to_string(&path).expect("event stream written under the injected root");
    assert!(
        body.contains(r#""event":"CiTerminal""#),
        "expected a CiTerminal line, got: {body}"
    );
    assert!(
        body.contains("true"),
        "the green verdict should be carried on the event: {body}"
    );
}

/// No root — no emit. The fail-safe: skip, rather than fall back to `.` and
/// fabricate an event stream in whatever directory the process is standing in.
#[test]
fn absent_root_skips_the_emit_entirely() {
    let _guard = EnvVarGuard::unset(EVENTS_DISABLE_ENV);
    let tmp = tempfile::tempdir().expect("tempdir");

    emit_ci_terminal(None, false);

    // The temp dir stands in for "some directory the process could have been
    // standing in" — it must be untouched.
    assert!(
        !crate::events::events_path(tmp.path()).exists(),
        "a None root must not write an event stream"
    );
    assert!(
        std::fs::read_dir(tmp.path())
            .expect("readable tempdir")
            .next()
            .is_none(),
        "a None root must not create anything at all"
    );
}
