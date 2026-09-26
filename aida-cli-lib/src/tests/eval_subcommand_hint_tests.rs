//! TASK-667: the shell wrapper exports AIDA_SHELL_WRAPPER; the binary
//! must then emit the BARE auto-eval hint (the `aida()` function evals
//! the binary's stdout, so `eval "$(...)"` would double-eval). Without
//! the wrapper, the `eval "$(...)"` form is required. trace:TASK-667
use super::eval_subcommand_hint;

// Single test (not split) so the two branches mutate AIDA_SHELL_WRAPPER
// sequentially — env vars are process-global and tests run in parallel. The
// guard holds the shared env lock for the whole test and restores the prior
// value on drop. trace:BUG-1666 | ai:claude
#[test]
fn bare_when_wrapper_set_eval_form_when_unset() {
    // Wrapper present → bare form (no eval wrapping).
    let mut env = crate::test_env::EnvVarGuard::set("AIDA_SHELL_WRAPPER", "role,session,dev");
    assert_eq!(
        eval_subcommand_hint("role enter advisor"),
        "aida role enter advisor"
    );
    // Even an empty value counts as present (var set by the wrapper).
    env.reset("");
    assert_eq!(eval_subcommand_hint("dev activate"), "aida dev activate");

    // Wrapper absent → eval "$(...)" form (raw binary on PATH).
    env.reset_unset();
    assert_eq!(
        eval_subcommand_hint("role enter advisor"),
        "eval \"$(aida role enter advisor)\""
    );
    assert_eq!(
        eval_subcommand_hint("session end"),
        "eval \"$(aida session end)\""
    );
}
