//! TASK-667: the shell wrapper exports AIDA_SHELL_WRAPPER; the binary
//! must then emit the BARE auto-eval hint (the `aida()` function evals
//! the binary's stdout, so `eval "$(...)"` would double-eval). Without
//! the wrapper, the `eval "$(...)"` form is required. trace:TASK-667
use super::eval_subcommand_hint;

// Single test (not split) so the two branches mutate AIDA_SHELL_WRAPPER
// sequentially — env vars are process-global and tests run in parallel.
#[test]
fn bare_when_wrapper_set_eval_form_when_unset() {
    let prev = std::env::var_os("AIDA_SHELL_WRAPPER");

    // Wrapper present → bare form (no eval wrapping).
    std::env::set_var("AIDA_SHELL_WRAPPER", "role,session,dev");
    assert_eq!(
        eval_subcommand_hint("role enter advisor"),
        "aida role enter advisor"
    );
    // Even an empty value counts as present (var set by the wrapper).
    std::env::set_var("AIDA_SHELL_WRAPPER", "");
    assert_eq!(eval_subcommand_hint("dev activate"), "aida dev activate");

    // Wrapper absent → eval "$(...)" form (raw binary on PATH).
    std::env::remove_var("AIDA_SHELL_WRAPPER");
    assert_eq!(
        eval_subcommand_hint("role enter advisor"),
        "eval \"$(aida role enter advisor)\""
    );
    assert_eq!(
        eval_subcommand_hint("session end"),
        "eval \"$(aida session end)\""
    );

    // Restore whatever the test environment had.
    match prev {
        Some(v) => std::env::set_var("AIDA_SHELL_WRAPPER", v),
        None => std::env::remove_var("AIDA_SHELL_WRAPPER"),
    }
}
