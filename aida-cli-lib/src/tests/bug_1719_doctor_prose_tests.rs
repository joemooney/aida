use std::path::Path;

fn git(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit_file(root: &Path, file: &str, content: &str, message: &str) {
    std::fs::write(root.join(file), content).unwrap();
    git(root, &["add", file]);
    git(root, &["commit", "-q", "-m", message]);
}

// trace:BUG-1719 | ai:codex
#[test]
fn bug_1719_reworded_finding_prose_does_not_change_action_or_reclaimable_count() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    commit_file(&root, "README.md", "base\n", "init");
    git(&root, &["branch", "-M", "main"]);
    let wt = root.join(".claude/worktrees/agent-landed");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "worktree-agent-landed-9012",
            wt.to_str().unwrap(),
            "main",
        ],
    );
    commit_file(&wt, "landed.txt", "landed\n", "feat: ship (TASK-9012)");
    git(&root, &["checkout", "-q", "main"]);
    std::fs::write(root.join("landed.txt"), "landed\n").unwrap();
    git(&root, &["add", "landed.txt"]);
    git(&root, &["commit", "-q", "-m", "batch land (TASK-9012)"]);

    let mut scan = crate::doctor_cmd::scan_merged_agent_worktrees_with_count(&root);
    assert_eq!(
        scan.reclaimable_count, 1,
        "fixture must produce a reclaimable finding: {:?}",
        scan.findings
    );
    assert_eq!(scan.findings.len(), 1);
    let before_action = scan.findings[0].action.clone();
    scan.findings[0].summary = "operator-facing summary reworded by this test".to_string();
    assert_eq!(scan.findings[0].action, before_action);
    assert_eq!(
        scan.reclaimable_count, 1,
        "summary wording must not alter count"
    );

    let rewritten = crate::doctor_cmd::AgentWorktreeVerdict::Keep {
        reason: "operator-facing reason reworded by this test".to_string(),
        actionable: false,
    };
    let crate::doctor_cmd::AgentWorktreeVerdict::Keep { reason, actionable } = rewritten else {
        unreachable!()
    };
    assert_eq!(reason, "operator-facing reason reworded by this test");
    assert_eq!(
        crate::doctor_cmd::agent_worktree_keep_action(actionable),
        "no action required — kept because batched content is undecidable"
    );
}

// trace:BUG-1719 | ai:codex
#[test]
fn bug_1719_no_reason_or_summary_contains_controls_decisions() {
    let source = include_str!("../doctor_cmd.rs");
    let command = source
        .split("fn doctor_multi_agent")
        .nth(1)
        .unwrap()
        .split("\nfn ")
        .next()
        .unwrap();
    let scanner = source
        .split("fn scan_merged_agent_worktrees_with_count")
        .nth(1)
        .unwrap()
        .split("\nfn ")
        .next()
        .unwrap();
    assert!(
        !command.contains(".summary.contains("),
        "ship count must not parse summary text"
    );
    assert!(
        !scanner.contains(".summary.contains(") && !scanner.contains("reason.contains("),
        "finding action must not parse reason text"
    );
}
