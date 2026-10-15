//! BUG-2000: the wrapper's option scoping must not print the environment.
//!
//! `local -` is a bash idiom. In zsh it is a bare `local`, which lists every
//! shell parameter (API keys included) on stdout on each wrapped call. The
//! wrapper now uses `setopt localoptions` under zsh and keeps `local -` for
//! bash only.
// trace:BUG-2000 | ai:claude

use crate::shell_wrapper_harness::{run_wrapper_in, wrapper_shells};

const SENTINEL: &str = "bug2000-sentinel-value-must-not-leak";

/// A plain wrapped call prints only the binary's own output: no shell
/// parameter (here a sentinel standing in for an API key) reaches stdout.
#[test]
fn wrapped_call_does_not_print_shell_parameters() {
    for shell in wrapper_shells() {
        let (stdout, stderr, code) = run_wrapper_in(
            shell,
            "#!/bin/sh\necho stub-ran\n",
            &format!("AIDA_FAKE_TOKEN='{SENTINEL}'\nexport AIDA_FAKE_TOKEN\naida status"),
        );
        assert_eq!(code, Some(0), "{shell}: stderr={stderr}");
        assert!(
            !stdout.contains(SENTINEL),
            "{shell} leaked parameters:\n{stdout}"
        );
        assert_eq!(
            stdout.trim(),
            "stub-ran",
            "{shell}: unexpected stdout:\n{stdout}"
        );
    }
}

/// The wrapper's `set +x` stays scoped to the function in both shells: a
/// caller that had xtrace on still has it after the call.
#[test]
fn wrapper_option_changes_do_not_escape_the_function() {
    for shell in wrapper_shells() {
        let (stdout, stderr, code) = run_wrapper_in(
            shell,
            "#!/bin/sh\necho stub-ran\n",
            "set -x\naida status\ncase $- in *x*) echo xtrace-kept ;; *) echo xtrace-lost ;; esac",
        );
        assert_eq!(code, Some(0), "{shell}: stderr={stderr}");
        assert!(stdout.contains("xtrace-kept"), "{shell}:\n{stdout}");
    }
}

/// Static guard for runners without zsh: the only `local -` in the wrapper
/// sits in the non-zsh branch.
#[test]
fn bare_local_dash_is_guarded_from_zsh() {
    let helpers = crate::dev_cmd::SHELL_HELPERS;
    let lines: Vec<&str> = helpers.lines().map(str::trim).collect();
    let local_dash: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with("local -") && !l.starts_with("local -a"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(local_dash.len(), 1, "expected exactly one `local -`");
    let i = local_dash[0];
    assert_eq!(lines[i - 1], "else", "`local -` must be the non-zsh branch");
    assert_eq!(lines[i - 2], "setopt localoptions");
    assert_eq!(lines[i - 3], "if [ -n \"${ZSH_VERSION:-}\" ]; then");
}
