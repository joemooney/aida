//! TASK-1171: shell code the `aida()` wrapper evals arrives on a DEDICATED
//! CHANNEL — a marker-delimited block inside stdout — so ordinary human-facing
//! stdout is never an eval candidate.
//!
//! BUG-779 fixed the observed symptom with exit-code discipline (eval only on
//! exit 0). That leaves the shape wrong: a subcommand that SUCCEEDS while
//! printing prose to stdout still feeds the prose to `eval`. These tests pin
//! the structural fix, and — just as importantly — the version-skew contract in
//! both directions, since an operator upgrading the binary keeps the old
//! wrapper sourced in every shell they are already standing in.
// trace:TASK-1171 | ai:claude

use crate::dev_cmd::{classify_wrapper, WrapperState};
use crate::shell_eval::{EVAL_BEGIN, EVAL_BLOCK_CAP, EVAL_END};

/// Drive the real `SHELL_HELPERS` wrapper through bash with a stub `aida` on
/// PATH. `stub` is the body of the fake binary; `body` runs after the helpers
/// are sourced. Returns (stdout, stderr, exit code).
// trace:TASK-1171 | ai:claude
fn run_wrapper(stub: &str, body: &str) -> (String, String, Option<i32>) {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("aida");
    std::fs::write(&bin, stub).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let script = format!(
        "PATH='{path}':\"$PATH\"\nexport PATH\n{helpers}\n{body}\n",
        path = dir.path().display(),
        helpers = crate::dev_cmd::SHELL_HELPERS,
        body = body,
    );
    let out = std::process::Command::new("bash")
        .arg("-c")
        .arg(&script)
        .output()
        .expect("bash available");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code(),
    )
}

// ── the acceptance case: prose AND an eval directive from one subcommand ─────

/// The regression this task exists for: a subcommand that prints human prose to
/// stdout AND emits a shell directive. The prose must be DISPLAYED; only the
/// marked block may reach `eval`.
///
/// The stub's prose is deliberately hostile — an unquoted backtick and a bare
/// word — exactly the shape that used to become `command not found` noise.
// trace:TASK-1171 | ai:claude
#[test]
fn prose_on_stdout_is_displayed_never_evaled() {
    let stub = format!(
        "#!/usr/bin/env bash\n\
         printf 'Entered worktree for TASK-1171\\n'\n\
         printf 'resume it later with `aida worktree enter`\\n'\n\
         printf '%s\\n' '{begin}'\n\
         printf 'export AIDA_TASK1171_MARKER=applied\\n'\n\
         printf '%s\\n' '{end}'\n\
         exit 0\n",
        begin = EVAL_BEGIN,
        end = EVAL_END,
    );
    let (stdout, stderr, _) = run_wrapper(
        &stub,
        "aida worktree enter TASK-1171\n\
         echo \"rc:$?\"\n\
         echo \"marker:${AIDA_TASK1171_MARKER-unset}\"",
    );

    assert!(
        stdout.contains("Entered worktree for TASK-1171"),
        "prose must be displayed on stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("resume it later with `aida worktree enter`"),
        "prose must be displayed VERBATIM, backticks and all:\n{stdout}"
    );
    assert!(
        stdout.contains("marker:applied"),
        "the marked block must still mutate the shell:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("command not found"),
        "prose must never be eval'd:\n{stderr}"
    );
    assert!(
        !stdout.contains(EVAL_BEGIN) && !stdout.contains(EVAL_END),
        "the markers are protocol, not output:\n{stdout}"
    );
    assert!(stdout.contains("rc:0"), "{stdout}");
}

/// Prose AFTER the block is displayed too — the wrapper must not silently
/// swallow whatever trails the payload.
// trace:TASK-1171 | ai:claude
#[test]
fn prose_after_the_block_is_displayed() {
    let stub = format!(
        "#!/usr/bin/env bash\n\
         printf '%s\\n' '{begin}'\n\
         printf 'export AIDA_TASK1171_TAIL=set\\n'\n\
         printf '%s\\n' '{end}'\n\
         printf 'Next: aida session end\\n'\n\
         exit 0\n",
        begin = EVAL_BEGIN,
        end = EVAL_END,
    );
    let (stdout, stderr, _) = run_wrapper(
        &stub,
        "aida role enter advisor\necho \"tail:${AIDA_TASK1171_TAIL-unset}\"",
    );

    assert!(
        stdout.contains("Next: aida session end"),
        "trailing prose must be displayed:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("tail:set"), "{stdout}");
}

/// A failing subcommand still reports on stderr and propagates its status — and
/// the protocol markers are stripped from that report rather than shown to the
/// operator as noise. (BUG-779's guarantee, preserved through the redesign.)
// trace:TASK-1171 | ai:claude
#[test]
fn failure_reports_on_stderr_with_markers_stripped() {
    let stub = format!(
        "#!/usr/bin/env bash\n\
         printf 'error: \"No lease found for branch `019f8357`\"\\n'\n\
         printf '%s\\n' '{begin}'\n\
         printf 'export AIDA_TASK1171_SHOULD_NOT_APPLY=1\\n'\n\
         printf '%s\\n' '{end}'\n\
         exit 1\n",
        begin = EVAL_BEGIN,
        end = EVAL_END,
    );
    let (stdout, stderr, _) = run_wrapper(
        &stub,
        "aida session end 019f8357 --yes\n\
         echo \"rc:$?\"\n\
         echo \"leaked:${AIDA_TASK1171_SHOULD_NOT_APPLY-unset}\"",
    );

    assert!(stdout.contains("rc:1"), "status propagates:\n{stdout}");
    assert!(
        stderr.contains("No lease found for branch"),
        "the real error reaches the operator:\n{stderr}"
    );
    assert!(
        !stderr.contains(EVAL_BEGIN) && !stderr.contains(EVAL_END),
        "markers must not surface in the error report:\n{stderr}"
    );
    assert!(
        stdout.contains("leaked:unset"),
        "a failing command's payload must not be applied:\n{stdout}"
    );
    assert!(!stderr.contains("command not found"), "{stderr}");
}

// ── version skew ─────────────────────────────────────────────────────────────

/// NEW wrapper + OLD binary. A binary that predates the channel emits a bare
/// payload with no markers; the wrapper must fall back to the legacy "all of
/// stdout is shell" reading rather than dropping the payload on the floor.
// trace:TASK-1171 | ai:claude
#[test]
fn unmarked_payload_from_an_older_binary_still_evals() {
    let stub = "#!/usr/bin/env bash\n\
                echo \"# aida dev activate\"\n\
                echo \"export AIDA_TASK1171_LEGACY=applied\"\n\
                exit 0\n";
    let (stdout, stderr, _) = run_wrapper(
        stub,
        "aida dev activate\n\
         echo \"rc:$?\"\n\
         echo \"legacy:${AIDA_TASK1171_LEGACY-unset}\"",
    );

    assert!(
        stdout.contains("legacy:applied"),
        "an older binary's bare payload must still be eval'd:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("rc:0"), "{stdout}");
}

/// OLD wrapper + NEW binary. The binary only emits markers when the wrapper
/// advertises the capability, so a shell still running the pre-TASK-1171
/// wrapper gets exactly the bytes it has always consumed.
// trace:TASK-1171 | ai:claude
#[test]
fn binary_emits_a_bare_payload_to_a_stale_or_absent_wrapper() {
    // The capability list a pre-TASK-1171 wrapper exported.
    assert!(!crate::shell_eval::marker_speaks_eval_block(Some(
        "role,session,dev,worktree,worktree-exit,worktree-stale"
    )));
    // No wrapper at all — a hand-rolled `eval "$(aida role enter x)"`.
    assert!(!crate::shell_eval::marker_speaks_eval_block(None));
    // And the current wrapper does advertise it.
    assert!(crate::shell_eval::marker_speaks_eval_block(Some(
        crate::dev_cmd::SHELL_HELPERS
            .lines()
            .find_map(|l| l.strip_prefix("export AIDA_SHELL_WRAPPER="))
            .expect("the wrapper exports its capability list")
            .trim_matches('\'')
    )));
}

/// The staleness verdict `aida dev status` reports, over the three shapes a
/// caller's shell can present.
// trace:TASK-1171 | ai:claude
#[test]
fn wrapper_state_is_classified_for_the_operator() {
    assert_eq!(classify_wrapper(None), WrapperState::Absent);
    assert_eq!(
        classify_wrapper(Some("role,session,dev,worktree")),
        WrapperState::Stale
    );
    assert_eq!(
        classify_wrapper(Some("role,session,dev,worktree,eval-block")),
        WrapperState::Current
    );
}

// ── the shell and the Rust constants must not drift ──────────────────────────

/// The wrapper is generated from a Rust string constant while the markers it
/// slices on are separate Rust constants. Pin them together — a rename on
/// either side that missed the other would silently disable the channel.
// trace:TASK-1171 | ai:claude
#[test]
fn wrapper_shell_and_rust_constants_agree() {
    let helpers = crate::dev_cmd::SHELL_HELPERS;
    assert!(
        helpers.contains(EVAL_BEGIN),
        "the wrapper must slice on the same begin marker the binary emits"
    );
    assert!(
        helpers.contains(EVAL_END),
        "the wrapper must slice on the same end marker the binary emits"
    );
    let advertised = helpers
        .lines()
        .find_map(|l| l.strip_prefix("export AIDA_SHELL_WRAPPER="))
        .expect("the wrapper exports its capability list");
    assert!(
        advertised.contains(EVAL_BLOCK_CAP),
        "the wrapper must advertise the eval-block capability, else the binary \
         will keep emitting the legacy bare payload to it: {advertised}"
    );
}
