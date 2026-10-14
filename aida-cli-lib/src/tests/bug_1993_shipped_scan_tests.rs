//! BUG-1993: the default-branch linkage scan runs under one budget, and a
//! scan that runs out reports itself incomplete — the TOON line says
//! `shipped=unknown` and the completion warning stays silent — rather than
//! reading as "not shipped".
// trace:BUG-1993 | ai:claude
use super::*;

fn git(repo: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("git on PATH");
    assert!(status.success(), "git {args:?} failed");
}

/// A repo whose `main` carries one commit naming TASK-7.
fn shipped_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "--initial-branch=main", "--quiet"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    git(
        &root,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "fix: the thing (TASK-7)",
        ],
    );
    (tmp, root)
}

#[test]
fn scan_within_budget_is_complete() {
    let (_tmp, root) = shipped_repo();
    let scan = scan_default_branch_linkage(&root, &["TASK-7".to_string()]);
    assert!(!scan.scan_incomplete, "{scan:?}");
    assert_eq!(scan.found_on.as_deref(), Some("main"), "{scan:?}");
    assert!(render_toon_linkage(&scan).starts_with("shipped=true "));
}

#[test]
fn exhausted_budget_reads_unknown_not_unshipped() {
    let (_tmp, root) = shipped_repo();
    let scan = scan_default_branch_linkage_within(
        &root,
        &["TASK-7".to_string()],
        std::time::Duration::ZERO,
    );
    assert!(scan.scan_incomplete, "{scan:?}");
    assert!(!scan.corroborated(), "{scan:?}");
    let line = render_toon_linkage(&scan);
    assert!(line.starts_with("shipped=unknown "), "{line}");
    assert!(line.contains("reason=scan-timeout"), "{line}");
    assert!(!line.contains("shipped=false"), "{line}");
    assert!(
        line.split_whitespace().all(|t| t.contains('=')),
        "every token is key=value: {line}"
    );
}

#[test]
fn corroborated_scan_with_partial_file_count_still_reads_shipped() {
    let linkage = DefaultBranchLinkage {
        scanned_refs: vec!["origin/main".into()],
        found_on: Some("origin/main".into()),
        commits: vec![("abc1234".into(), "fix: x (TASK-7)".into())],
        scan_incomplete: true,
        ..Default::default()
    };
    assert_eq!(
        render_toon_linkage(&linkage),
        "shipped=true commit=abc1234 commits=1 files=unknown reason=scan-timeout on=origin/main"
    );
}

#[test]
fn incomplete_scan_does_not_warn_on_completion() {
    let finished = DefaultBranchLinkage {
        scanned_refs: vec!["origin/main".into()],
        ..Default::default()
    };
    assert!(linkage_warrants_warning(&finished));
    let timed_out = DefaultBranchLinkage {
        scan_incomplete: true,
        ..finished
    };
    assert!(!linkage_warrants_warning(&timed_out));
}
