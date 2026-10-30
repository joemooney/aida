//! BUG-HOME-033: the post-ship merged-worktree GC must not `git status` every
//! registered worktree. It settles "is this branch merged?" from local refs
//! first, skips the unmerged ones, and stops at a time budget.
// trace:BUG-B6B1278890774EF993D2EBC327862470-033 | ai:claude

use std::path::{Path, PathBuf};

use crate::doctor_cmd::{post_ship_gc_summary, AgentWorktreeScanLimits};

fn git(root: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
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

const UNMERGED: usize = 12;

/// A repo with one merged agent worktree (its branch is an ancestor of main)
/// and `UNMERGED` agent worktrees each carrying an unmerged commit AND an
/// uncommitted file. A full scan has to `git status` the unmerged ones (and
/// flags each as dirty); a merged-only scan must never look at them.
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    commit_file(&root, "README.md", "base\n", "init");
    git(&root, &["branch", "-M", "main"]);
    let base = root.join(".claude/worktrees");
    std::fs::create_dir_all(&base).unwrap();

    let merged = base.join("agent-merged");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "worktree-agent-merged-1000",
            merged.to_str().unwrap(),
            "main",
        ],
    );
    commit_file(&merged, "merged.txt", "m\n", "feat: merged (TASK-1000)");
    git(
        &root,
        &["merge", "-q", "--ff-only", "worktree-agent-merged-1000"],
    );

    for i in 0..UNMERGED {
        let wt = base.join(format!("agent-open-{i}"));
        let branch = format!("worktree-agent-open-{}", 2000 + i);
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &branch,
                wt.to_str().unwrap(),
                "main",
            ],
        );
        commit_file(
            &wt,
            "open.txt",
            "o\n",
            &format!("feat: open (TASK-{})", 2000 + i),
        );
        std::fs::write(wt.join("scratch.txt"), "uncommitted\n").unwrap();
    }
    (tmp, root)
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-033.acbe47d8 | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-033.ac7e8240 | ai:claude
#[test]
fn merged_only_scan_never_status_scans_unmerged_worktrees() {
    let (_tmp, root) = fixture();

    // Baseline: the full doctor scan reaches every unmerged tree and sees its
    // uncommitted file — proof that a status probe there would observe it.
    let full = crate::doctor_cmd::scan_merged_agent_worktrees_with_count(&root);
    let dirty_flags = full
        .findings
        .iter()
        .filter(|f| f.summary.contains("uncommitted changes"))
        .count();
    assert_eq!(dirty_flags, UNMERGED, "full scan: {:?}", full.findings);

    let limited = crate::doctor_cmd::scan_merged_agent_worktrees_limited(
        &root,
        AgentWorktreeScanLimits {
            merged_only: true,
            deadline: None,
        },
    );
    assert_eq!(limited.skipped_unmerged, UNMERGED);
    assert_eq!(limited.deferred, 0);
    assert_eq!(limited.reclaimable_count, 1, "{:?}", limited.findings);
    assert_eq!(
        limited.findings.len(),
        1,
        "only the merged worktree is examined: {:?}",
        limited.findings
    );
    assert!(limited.findings[0].id.ends_with("agent-merged"));
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-033.ac152090 | ai:claude
#[test]
fn spent_budget_defers_every_worktree_to_doctor() {
    let (_tmp, root) = fixture();
    let limited = crate::doctor_cmd::scan_merged_agent_worktrees_limited(
        &root,
        AgentWorktreeScanLimits {
            merged_only: true,
            deadline: Some(std::time::Instant::now()),
        },
    );
    assert!(limited.findings.is_empty());
    assert_eq!(limited.reclaimable_count, 0);
    // Main checkout + the merged tree + every unmerged tree.
    assert_eq!(limited.deferred, UNMERGED + 2);
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-033.ac152090 | ai:claude
#[test]
fn summary_names_the_doctor_follow_up_when_deferred() {
    let done = post_ship_gc_summary(1, 12, 0, 0);
    assert!(
        done.contains("1 reclaimable, 12 without a local merge signal skipped"),
        "{done}"
    );
    assert!(!done.contains("budget"), "{done}");

    let deferred = post_ship_gc_summary(0, 3, 40, 0);
    assert!(deferred.contains("60s budget"), "{deferred}");
    assert!(
        deferred.contains("40 worktree(s) not examined"),
        "{deferred}"
    );
    assert!(!deferred.contains("not removed"), "{deferred}");
    assert!(
        deferred.contains("aida doctor --category merged-agent-worktrees"),
        "{deferred}"
    );

    let unremoved = post_ship_gc_summary(5, 0, 2, 4);
    assert!(
        unremoved.contains("2 worktree(s) not examined and 4 reclaimable worktree(s) not removed"),
        "{unremoved}"
    );
}

fn reclaimable_finding(id: &str) -> crate::doctor_root::DoctorFinding {
    crate::doctor_root::DoctorFinding {
        category: "merged-agent-worktrees".into(),
        id: id.into(),
        summary: "merged".into(),
        action: "remove".into(),
        // Safe so the heal disposition proceeds in a TTY-less test, and no
        // `--force`, so a heal that did run would only skip, never remove.
        safe_heal: true,
    }
}

/// The heal pass that removes what the scan found runs under the same
/// deadline: once it passes, every remaining removal is deferred to `aida
/// doctor` instead of run.
// trace:BUG-B6B1278890774EF993D2EBC327862470-033.ac152090 | ai:claude
#[test]
fn spent_budget_defers_every_heal_to_doctor() {
    let tmp = tempfile::tempdir().unwrap();
    let findings = vec![reclaimable_finding("wt-a"), reclaimable_finding("wt-b")];
    let opts = super::DoctorRunOptions {
        heal: true,
        yes: true,
        ..Default::default()
    };

    let healed = super::heal_doctor_findings(
        tmp.path(),
        &findings,
        &opts,
        Some(std::time::Instant::now()),
    )
    .unwrap();
    assert_eq!(healed.len(), 2, "{healed:?}");
    for result in &healed {
        assert_eq!(result.status, super::HEAL_DEFERRED, "{result:?}");
        assert!(
            result
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("aida doctor --category merged-agent-worktrees"),
            "{result:?}"
        );
    }

    let unbounded = super::heal_doctor_findings(tmp.path(), &findings, &opts, None).unwrap();
    assert!(
        unbounded.iter().all(|r| r.status != super::HEAL_DEFERRED),
        "{unbounded:?}"
    );
}

/// `--no-cleanup` skips step 6 entirely: neither the session reap nor the
/// merged-worktree pruning runs, and the skip is printed.
// trace:BUG-B6B1278890774EF993D2EBC327862470-033.acac633f | ai:claude
#[test]
fn no_cleanup_skips_the_post_merge_reap_and_pruning() {
    use std::cell::RefCell;

    let calls = RefCell::new(Vec::new());
    let mut out = Vec::new();
    crate::pr_cmd::run_post_merge_worktree_gc(
        true,
        true,
        || {
            calls.borrow_mut().push("reap");
            Ok(())
        },
        || {
            calls.borrow_mut().push("prune");
            Ok(())
        },
        &mut out,
    );
    assert!(calls.borrow().is_empty(), "{:?}", calls.borrow());
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains("skipping the session reap and merged agent worktree pruning (--no-cleanup)"),
        "{text}"
    );

    let mut out = Vec::new();
    crate::pr_cmd::run_post_merge_worktree_gc(
        true,
        false,
        || {
            calls.borrow_mut().push("reap");
            Ok(())
        },
        || {
            calls.borrow_mut().push("prune");
            Ok(())
        },
        &mut out,
    );
    assert_eq!(*calls.borrow(), ["reap", "prune"]);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("up to 60s; skip with --no-cleanup"), "{text}");

    // No merge this run: nothing to clean up, nothing printed.
    let mut out = Vec::new();
    crate::pr_cmd::run_post_merge_worktree_gc(
        false,
        false,
        || panic!("reap must not run without a merge"),
        || panic!("prune must not run without a merge"),
        &mut out,
    );
    assert!(out.is_empty());
}

/// The ship prints its step-3 merge result before step 6 starts cleanup, so a
/// slow cleanup never hides whether the PR merged. `pr ship` drives a live
/// forge, so this pins the order in the ship body itself.
// trace:BUG-B6B1278890774EF993D2EBC327862470-033.acac633f | ai:claude
#[test]
fn merge_result_prints_before_post_merge_cleanup() {
    let source = include_str!("../pr_cmd.rs");
    let body_start = source
        .find("pub(crate) fn pr_ship_with_method_and_dispatch_ctx(")
        .expect("ship body");
    let body = &source[body_start..];
    let merged_line = body
        .find("\"  {} merged PR-{}\"")
        .expect("step-3 merge line");
    let cleanup = body
        .find("run_post_merge_worktree_gc(")
        .expect("step-6 cleanup call");
    assert!(
        merged_line < cleanup,
        "the merge result must print before post-merge cleanup starts"
    );
}
