//! BUG-2005: installed shell wrappers carrying the BUG-2000 zsh leak are
//! detected as stale so dev status triggers a re-install, and newly installed
//! wrappers are current.
// trace:BUG-2005 | ai:antigravity

use crate::dev_cmd::{
    classify_wrapper, installed_helpers_are_current, WrapperState, SHELL_HELPERS,
};
use crate::shell_eval::{EVAL_BLOCK_CAP, ZSH_LOCALOPTS_CAP};

/// Fixture representing the pre-BUG-2000 / pre-BUG-2005 wrapper that leaked
/// shell parameters in zsh due to the unguarded `local -` and lacked the
/// `zsh-localopts` capability marker.
const PRE_FIX_LEAKING_WRAPPER: &str = r#"# AIDA shell wrapper.
export AIDA_SHELL_WRAPPER='role,session,dev,worktree,worktree-exit,worktree-stale,eval-block,init-cd-file'

aida() {
    # Suppress set -x trace for wrapper internals
    local - 2>/dev/null || true
    set +x 2>/dev/null
    local _aida_cmd="${1:-} ${2:-}"
    case "$_aida_cmd" in
        *)
            command aida "$@"
            ;;
    esac
}
"#;

#[test]
fn pre_fix_leaking_wrapper_is_classified_stale_and_needs_reinstall() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let helper_path = tmp.path().join("shell-init.sh");
    std::fs::write(&helper_path, PRE_FIX_LEAKING_WRAPPER).expect("write fixture");

    // The installed file itself must be detected as NOT current (stale), so
    // `print_shell_wrapper_status` knows re-running `--install` will fix it.
    assert!(
        !installed_helpers_are_current(&helper_path),
        "pre-fix leaking wrapper must NOT be reported as current"
    );

    // Sourced in a shell, its exported marker lacks `zsh-localopts`, so it
    // must classify as Stale (not Current).
    let pre_fix_marker =
        "role,session,dev,worktree,worktree-exit,worktree-stale,eval-block,init-cd-file";
    assert_eq!(
        classify_wrapper(Some(pre_fix_marker)),
        WrapperState::Stale,
        "pre-fix marker lacking zsh-localopts must classify as Stale"
    );
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
