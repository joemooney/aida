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
//!
//! TASK-1174: every shell-driving test here runs over the SHELL MATRIX
//! (`wrapper_shells()`) rather than hardcoding bash, because the wrapper is
//! installed into `~/.zshrc` too. zsh joins the matrix when the runner has it
//! and is cleanly skipped when it does not; a block-slicing divergence between
//! the shells fails the suite here instead of shipping as a broken shell.
// trace:TASK-1171 | ai:claude
// trace:TASK-1174 | ai:claude

use crate::dev_cmd::{classify_wrapper, WrapperState};
use crate::shell_eval::{EVAL_BEGIN, EVAL_BLOCK_CAP, EVAL_END};
use crate::shell_wrapper_harness::{run_wrapper_in, wrapper_shells};

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
    for &shell in wrapper_shells() {
        let (stdout, stderr, _) = run_wrapper_in(
            shell,
            &prose_and_payload_stub(),
            "aida worktree enter TASK-1171\n\
             echo \"rc:$?\"\n\
             echo \"marker:${AIDA_TASK1171_MARKER-unset}\"",
        );

        assert!(
            stdout.contains("Entered worktree for TASK-1171"),
            "[{shell}] prose must be displayed on stdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            stdout.contains("resume it later with `aida worktree enter`"),
            "[{shell}] prose must be displayed VERBATIM, backticks and all:\n{stdout}"
        );
        assert!(
            stdout.contains("marker:applied"),
            "[{shell}] the marked block must still mutate the shell:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            !stderr.contains("command not found"),
            "[{shell}] prose must never be eval'd:\n{stderr}"
        );
        assert!(
            !stdout.contains(EVAL_BEGIN) && !stdout.contains(EVAL_END),
            "[{shell}] the markers are protocol, not output:\n{stdout}"
        );
        assert!(stdout.contains("rc:0"), "[{shell}] {stdout}");
    }
}

/// Prose AFTER the block is displayed too — the wrapper must not silently
/// swallow whatever trails the payload.
// trace:TASK-1171 | ai:claude
#[test]
fn prose_after_the_block_is_displayed() {
    for &shell in wrapper_shells() {
        let (stdout, stderr, _) = run_wrapper_in(
            shell,
            &trailing_prose_stub(),
            "aida role enter advisor\necho \"tail:${AIDA_TASK1171_TAIL-unset}\"",
        );

        assert!(
            stdout.contains("Next: aida session end"),
            "[{shell}] trailing prose must be displayed:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(stdout.contains("tail:set"), "[{shell}] {stdout}");
    }
}

/// A failing subcommand still reports on stderr and propagates its status — and
/// the protocol markers are stripped from that report rather than shown to the
/// operator as noise. (BUG-779's guarantee, preserved through the redesign.)
// trace:TASK-1171 | ai:claude
#[test]
fn failure_reports_on_stderr_with_markers_stripped() {
    for &shell in wrapper_shells() {
        let (stdout, stderr, _) = run_wrapper_in(
            shell,
            &failing_stub(),
            "aida session end 019f8357 --yes\n\
             echo \"rc:$?\"\n\
             echo \"leaked:${AIDA_TASK1171_SHOULD_NOT_APPLY-unset}\"",
        );

        assert!(
            stdout.contains("rc:1"),
            "[{shell}] status propagates:\n{stdout}"
        );
        assert!(
            stderr.contains("No lease found for branch"),
            "[{shell}] the real error reaches the operator:\n{stderr}"
        );
        assert!(
            !stderr.contains(EVAL_BEGIN) && !stderr.contains(EVAL_END),
            "[{shell}] markers must not surface in the error report:\n{stderr}"
        );
        assert!(
            stdout.contains("leaked:unset"),
            "[{shell}] a failing command's payload must not be applied:\n{stdout}"
        );
        assert!(!stderr.contains("command not found"), "[{shell}] {stderr}");
    }
}

// ── version skew ─────────────────────────────────────────────────────────────

/// NEW wrapper + OLD binary. A binary that predates the channel emits a bare
/// payload with no markers; the wrapper must fall back to the legacy "all of
/// stdout is shell" reading rather than dropping the payload on the floor.
// trace:TASK-1171 | ai:claude
#[test]
fn unmarked_payload_from_an_older_binary_still_evals() {
    for &shell in wrapper_shells() {
        let (stdout, stderr, _) = run_wrapper_in(
            shell,
            LEGACY_STUB,
            "aida dev activate\n\
             echo \"rc:$?\"\n\
             echo \"legacy:${AIDA_TASK1171_LEGACY-unset}\"",
        );

        assert!(
            stdout.contains("legacy:applied"),
            "[{shell}] an older binary's bare payload must still be eval'd:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(stdout.contains("rc:0"), "[{shell}] {stdout}");
    }
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

// ── TASK-1174: bash and zsh must not diverge ─────────────────────────────────

/// The matrix contract itself. bash is always exercised; zsh is exercised
/// exactly when the machine can run it, and its absence is a SKIP rather than
/// a failure — the operator machine has no zsh, and CI runners may not either.
// trace:TASK-1174 | ai:claude
#[test]
fn shell_matrix_always_covers_bash_and_skips_a_missing_zsh() {
    let shells = wrapper_shells();
    assert!(
        shells.contains(&"bash"),
        "bash coverage is not optional: {shells:?}"
    );

    let zsh_runs = std::process::Command::new("zsh")
        .args(["-f", "-c", "exit 0"])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    assert_eq!(
        shells.contains(&"zsh"),
        zsh_runs,
        "zsh must join the matrix exactly when it is installed (installed={zsh_runs}): {shells:?}"
    );
}

/// The divergence gate. The wrapper's block-slicing is pure parameter
/// expansion, and bash and zsh do not always agree on it. Run the same stub
/// through every available shell and demand byte-identical stdout/stderr/status
/// — so a zsh-only slicing difference fails HERE rather than in a zsh user's
/// terminal.
// trace:TASK-1174 | ai:claude
#[test]
fn block_slicing_agrees_across_shells() {
    let cases: [(&str, String, &str); 4] = [
        (
            "prose around a payload",
            prose_and_payload_stub(),
            "aida worktree enter TASK-1174\n\
             echo \"rc:$?\"\n\
             echo \"marker:${AIDA_TASK1171_MARKER-unset}\"",
        ),
        (
            "payload then prose",
            trailing_prose_stub(),
            "aida role enter advisor\necho \"tail:${AIDA_TASK1171_TAIL-unset}\"",
        ),
        (
            "failure with a payload",
            failing_stub(),
            "aida session end 019f8357 --yes\n\
             echo \"rc:$?\"\n\
             echo \"leaked:${AIDA_TASK1171_SHOULD_NOT_APPLY-unset}\"",
        ),
        (
            "unmarked legacy payload",
            LEGACY_STUB.to_string(),
            "aida dev activate\n\
             echo \"rc:$?\"\n\
             echo \"legacy:${AIDA_TASK1171_LEGACY-unset}\"",
        ),
    ];

    for (label, stub, body) in &cases {
        let baseline = run_wrapper_in("bash", stub, body);
        for &shell in wrapper_shells() {
            if shell == "bash" {
                continue;
            }
            let actual = run_wrapper_in(shell, stub, body);
            assert_eq!(
                actual.0, baseline.0,
                "[{label}] {shell} sliced stdout differently than bash\n\
                 {shell}:\n{}\nbash:\n{}",
                actual.0, baseline.0
            );
            assert_eq!(
                actual.1, baseline.1,
                "[{label}] {shell} routed stderr differently than bash\n\
                 {shell}:\n{}\nbash:\n{}",
                actual.1, baseline.1
            );
            assert_eq!(
                actual.2, baseline.2,
                "[{label}] {shell} returned a different status than bash"
            );
        }
    }
}

// ── stubs shared by the matrix ───────────────────────────────────────────────

/// A subcommand that prints hostile prose (backticks, bare words) AND emits a
/// marked payload.
// trace:TASK-1174 | ai:claude
fn prose_and_payload_stub() -> String {
    format!(
        "#!/usr/bin/env bash\n\
         printf 'Entered worktree for TASK-1171\\n'\n\
         printf 'resume it later with `aida worktree enter`\\n'\n\
         printf '%s\\n' '{begin}'\n\
         printf 'export AIDA_TASK1171_MARKER=applied\\n'\n\
         printf '%s\\n' '{end}'\n\
         exit 0\n",
        begin = EVAL_BEGIN,
        end = EVAL_END,
    )
}

/// A subcommand whose prose TRAILS the payload.
// trace:TASK-1174 | ai:claude
fn trailing_prose_stub() -> String {
    format!(
        "#!/usr/bin/env bash\n\
         printf '%s\\n' '{begin}'\n\
         printf 'export AIDA_TASK1171_TAIL=set\\n'\n\
         printf '%s\\n' '{end}'\n\
         printf 'Next: aida session end\\n'\n\
         exit 0\n",
        begin = EVAL_BEGIN,
        end = EVAL_END,
    )
}

/// A FAILING subcommand that still emitted a payload.
// trace:TASK-1174 | ai:claude
fn failing_stub() -> String {
    format!(
        "#!/usr/bin/env bash\n\
         printf 'error: \"No lease found for branch `019f8357`\"\\n'\n\
         printf '%s\\n' '{begin}'\n\
         printf 'export AIDA_TASK1171_SHOULD_NOT_APPLY=1\\n'\n\
         printf '%s\\n' '{end}'\n\
         exit 1\n",
        begin = EVAL_BEGIN,
        end = EVAL_END,
    )
}

/// A pre-channel binary: a bare payload with no markers at all.
// trace:TASK-1174 | ai:claude
const LEGACY_STUB: &str = "#!/usr/bin/env bash\n\
                           echo \"# aida dev activate\"\n\
                           echo \"export AIDA_TASK1171_LEGACY=applied\"\n\
                           exit 0\n";
