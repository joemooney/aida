use super::*;

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init(root: &std::path::Path) {
    git(root, &["init", "-q", "-b", "aida-store"]);
    git(root, &["config", "user.email", "t@t.t"]);
    git(root, &["config", "user.name", "t"]);
    std::fs::write(root.join("a"), "1").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "A"]);
}

/// TASK-475: count commits origin/aida-store is ahead of local aida-store.
#[test]
fn behind_count_counts_origin_ahead() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    let a = git(root, &["rev-parse", "HEAD"]);
    for f in ["b", "c"] {
        std::fs::write(root.join(f), "x").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", f]);
    }
    // origin = C (2 ahead), local aida-store reset back to A.
    git(
        root,
        &["update-ref", "refs/remotes/origin/aida-store", "HEAD"],
    );
    git(root, &["update-ref", "refs/heads/aida-store", &a]);
    assert_eq!(orphan_store_behind_count(root), Some(2));
}

/// Up-to-date → Some(0); no origin ref → None (skip the nudge).
#[test]
fn behind_count_zero_and_none_cases() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init(root);
    // No origin ref yet → unknown.
    assert_eq!(orphan_store_behind_count(root), None);
    // Origin == local → 0.
    git(
        root,
        &["update-ref", "refs/remotes/origin/aida-store", "HEAD"],
    );
    assert_eq!(orphan_store_behind_count(root), Some(0));
}
