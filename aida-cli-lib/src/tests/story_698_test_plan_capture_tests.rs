use super::*;

fn steps(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn flags_win_and_join_steps_skipping_the_prompt() {
    // Acceptance: `--test-plan STEP` (repeatable) records exactly those
    // steps, joined newline-separated, and never prompts — even at a TTY.
    let d = decide_test_plan_capture(
        &steps(&["cargo test -p aida-cli", "manual: aida queue done"]),
        /* no_test_plan = */ false,
        /* interactive = */ true,
        /* at_tty = */ true,
        /* disabled = */ false,
        /* already_set = */ false,
    );
    assert_eq!(
        d,
        TestPlanCapture::Record("cargo test -p aida-cli\nmanual: aida queue done".to_string())
    );
}

#[test]
fn flag_overrides_an_already_set_value() {
    // A `--test-plan` flag is explicit intent — it overwrites even when
    // notes already exist (the "no flag overrides" carve-out is #6).
    let d = decide_test_plan_capture(
        &steps(&["new step"]),
        false,
        true,
        true,
        false,
        /* already_set = */ true,
    );
    assert_eq!(d, TestPlanCapture::Record("new step".to_string()));
}

#[test]
fn no_test_plan_flag_skips_the_prompt() {
    // Acceptance: `--no-test-plan` skips the interactive prompt and records
    // nothing, leaving any existing value untouched.
    let d = decide_test_plan_capture(
        &[],
        /* no_test_plan = */ true,
        true,
        true,
        false,
        false,
    );
    assert_eq!(d, TestPlanCapture::Skip);
}

#[test]
fn non_interactive_no_flag_is_a_silent_skip() {
    // Acceptance: non-interactive (`--yes`) with no flag ⇒ leave
    // test_coverage_notes untouched and print no prompt.
    let d = decide_test_plan_capture(
        &[],
        false,
        /* interactive = */ false,
        true,
        false,
        false,
    );
    assert_eq!(d, TestPlanCapture::Skip);
}

#[test]
fn no_tty_never_prompts() {
    // A non-TTY interactive invocation (piped stdin) must not block on a
    // prompt that nobody can answer.
    let d = decide_test_plan_capture(&[], false, true, /* at_tty = */ false, false, false);
    assert_eq!(d, TestPlanCapture::Skip);
}

#[test]
fn env_opt_out_skips_the_prompt() {
    // Acceptance: `AIDA_AUTO_TEST_PLAN_CAPTURE=false` disables the capture.
    let d = decide_test_plan_capture(&[], false, true, true, /* disabled = */ true, false);
    assert_eq!(d, TestPlanCapture::Skip);
}

#[test]
fn already_set_leaves_the_value_unchanged_on_the_prompt_path() {
    // Acceptance #6: with a value already captured and no flag override,
    // the interactive path skips (never re-prompts, never clobbers).
    let d = decide_test_plan_capture(&[], false, true, true, false, /* already_set = */ true);
    assert_eq!(d, TestPlanCapture::Skip);
}

#[test]
fn tty_interactive_first_time_prompts() {
    // The one path that reaches the prompt: TTY + interactive + not opted
    // out + nothing captured yet + no flags.
    let d = decide_test_plan_capture(&[], false, true, true, false, false);
    assert_eq!(d, TestPlanCapture::Prompt);
}

#[test]
fn env_disabled_predicate_matches_falsey_values() {
    for (val, want) in [
        ("0", true),
        ("false", true),
        ("no", true),
        ("FALSE", true),
        ("1", false),
        ("true", false),
        ("", false),
    ] {
        std::env::set_var("AIDA_AUTO_TEST_PLAN_CAPTURE", val);
        assert_eq!(
            capture_test_plan_disabled(),
            want,
            "AIDA_AUTO_TEST_PLAN_CAPTURE={val:?}"
        );
    }
    std::env::remove_var("AIDA_AUTO_TEST_PLAN_CAPTURE");
}
