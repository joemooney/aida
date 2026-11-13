// trace:TASK-3780 | ai:codex
use super::*;
use std::path::{Path, PathBuf};

// trace:BUG-1744 trace:BUG-1777 | ai:codex
#[test]
fn broad_ancestor_with_unset_parent_discloses_the_narrow_remediation() {
    let trusted = vec![PathBuf::from("/home/op")];
    let findings = worktree_trust_findings(
        Path::new("/home/op"),
        false,
        Some(&trusted),
        Some(TrustBreadth { unrelated_dirs: 5 }),
    );
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, "worktree-folder-trust-breadth");
    assert_eq!(findings[0].category, "agent-launch");
    assert!(findings[0].summary.replace('\\', "/").contains("/home/op"));
    assert!(findings[0].summary.contains('5'));
    assert!(findings[0].action.contains("worktree_parent"));
    assert!(!findings[0].safe_heal);
}

// trace:BUG-1744 | ai:codex
#[test]
fn configured_and_trusted_stays_silent_even_under_a_broad_grant() {
    let trusted = vec![PathBuf::from("/home/op")];
    assert!(worktree_trust_findings(
        Path::new("/home/op/aida-worktrees"),
        true,
        Some(&trusted),
        Some(TrustBreadth { unrelated_dirs: 5 })
    )
    .is_empty());
}

// trace:BUG-1744 | ai:codex
#[test]
fn single_project_parent_is_not_a_broad_grant() {
    let trusted = vec![PathBuf::from("/home/op")];
    assert!(worktree_trust_findings(
        Path::new("/home/op"),
        false,
        Some(&trusted),
        Some(TrustBreadth { unrelated_dirs: 0 })
    )
    .is_empty());
}

// trace:BUG-1744 | ai:codex
#[test]
fn unknown_breadth_is_silent() {
    let trusted = vec![PathBuf::from("/home/op")];
    assert!(worktree_trust_findings(Path::new("/home/op"), false, Some(&trusted), None).is_empty());
}

// trace:BUG-1744 | ai:codex
#[test]
fn an_untrusted_parent_still_reports_the_original_finding_id() {
    let trusted = vec![PathBuf::from("/home/op/other")];
    let findings = worktree_trust_findings(
        Path::new("/home/op/aida-worktrees"),
        false,
        Some(&trusted),
        Some(TrustBreadth { unrelated_dirs: 9 }),
    );
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, "worktree-folder-trust");
}

// trace:BUG-1744 | ai:codex
#[test]
fn counts_only_unrelated_sibling_directories() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let worktrees = root.path().join("worktrees");
    std::fs::create_dir(&project).unwrap();
    std::fs::create_dir(&worktrees).unwrap();
    for name in ["one", "two", "three", ".hidden"] {
        std::fs::create_dir(root.path().join(name)).unwrap();
    }
    std::fs::write(root.path().join("file"), b"file").unwrap();
    assert_eq!(
        count_unrelated_child_dirs(root.path(), &project, &worktrees),
        3
    );
}

// An ancestor we cannot list must not be reported as a broad grant.
// trace:BUG-1744 | ai:codex
#[test]
fn an_unreadable_ancestor_counts_as_no_breadth() {
    let root = tempfile::tempdir().unwrap();
    let missing_ancestor = root.path().join("never-created");
    let project = root.path().join("project");
    let worktrees = root.path().join("worktrees");
    assert_eq!(
        count_unrelated_child_dirs(&missing_ancestor, &project, &worktrees),
        0
    );
}

// trace:BUG-1744 | ai:codex
// trace:TASK-1748 | ai:antigravity
#[test]
fn the_trust_scan_never_writes_the_claude_config() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fake_home = tempfile::tempdir().unwrap();
    let project_root = tempfile::tempdir().unwrap();
    let trusted_parent = project_root.path().parent().unwrap();
    let config_path = fake_home.path().join(".claude.json");
    let config = serde_json::json!({
        "projects": {
            trusted_parent.to_string_lossy().as_ref(): {
                "hasTrustDialogAccepted": true
            }
        }
    });
    std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let before = std::fs::read(&config_path).unwrap();
    let fake_home_path = fake_home.path().to_str().unwrap();
    let _env = crate::test_env::EnvVarsGuard::set(&[("HOME", fake_home_path)]);

    drop(scan_agent_worktree_trust(project_root.path()));

    assert_eq!(std::fs::read(&config_path).unwrap(), before);
}

/// BUG-2394: run the trust scan with no folder trusted and the project's
/// `.aida/config.toml` set to `config_toml`.
// trace:BUG-2394 | ai:claude
fn trust_scan_with_config(config_toml: &str) -> Vec<DoctorFinding> {
    let fake_home = tempfile::tempdir().unwrap();
    let project_root = tempfile::tempdir().unwrap();
    std::fs::write(fake_home.path().join(".claude.json"), r#"{"projects":{}}"#).unwrap();
    std::fs::create_dir_all(project_root.path().join(".aida")).unwrap();
    std::fs::write(project_root.path().join(".aida/config.toml"), config_toml).unwrap();
    let fake_home_path = fake_home.path().to_str().unwrap();
    let _env = crate::test_env::EnvVarsGuard::set(&[("HOME", fake_home_path)]);
    scan_agent_worktree_trust(project_root.path())
}

// `[agents] vendor` is only the default vendor: with no enabled list Claude
// still launches, so its folder-trust check must still run.
// trace:BUG-2394 | ai:claude
#[test]
fn bug_2394_codex_default_vendor_keeps_the_claude_trust_check() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let findings = trust_scan_with_config("[agents]\nvendor = \"codex\"\n");
    assert!(
        findings.iter().any(|f| f.id == "worktree-folder-trust"),
        "vendor=codex with no enabled list must keep the Claude trust check: {findings:?}"
    );
}

// trace:BUG-2394 | ai:claude
#[test]
fn bug_2394_enabled_list_without_claude_silences_the_claude_trust_check() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let findings = trust_scan_with_config("[agents]\nvendor = \"codex\"\nenabled = [\"codex\"]\n");
    assert!(findings.is_empty(), "{findings:?}");
}
