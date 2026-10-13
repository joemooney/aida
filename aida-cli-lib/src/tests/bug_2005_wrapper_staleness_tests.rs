//! BUG-2005: installed shell wrappers carrying the BUG-2000 zsh leak are
//! detected as stale so dev status triggers a re-install, and newly installed
//! wrappers are current.
// trace:BUG-2005 | ai:antigravity

use crate::dev_cmd::{
    classify_wrapper, format_shell_wrapper_status, installed_helpers_are_current, WrapperState,
    SHELL_HELPERS,
};
use crate::shell_eval::{EVAL_BLOCK_CAP, ZSH_LOCALOPTS_CAP};
use crate::shell_wrapper_harness::{shell_args, wrapper_shells};
use crate::test_exec::write_executable;

/// Real pre-fix leaking wrapper built from `3618fe739b^` (the commit immediately
/// preceding BUG-2000 / PR #2584), containing header + `SHELL_HELPERS` + `WORKER_FUNCTION`.
///
/// Provenance:
/// - Source commit: `3618fe739b^` (`fba16d9b9c9f7a8342416bc28ca1e8ba846a84eb`)
/// - Assembled components: file header comment + `SHELL_HELPERS` + `WORKER_FUNCTION` (21,143 bytes)
/// - SHA256: `5344afd309d4fda467d230f44f150f2327916968d42bc3771fac56be1e841109`
/// - Regeneration: extract header comments, `SHELL_HELPERS`, and `WORKER_FUNCTION` from
///   `3618fe739b^:aida-cli-lib/src/dev_cmd.rs` and concatenate them.
const PRE_FIX_LEAKING_WRAPPER: &str = include_str!("fixtures/bug_2005_leaking_wrapper.sh");

#[test]
fn pre_fix_leaking_wrapper_is_classified_stale_and_needs_reinstall() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let helper_path = tmp.path().join("shell-init.sh");
    std::fs::write(&helper_path, PRE_FIX_LEAKING_WRAPPER).expect("write fixture");

    // The installed file itself must be detected as NOT current (stale), so
    // `print_shell_wrapper_status` knows re-running `--install` will fix it.
    let is_current = installed_helpers_are_current(&helper_path);
    assert!(
        !is_current,
        "pre-fix leaking wrapper must NOT be reported as current"
    );

    // Derive the marker by sourcing the fixture through each supported shell
    // in `wrapper_shells()` and inspecting the exported `AIDA_SHELL_WRAPPER`.
    for &shell in wrapper_shells() {
        let script = format!(
            ". '{}'\nprintf '%s' \"$AIDA_SHELL_WRAPPER\"\n",
            helper_path.display()
        );
        let out = std::process::Command::new(shell)
            .args(shell_args(shell))
            .arg(&script)
            .output()
            .unwrap_or_else(|e| panic!("failed to run {shell}: {e}"));

        assert!(
            out.status.success(),
            "sourcing fixture in {shell} failed: stdout={:?}, stderr={:?}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );

        let derived_marker = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert!(
            !derived_marker.is_empty(),
            "derived marker from {shell} must not be empty"
        );
        assert!(
            !derived_marker.split(',').any(|c| c == ZSH_LOCALOPTS_CAP),
            "pre-fix leaking wrapper in {shell} must lack {ZSH_LOCALOPTS_CAP} capability"
        );

        let state = classify_wrapper(Some(&derived_marker));
        assert_eq!(
            state,
            WrapperState::Stale,
            "pre-fix marker derived from {shell} must classify as Stale"
        );

        // Run the whole status path: pass `installed_helpers_are_current(fixture)`
        // into `format_shell_wrapper_status` and assert the remedy line names the reinstall command.
        let status_lines = format_shell_wrapper_status(state, is_current);
        assert!(
            status_lines
                .iter()
                .any(|l| l.contains("aida dev shell-init --install")),
            "stale status for leaking wrapper must instruct reinstall: {status_lines:?}"
        );
    }
}

#[test]
fn pre_fix_fixture_actually_leaks_in_zsh_and_not_in_bash() {
    const SECRET: &str = "bug2005-leaked-secret-token-xyz789";

    for &shell in wrapper_shells() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let home = dir.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let bin = dir.path().join("aida");
        write_executable(&bin, "#!/bin/sh\necho stub-ran\n");

        let script = format!(
            "PATH='{path}':\"$PATH\"\nexport PATH\n{fixture}\nAIDA_SECRET_KEY='{SECRET}'\nexport AIDA_SECRET_KEY\naida status\n",
            path = dir.path().display(),
            fixture = PRE_FIX_LEAKING_WRAPPER,
        );

        let out = std::process::Command::new(shell)
            .env_clear()
            .env("HOME", &home)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(dir.path())
            .args(shell_args(shell))
            .arg(&script)
            .output()
            .unwrap_or_else(|e| panic!("failed to run {shell}: {e}"));

        assert!(
            out.status.success(),
            "{shell} failed: stderr={:?}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);

        if shell == "zsh" {
            assert!(
                stdout.contains(SECRET),
                "zsh must leak environment parameters when running pre-fix wrapper:\n{stdout}"
            );
        } else {
            assert!(
                !stdout.contains(SECRET),
                "{shell} should not leak environment parameters:\n{stdout}"
            );
        }
    }
}

#[test]
fn fresh_install_wrapper_is_current() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let helper_path = tmp.path().join("shell-init.sh");
    std::fs::write(&helper_path, SHELL_HELPERS).expect("write SHELL_HELPERS");

    assert!(
        installed_helpers_are_current(&helper_path),
        "freshly written SHELL_HELPERS must be current"
    );

    let extracted_marker = SHELL_HELPERS
        .lines()
        .find_map(|l| l.strip_prefix("export AIDA_SHELL_WRAPPER="))
        .expect("wrapper must export AIDA_SHELL_WRAPPER")
        .trim_matches('\'');

    assert_eq!(
        classify_wrapper(Some(extracted_marker)),
        WrapperState::Current,
        "marker extracted from current SHELL_HELPERS must classify as Current"
    );
}

#[test]
fn installed_helpers_detects_leaking_local_dash_as_stale() {
    let tmp = tempfile::tempdir().expect("create tempdir");

    // Missing file is not current.
    let non_existent = tmp.path().join("does_not_exist.sh");
    assert!(!installed_helpers_are_current(&non_existent));

    // Empty file is not current.
    let empty = tmp.path().join("empty.sh");
    std::fs::write(&empty, "").unwrap();
    assert!(!installed_helpers_are_current(&empty));

    // File with eval-block only is not current.
    let eval_only = tmp.path().join("eval_only.sh");
    std::fs::write(
        &eval_only,
        format!("export AIDA_SHELL_WRAPPER='{EVAL_BLOCK_CAP}'\n"),
    )
    .unwrap();
    assert!(!installed_helpers_are_current(&eval_only));

    // File with eval-block and zsh-localopts but missing setopt localoptions is not current.
    let missing_setopt = tmp.path().join("missing_setopt.sh");
    std::fs::write(
        &missing_setopt,
        format!("export AIDA_SHELL_WRAPPER='{EVAL_BLOCK_CAP},{ZSH_LOCALOPTS_CAP}'\nlocal - 2>/dev/null\n"),
    )
    .unwrap();
    assert!(!installed_helpers_are_current(&missing_setopt));
}

#[test]
fn classify_wrapper_requires_zsh_localopts_capability() {
    assert_eq!(classify_wrapper(None), WrapperState::Absent);
    assert_eq!(classify_wrapper(Some("")), WrapperState::Stale);
    assert_eq!(classify_wrapper(Some(EVAL_BLOCK_CAP)), WrapperState::Stale);
    assert_eq!(
        classify_wrapper(Some(ZSH_LOCALOPTS_CAP)),
        WrapperState::Stale
    );
    assert_eq!(
        classify_wrapper(Some(&format!("{EVAL_BLOCK_CAP},{ZSH_LOCALOPTS_CAP}"))),
        WrapperState::Current
    );
    assert_eq!(
        classify_wrapper(Some(&format!(
            "role,dev,{},worktree,{}",
            EVAL_BLOCK_CAP.to_uppercase(),
            ZSH_LOCALOPTS_CAP.to_uppercase()
        ))),
        WrapperState::Current
    );
}

#[test]
fn stale_wrapper_status_names_reinstall_command() {
    let lines = format_shell_wrapper_status(WrapperState::Stale, false);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("aida dev shell-init --install")),
        "stale status without up-to-date helpers must instruct running `aida dev shell-init --install`: {lines:?}"
    );

    let up_to_date_lines = format_shell_wrapper_status(WrapperState::Stale, true);
    assert!(
        up_to_date_lines
            .iter()
            .any(|l| l.contains("already up to date") && l.contains("open a new shell")),
        "stale status with up-to-date helpers must instruct opening a new shell: {up_to_date_lines:?}"
    );

    let current_lines = format_shell_wrapper_status(WrapperState::Current, false);
    assert!(
        current_lines.iter().any(|l| l.contains("current")),
        "current status must report current: {current_lines:?}"
    );

    let absent_lines = format_shell_wrapper_status(WrapperState::Absent, false);
    assert!(
        absent_lines
            .iter()
            .any(|l| l.contains("aida dev shell-init --install")),
        "absent status must name install command: {absent_lines:?}"
    );
}
