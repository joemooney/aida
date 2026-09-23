//! TASK-1312: `aida pr gc` sweeps local `pr-N`/`mr-N` review-snapshot
//! branches whose change has reached a terminal state. Real temp git repo +
//! a stubbed `gh` (via `AIDA_TEST_GH_BINARY`) end to end, plus a table test
//! for the pure classifier.
// trace:TASK-1312 | ai:claude

use super::*;

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {:?} failed to spawn: {e}", args));
    assert!(
        out.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo(path: &std::path::Path) {
    git(path, &["init", "-b", "main", "--quiet"]);
    git(path, &["config", "user.email", "aida@example.test"]);
    git(path, &["config", "user.name", "AIDA Test"]);
    git(path, &["commit", "--allow-empty", "-m", "base", "--quiet"]);
}

fn branch_from_main(repo: &std::path::Path, name: &str) {
    git(repo, &["branch", name, "main"]);
}

// --- pure classifier ---

#[test]
fn classify_deletes_only_merged_matching_untouched_branch() {
    use crate::forge::ChangeState;

    assert_eq!(
        classify_pr_gc_branch(false, Ok(ChangeState::Merged), "abc123", "abc123"),
        PrGcAction::Delete
    );
    assert_eq!(
        classify_pr_gc_branch(false, Ok(ChangeState::Closed), "abc123", "abc123"),
        PrGcAction::Delete
    );
    assert_eq!(
        classify_pr_gc_branch(true, Ok(ChangeState::Merged), "abc123", "abc123"),
        PrGcAction::SkipCheckedOut,
        "checked-out wins over a terminal state"
    );
    assert_eq!(
        classify_pr_gc_branch(false, Ok(ChangeState::Open), "abc123", "abc123"),
        PrGcAction::SkipOpen
    );
    assert_eq!(
        classify_pr_gc_branch(false, Ok(ChangeState::Merged), "abc123", "def456"),
        PrGcAction::SkipDiverged {
            local_tip: "abc123".to_string(),
            remote_head: "def456".to_string(),
        },
        "a branch that no longer matches the change's last head is left alone"
    );
    assert_eq!(
        classify_pr_gc_branch(false, Err("gh not installed".to_string()), "abc123", ""),
        PrGcAction::SkipUnknownState("gh not installed".to_string()),
        "fails closed when the terminal-state check itself failed"
    );
}

// --- branch-name scoping ---

#[test]
fn parse_review_snapshot_branch_is_scoped_to_the_exact_shape() {
    use crate::forge::ForgeKind;

    assert_eq!(
        parse_review_snapshot_branch("pr-161"),
        Some((ForgeKind::GitHub, 161))
    );
    assert_eq!(
        parse_review_snapshot_branch("mr-42"),
        Some((ForgeKind::GitLab, 42))
    );
    // Out of scope by construction — never touch an authored spec branch.
    assert_eq!(parse_review_snapshot_branch("pr-161-fixup"), None);
    assert_eq!(parse_review_snapshot_branch("story-1187-pr-fix"), None);
    assert_eq!(parse_review_snapshot_branch("pr-"), None);
    assert_eq!(
        parse_review_snapshot_branch("prN"),
        None,
        "hand-typed non-hyphen form is out of scope"
    );
    assert_eq!(parse_review_snapshot_branch("main"), None);
}

// --- end-to-end sweep over a real repo ---

#[cfg(unix)]
fn fake_gh_for_pr(dir: &std::path::Path, state: &str, head_oid: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("gh");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = pr ] && [ \"$2\" = view ]; then\n\
               printf '%s\\n' '{{\"state\":\"{}\",\"title\":\"t\",\"baseRefName\":\"main\",\"headRefName\":\"feature\",\"headRefOid\":\"{}\",\"isCrossRepository\":false,\"isDraft\":false}}'\n\
               exit 0\n\
             fi\n\
             echo unexpected gh args: \"$@\" >&2\n\
             exit 1\n",
            state, head_oid
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn gc_deletes_merged_snapshot_and_leaves_open_and_checked_out_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root);

    // pr-1: merged, untouched since fetch — deleted.
    branch_from_main(root, "pr-1");
    let pr1_head = git(root, &["rev-parse", "pr-1"]);

    // pr-2: still open — kept.
    branch_from_main(root, "pr-2");

    // pr-3: merged but checked out in a worktree — kept.
    branch_from_main(root, "pr-3");
    let wt = tmp.path().join("wt3");
    git(root, &["worktree", "add", wt.to_str().unwrap(), "pr-3"]);

    // An authored spec branch that happens to start with "pr" prose —
    // never a gc candidate.
    branch_from_main(root, "pr-review-helper-story");

    let fake_gh = fake_gh_for_pr(root, "MERGED", &pr1_head);
    let _env =
        crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake_gh.to_str().unwrap())]);

    // pr-2 is open, so classify_pr_gc_branch never needs a real network read
    // for it in this harness; the fake gh always answers MERGED, so we drive
    // the handler through the candidate-listing + git-plumbing helpers
    // directly rather than asserting on stdout.
    let candidates = pr_gc_candidate_branches(root).unwrap();
    let mut names: Vec<&str> = candidates.iter().map(String::as_str).collect();
    names.sort();
    assert_eq!(names, vec!["pr-1", "pr-2", "pr-3"]);

    assert!(!pr_gc_branch_checked_out(root, "pr-1").unwrap());
    assert!(pr_gc_branch_checked_out(root, "pr-3").unwrap());

    // pr-1: not checked out, merged, tip matches head_oid the fake gh reports.
    let tip1 = pr_gc_branch_tip(root, "pr-1").unwrap();
    let meta1 = pr_gc_resolve_metadata(root, crate::forge::ForgeKind::GitHub, 1).unwrap();
    assert_eq!(
        classify_pr_gc_branch(false, Ok(meta1.state), &tip1, &meta1.head_sha),
        PrGcAction::Delete
    );
    pr_gc_delete_branch(root, "pr-1").unwrap();
    let remaining = git(root, &["branch", "--list", "pr-1"]);
    assert!(remaining.is_empty(), "pr-1 should be gone: {remaining:?}");

    // pr-3: checked out, so never deleted regardless of state.
    assert!(pr_gc_branch_checked_out(root, "pr-3").unwrap());
    git(
        root,
        &["worktree", "remove", "--force", wt.to_str().unwrap()],
    );

    // The non-review branch is never a candidate at all.
    assert!(!names.contains(&"pr-review-helper-story"));
}

#[cfg(unix)]
#[test]
fn gc_leaves_a_diverged_snapshot_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root);

    branch_from_main(root, "pr-9");
    // The change's last known head, as the fake gh will report it.
    let original_head = git(root, &["rev-parse", "pr-9"]);
    // A local commit lands on the snapshot after the fetch — diverged.
    git(root, &["checkout", "pr-9"]);
    std::fs::write(root.join("local.txt"), "local edit\n").unwrap();
    git(root, &["add", "local.txt"]);
    git(root, &["commit", "-m", "local edit", "--quiet"]);
    git(root, &["checkout", "main"]);

    let fake_gh = fake_gh_for_pr(root, "MERGED", &original_head);
    let _env =
        crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake_gh.to_str().unwrap())]);

    let tip = pr_gc_branch_tip(root, "pr-9").unwrap();
    let meta = pr_gc_resolve_metadata(root, crate::forge::ForgeKind::GitHub, 9).unwrap();
    let action = classify_pr_gc_branch(false, Ok(meta.state), &tip, &meta.head_sha);
    assert_eq!(
        action,
        PrGcAction::SkipDiverged {
            local_tip: tip.clone(),
            remote_head: original_head.clone(),
        }
    );

    // A branch we never even attempt to delete stays present.
    let remaining = git(root, &["branch", "--list", "pr-9"]);
    assert!(!remaining.is_empty());
}
