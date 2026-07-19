use super::*;

fn init_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Pin every config that a sandboxed CI shell might have set on the
    // global level — signing usually breaks first, so override here.
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.email", "t@x.example"],
        vec!["config", "user.name", "t"],
        vec!["config", "commit.gpgsign", "false"],
        vec!["config", "tag.gpgsign", "false"],
    ] {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(&args)
            .status()
            .unwrap()
            .success());
    }
    std::fs::write(root.join("a.txt"), b"a\n").unwrap();
    for args in [vec!["add", "a.txt"], vec!["commit", "-qm", "init"]] {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(&args)
            .status()
            .unwrap()
            .success());
    }
    tmp
}

#[test]
fn resolve_stack_base_returns_none_without_flags() {
    let tmp = init_repo();
    let out = resolve_stack_base(tmp.path(), tmp.path(), false, None, false).unwrap();
    assert_eq!(out, None);
}

#[test]
fn resolve_stack_base_refuses_unknown_base() {
    let tmp = init_repo();
    let err = resolve_stack_base(tmp.path(), tmp.path(), false, Some("nope"), false).unwrap_err();
    assert!(err.to_string().contains("nope"), "{err}");
    assert!(err.to_string().contains("does not exist"), "{err}");
}

#[test]
fn resolve_stack_base_accepts_existing_local_branch() {
    let tmp = init_repo();
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(tmp.path())
        .args(["branch", "task-x"])
        .status()
        .unwrap()
        .success());
    // No `gh` in the sandbox → detect_merged_pr_for_branch returns
    // GhMissing, which falls through the merged check.
    let out = resolve_stack_base(tmp.path(), tmp.path(), false, Some("task-x"), false).unwrap();
    assert_eq!(out.as_deref(), Some("task-x"));
}

#[test]
fn resolve_stack_base_force_keeps_through_safety_check() {
    // Same shape, but with --force-base set, the merged check is
    // skipped entirely. Asserts the flag wires through.
    let tmp = init_repo();
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(tmp.path())
        .args(["branch", "task-x"])
        .status()
        .unwrap()
        .success());
    let out = resolve_stack_base(tmp.path(), tmp.path(), false, Some("task-x"), true).unwrap();
    assert_eq!(out.as_deref(), Some("task-x"));
}

#[test]
fn resolve_stack_base_stack_with_no_leases_errors() {
    let tmp = init_repo();
    std::fs::create_dir_all(tmp.path().join(".aida").join("sessions")).unwrap();
    let err = resolve_stack_base(tmp.path(), tmp.path(), true, None, false).unwrap_err();
    assert!(err.to_string().contains("no un-merged"), "{err}");
}
