//! BUG-1686: the Antigravity (`agy`) headless launch argv ORDER.
//!
//! Observed 2026-09-27 running `aida queue work BUG-1680 --vendor agy
//! --no-human=both --strict`: the headless launcher emitted
//! `-p --dangerously-skip-permissions [--effort E] <prompt>`. Antigravity CLI
//! 1.2.12 parses Go-style flags and `-p` / `--print` / `--prompt` TAKES A VALUE
//! ("Run a single prompt non-interactively and print the response") — unlike
//! claude's boolean `-p` — so agy read the literal string
//! `--dangerously-skip-permissions` as the prompt, never saw the permission
//! flag as a flag, and rejected the launch. AIDA had already minted the
//! lease/worktree, so the spec sat In Progress with no implementer process.
//!
//! These tests pin the ORDER itself, not merely that a launch succeeds: every
//! option flag precedes `-p`, and the prompt is `-p`'s value at the tail. A
//! future reordering fails here.
//!
//! No real vendor CLI is ever invoked: the one spawning test installs a tiny
//! `#!/bin/sh` argv-recording fixture through the `AIDA_AGENT_CMD` mock seam
//! (the BUG-705 seam, as used by `bug_1607_reviewer_vendor_tests`), under a
//! fake `HOME`/`AIDA_HOME`.
// trace:BUG-1686 | ai:claude
use crate::session::{self, agy_headless_args, agy_headless_args_with_effort, HeadlessVendor};

/// The permission posture BUG-1686 must never weaken — one flag, parsed as a
/// flag.
// trace:BUG-1686 | ai:claude
const AGY_PERMISSION_FLAG: &str = "--dangerously-skip-permissions";

/// Position of `needle` in `argv`, or a readable panic naming the whole vector.
// trace:BUG-1686 | ai:claude
fn index_of(argv: &[String], needle: &str) -> usize {
    argv.iter()
        .position(|a| a == needle)
        .unwrap_or_else(|| panic!("`{needle}` missing from agy argv: {argv:?}"))
}

/// A minimal stand-in for Antigravity's Go `flag` parsing, over the argv AIDA
/// hands the process. Returns `(print_value, saw_permission_flag, leftovers)`.
///
/// Go's `flag` package (a) lets `-name value` consume the NEXT token as the
/// value of a non-boolean flag, and (b) stops parsing at the first non-flag
/// token, leaving the rest as positionals the CLI ignores. Both behaviours are
/// what broke the old ordering, so the test reasons about them explicitly
/// instead of trusting a `contains` check.
// trace:BUG-1686 | ai:claude
fn parse_like_go_flags(argv: &[String]) -> (Option<String>, bool, Vec<String>) {
    // `--print`/`--prompt`/`-p` and `--effort`/`--model` take a value; the
    // permission flag is boolean.
    let value_flags = ["-p", "--print", "--prompt", "--effort", "--model"];
    let mut print_value: Option<String> = None;
    let mut saw_permission_flag = false;
    let mut i = 0;
    while i < argv.len() {
        let tok = argv[i].as_str();
        if !tok.starts_with('-') || tok == "-" {
            break; // Go stops flag parsing here; the rest are positionals.
        }
        if tok == AGY_PERMISSION_FLAG {
            saw_permission_flag = true;
            i += 1;
            continue;
        }
        if value_flags.contains(&tok) {
            let value = argv
                .get(i + 1)
                .unwrap_or_else(|| panic!("`{tok}` has no value in agy argv: {argv:?}"))
                .clone();
            if matches!(tok, "-p" | "--print" | "--prompt") {
                assert!(
                    print_value.is_none(),
                    "the prompt flag must appear exactly once: {argv:?}"
                );
                print_value = Some(value);
            }
            i += 2;
            continue;
        }
        panic!("unexpected token `{tok}` in agy argv: {argv:?}");
    }
    (print_value, saw_permission_flag, argv[i..].to_vec())
}

/// BUG-1686: the exact argument vector for the plain headless launch — the
/// permission flag first, `-p <prompt>` last. Asserted as a whole vector, so a
/// reordering (or an extra token wedged between `-p` and the prompt) fails.
// trace:BUG-1686 | ai:claude
// trace:BUG-1686.ac8aba70 | ai:claude
#[test]
fn agy_headless_argv_is_permission_flag_then_dash_p_then_prompt() {
    let prompt = "/aida-pickup BUG-1680";
    assert_eq!(
        agy_headless_args(prompt),
        vec![
            AGY_PERMISSION_FLAG.to_string(),
            "-p".to_string(),
            prompt.to_string(),
        ],
        "agy headless argv order is load-bearing: option flags BEFORE -p"
    );
}

/// BUG-1686: the exact argument vector with a configured `--effort` (STORY-1033
/// per-seat tuning) — the model/output-style option flags stay ahead of `-p`
/// too, so the prompt remains `-p`'s value.
// trace:BUG-1686 | ai:claude
// trace:BUG-1686.ac8aba70 | ai:claude
// trace:BUG-1686.aca70285 | ai:claude
#[test]
fn agy_headless_argv_keeps_configured_effort_ahead_of_the_prompt_flag() {
    let prompt = "/aida-pickup BUG-1680";
    assert_eq!(
        agy_headless_args_with_effort(prompt, Some("high")),
        vec![
            AGY_PERMISSION_FLAG.to_string(),
            "--effort".to_string(),
            "high".to_string(),
            "-p".to_string(),
            prompt.to_string(),
        ],
        "a configured effort must not be pushed behind -p"
    );

    // A blank/whitespace effort is still dropped entirely (pre-BUG-1686
    // behaviour), and the ordering is the no-effort vector.
    for blank in ["", "   "] {
        assert_eq!(
            agy_headless_args_with_effort(prompt, Some(blank)),
            agy_headless_args(prompt),
            "blank effort {blank:?} must not emit --effort"
        );
    }
}

/// BUG-1686: the ORDER invariants, stated as invariants rather than as one
/// literal vector, so they keep biting when a future flag joins the arm:
/// the permission flag precedes `-p`; `-p` is second-to-last; the prompt is the
/// token immediately after `-p`; each appears exactly once; and nothing trails
/// the prompt as a stray positional.
// trace:BUG-1686 | ai:claude
// trace:BUG-1686.ac8aba70 | ai:claude
// trace:BUG-1686.aca70285 | ai:claude
// trace:BUG-1686.ac4341ac | ai:claude
#[test]
fn agy_headless_argv_order_invariants_hold_with_and_without_effort() {
    let prompt = "/aida-pickup BUG-1680";
    for effort in [None, Some("low"), Some("max")] {
        let argv = agy_headless_args_with_effort(prompt, effort);
        let perm = index_of(&argv, AGY_PERMISSION_FLAG);
        let dash_p = index_of(&argv, "-p");

        assert!(
            perm < dash_p,
            "the permission flag must precede -p (effort={effort:?}): {argv:?}"
        );
        assert_eq!(
            dash_p,
            argv.len() - 2,
            "-p must be second-to-last, with the prompt as its value: {argv:?}"
        );
        assert_eq!(
            argv.get(dash_p + 1).map(String::as_str),
            Some(prompt),
            "the token after -p must be the prompt itself, not a flag: {argv:?}"
        );
        assert_eq!(
            argv.iter().filter(|a| *a == AGY_PERMISSION_FLAG).count(),
            1,
            "permission flag exactly once: {argv:?}"
        );
        assert_eq!(
            argv.iter().filter(|a| *a == "-p").count(),
            1,
            "prompt flag exactly once: {argv:?}"
        );
        assert_eq!(
            argv.iter().filter(|a| *a == prompt).count(),
            1,
            "prompt exactly once: {argv:?}"
        );

        // And the whole point: a Go-style parse of this argv receives the
        // intended prompt and reads the permission flag AS a flag.
        let (print_value, saw_permission_flag, leftovers) = parse_like_go_flags(&argv);
        assert_eq!(
            print_value.as_deref(),
            Some(prompt),
            "agy must receive the intended prompt (effort={effort:?}): {argv:?}"
        );
        assert!(
            saw_permission_flag,
            "the permission posture must survive as a parsed flag: {argv:?}"
        );
        assert!(
            leftovers.is_empty(),
            "nothing may trail the prompt as an ignored positional: {leftovers:?}"
        );
    }
}

/// BUG-1686: the regression itself, spelled out — the pre-fix vector had the
/// permission flag as the PROMPT. Parsing the old shape the way agy 1.2.12 does
/// reproduces the failure, and the shape we now emit does not.
// trace:BUG-1686 | ai:claude
// trace:BUG-1686.ac8aba70 | ai:claude
#[test]
fn the_pre_fix_agy_argv_order_is_what_swallowed_the_prompt() {
    let prompt = "/aida-pickup BUG-1680";
    let pre_fix = vec![
        "-p".to_string(),
        AGY_PERMISSION_FLAG.to_string(),
        prompt.to_string(),
    ];
    let (print_value, saw_permission_flag, leftovers) = parse_like_go_flags(&pre_fix);
    assert_eq!(
        print_value.as_deref(),
        Some(AGY_PERMISSION_FLAG),
        "the old order fed the permission flag to -p as its prompt"
    );
    assert!(
        !saw_permission_flag,
        "the old order never presented the permission flag AS a flag"
    );
    assert_eq!(
        leftovers,
        vec![prompt.to_string()],
        "the real prompt was left as an ignored trailing positional"
    );

    assert_ne!(
        agy_headless_args(prompt),
        pre_fix,
        "the builder must never emit the pre-BUG-1686 order again"
    );
}

/// BUG-1686: neither of the other vendor arms shares the hazard, asserted so a
/// future edit cannot quietly give claude or codex the same shape. Claude's
/// `-p` is a BOOLEAN print flag (its prompt is a trailing positional) and codex
/// carries no `-p` at all — so for those arms a flag immediately after `-p` is
/// not a defect. This test records the distinction rather than widening the
/// BUG-1686 fix into them.
// trace:BUG-1686 | ai:claude
// trace:BUG-1686.ac4341ac | ai:claude
#[test]
fn claude_and_codex_arms_do_not_take_the_prompt_as_a_flag_value() {
    let prompt = "/aida-pickup BUG-1680";
    let sid = "019e0000-0000-7000-8000-000000000000";

    // Claude: `-p` leads and the prompt is the trailing POSITIONAL — correct
    // for claude's boolean `-p`, and unchanged by BUG-1686.
    let claude =
        session::headless_vendor_args(HeadlessVendor::Claude, prompt, sid, false, None, None);
    assert_eq!(claude.first().map(String::as_str), Some("-p"), "{claude:?}");
    assert_eq!(
        claude.last().map(String::as_str),
        Some(prompt),
        "{claude:?}"
    );
    assert_ne!(
        claude.get(claude.len() - 2).map(String::as_str),
        Some("-p"),
        "claude's prompt is a positional, not the value of -p: {claude:?}"
    );

    // Codex: no `-p` token exists, so there is no value-flag to swallow the
    // permission flag; the prompt is `codex exec`'s trailing positional.
    let codex =
        session::headless_vendor_args(HeadlessVendor::Codex, prompt, sid, false, None, None);
    assert!(
        !codex.iter().any(|a| a == "-p"),
        "codex arm must carry no -p: {codex:?}"
    );
    assert_eq!(codex.first().map(String::as_str), Some("exec"), "{codex:?}");
    assert_eq!(codex.last().map(String::as_str), Some(prompt), "{codex:?}");
}

/// Write a `#!/bin/sh` fixture at `dir/mock-agy.sh` that records each argv
/// element (one per line) into `capture` and exits 0. Mirrors
/// `bug_1607_reviewer_vendor_tests::write_argv_capture_mock`; kept local so
/// this file stands alone.
// trace:BUG-1686 | ai:claude
#[cfg(unix)]
fn write_argv_capture_mock(dir: &std::path::Path, capture: &std::path::Path) -> std::path::PathBuf {
    let script = dir.join("mock-agy.sh");
    crate::test_exec::write_executable(
        &script,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> \"{}\"; done\nexit 0\n",
            capture.display()
        ),
    );
    script
}

#[cfg(unix)]
fn read_captured_argv(capture: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(capture)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// BUG-1686: the launch regression — the argv the REAL headless spawn path
/// (`session::spawn_vendor_headless`, which `aida queue work --vendor agy
/// --no-human` reaches through `compose_headless_command` →
/// `headless_vendor_args`) hands to the process, recorded by the fixture and
/// asserted as an ordered vector.
///
/// This proves the process is reached AND that what it receives is ordered
/// correctly — the failure mode was a process that started and then rejected
/// its own argv, which a bare "the launch succeeded" assertion would miss.
// The fixture is a `#!/bin/sh` script, unspawnable on Windows (os error 193);
// same unix gating as the other shell-mock launch tests. trace:BUG-1646
// trace:BUG-1686 | ai:claude
// trace:BUG-1686.aca70285 | ai:claude
// trace:BUG-1686.acb8f1db | ai:claude
// trace:BUG-1686.ac4341ac | ai:claude
#[cfg(unix)]
#[test]
fn spawned_agy_process_receives_the_ordered_argv() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    let capture = tmp.path().join("captured-argv.txt");
    let mock = write_argv_capture_mock(tmp.path(), &capture);
    let log = tmp.path().join("headless.log");

    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", Some(home.to_str().unwrap())),
        ("HOME", Some(home.to_str().unwrap())),
        ("AIDA_AGENT_CMD", Some(mock.to_str().unwrap())),
        ("AIDA_OS_WRAP", Some("0")),
        // Pin the per-seat tuning so the expected vector is exact rather than
        // dependent on whatever the ambient config resolves to.
        ("AIDA_AGENT_MODEL", None),
        ("AIDA_AGENT_EFFORT", Some("high")),
    ]);

    let prompt = "/aida-pickup BUG-1680";
    let status = session::spawn_vendor_headless(
        HeadlessVendor::Agy,
        prompt,
        "019e0000-0000-7000-8000-0000000000aa",
        &log,
        &crate::headless_tee::TeeOptions {
            enabled: false,
            label: None,
        },
        false,
    )
    .expect("spawning the argv-recording fixture must succeed");
    assert!(status.success(), "fixture exits 0");

    let argv = read_captured_argv(&capture);
    assert_eq!(
        argv,
        vec![
            AGY_PERMISSION_FLAG.to_string(),
            "--effort".to_string(),
            "high".to_string(),
            "-p".to_string(),
            prompt.to_string(),
        ],
        "the spawned agy process must receive option flags BEFORE -p, with the \
         prompt as -p's value"
    );

    // And the recorded argv parses the way agy 1.2.12 parses it.
    let (print_value, saw_permission_flag, leftovers) = parse_like_go_flags(&argv);
    assert_eq!(print_value.as_deref(), Some(prompt), "{argv:?}");
    assert!(saw_permission_flag, "{argv:?}");
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// BUG-1686: `compose_headless_command` — the single function every headless
/// launch site builds its program+argv through — preserves the ordering and
/// routes the program through the `AIDA_AGENT_CMD` mock seam. Covers the
/// queue-work path's argv construction without spawning anything.
// trace:BUG-1686 | ai:claude
// trace:BUG-1686.acb8f1db | ai:claude
#[test]
fn compose_headless_command_for_agy_orders_flags_before_the_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    let fake = tmp.path().join("fake-agy");
    std::fs::write(&fake, "").unwrap();

    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", Some(home.to_str().unwrap())),
        ("HOME", Some(home.to_str().unwrap())),
        ("AIDA_AGENT_CMD", Some(fake.to_str().unwrap())),
        ("AIDA_OS_WRAP", Some("0")),
    ]);

    let prompt = "/aida-pickup BUG-1680";
    let (program, argv) = session::compose_headless_command(
        HeadlessVendor::Agy,
        prompt,
        "019e0000-0000-7000-8000-0000000000bb",
        false,
        None,
        Some("medium"),
    )
    .expect("composing the agy headless command must succeed with os_wrap off");

    assert_eq!(
        program,
        fake.to_str().unwrap(),
        "mock seam swaps the program"
    );
    assert_eq!(
        argv,
        vec![
            AGY_PERMISSION_FLAG.to_string(),
            "--effort".to_string(),
            "medium".to_string(),
            "-p".to_string(),
            prompt.to_string(),
        ],
        "compose_headless_command must not reorder the agy argv"
    );
}

/// BUG-1686: the operator-facing `--no-launch` / deferred handoff line for agy
/// — the command a human is told to paste when AIDA prepares a session but does
/// not launch it. It renders from the same builder, so before the fix the manual
/// workaround was broken in exactly the same way as the automatic launch.
// trace:BUG-1686 | ai:claude
#[test]
fn deferred_agy_launch_hint_renders_the_ordered_command() {
    let prompt = "/aida-pickup BUG-1680";
    let hint = crate::queue_cmd::deferred_headless_launch_hint(
        HeadlessVendor::Agy,
        prompt,
        "019e0000-0000-7000-8000-0000000000cc",
        false,
        None,
    );
    assert_eq!(
        hint, "AIDA_HEADLESS=1 agy --dangerously-skip-permissions -p '/aida-pickup BUG-1680'",
        "the pasteable agy command must carry the fixed flag order"
    );
}
