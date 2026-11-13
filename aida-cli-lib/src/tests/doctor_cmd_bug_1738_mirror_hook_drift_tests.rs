// trace:TASK-3780 | ai:codex
use super::*;

fn hooks_dir_with(hook: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    if let Some(content) = hook {
        std::fs::write(dir.path().join("pre-push"), content).unwrap();
    }
    dir
}

#[test]
fn current_hook_is_clean() {
    let dir = hooks_dir_with(Some(&crate::remote_create::mirror_pre_push_hook_script()));
    assert!(mirror_hook_drift_finding(dir.path()).is_none());
}

#[test]
fn stale_aida_hook_is_reported_with_the_reinstall_remedy() {
    // The pre-BUG-1706 script verbatim, as found installed on a real
    // machine two days after the fix merged: AIDA's banner, no --dry-run
    // forwarding.
    let stale = "#!/bin/sh\n\
             # Mirror fan-out pre-push hook — installed by `aida remote mirror`.\n\
             # Fans each code ref pushed to origin out to every configured mirror hub\n\
             # ([store.sync] mirror_remotes in .aida/config.toml). Best-effort: a\n\
             # mirror failure warns and never blocks the push. Safe to delete;\n\
             # reinstall with `aida remote mirror <name>`.\n\
             unset GIT_DIR GIT_WORK_TREE\n\
             if command -v aida >/dev/null 2>&1; then\n\
             \u{20} aida remote mirror-push \"$1\" || true\n\
             fi\n\
             exit 0\n";
    let dir = hooks_dir_with(Some(stale));
    let finding = mirror_hook_drift_finding(dir.path()).expect("stale hook must be reported");
    assert_eq!(finding.category, "mirror-hook-drift");
    assert_eq!(finding.id, "pre-push-mirror-hook-stale");
    assert!(
        finding.action.contains("aida remote mirror <name>"),
        "the remedy must name the re-install command: {}",
        finding.action
    );
    assert!(
        !finding.safe_heal,
        "rewriting .git/hooks/ is a write outside the store — report-only"
    );
}

#[test]
fn foreign_hook_is_not_flagged() {
    let dir = hooks_dir_with(Some("#!/bin/sh\n# deploy gate\nexit 0\n"));
    assert!(mirror_hook_drift_finding(dir.path()).is_none());
}

#[test]
fn foreign_hook_with_the_pasted_marker_line_is_not_flagged() {
    // The installer's own advice to custom-hook owners: add the marker
    // line to your hook. Such a hook contains MIRROR_HOOK_MARKER but not
    // the banner, and must never be reported as AIDA's to rewrite.
    let dir = hooks_dir_with(Some(
        "#!/bin/sh\n# my own pre-push checks\naida remote mirror-push \"$1\" || true\nexit 0\n",
    ));
    assert!(mirror_hook_drift_finding(dir.path()).is_none());
}

#[test]
fn absent_hook_is_clean() {
    let dir = hooks_dir_with(None);
    assert!(mirror_hook_drift_finding(dir.path()).is_none());
}

#[test]
fn newline_and_trailing_whitespace_drift_is_tolerated() {
    // The benign drift the installer's own refresh gate tolerates
    // (generated_text_matches): CRLF conversion and a trailing newline.
    let crlf = crate::remote_create::mirror_pre_push_hook_script().replace('\n', "\r\n") + "\r\n";
    let dir = hooks_dir_with(Some(&crlf));
    assert!(mirror_hook_drift_finding(dir.path()).is_none());
}

#[test]
fn category_and_aliases_resolve() {
    for raw in [
        "mirror-hook-drift",
        "mirror-hook",
        "pre-push-hook",
        "hook-drift",
    ] {
        assert!(doctor_category_selected(Some(raw), "mirror-hook-drift").unwrap());
    }
    assert!(!doctor_category_selected(Some("agent-launch"), "mirror-hook-drift").unwrap());
}
