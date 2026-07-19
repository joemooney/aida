use super::*;

fn git(root: &std::path::Path, args: &[&str]) {
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
}

/// TASK-140: the squash subject must come from the PR BRANCH's head, not the
/// local cwd HEAD. Here cwd HEAD is `main` (an unrelated subject) while the
/// PR branch carries the real one — the function must return the branch's.
#[test]
fn branch_head_commit_message_reads_branch_not_cwd_head() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "t@t.t"]);
    git(root, &["config", "user.name", "t"]);
    std::fs::write(root.join("a"), "1").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "unrelated main-tip subject"]);
    // Feature branch with the REAL subject.
    git(root, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(root.join("b"), "2").unwrap();
    git(root, &["add", "."]);
    git(
        root,
        &["commit", "-q", "-m", "the real PR subject (TASK-575)"],
    );
    // Back on main: cwd HEAD is now the unrelated tip — the bug condition.
    git(root, &["checkout", "-q", "main"]);

    let msg = branch_head_commit_message(root, "feature").expect("branch head readable");
    assert!(
        msg.contains("the real PR subject"),
        "must read the branch's commit, got: {msg}"
    );
    assert!(
        !msg.contains("unrelated main-tip"),
        "must NOT read cwd HEAD (main), got: {msg}"
    );
    // Unknown branch (no local, no origin) → None.
    assert!(branch_head_commit_message(root, "no-such-branch").is_none());
}
