// trace:BUG-2733 | ai:claude
use super::*;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .current_dir(repo)
        .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn commit_file(repo: &Path, name: &str, message: &str) -> String {
    std::fs::write(repo.join(name), name).unwrap();
    git(repo, &["add", name]);
    git(repo, &["commit", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

/// Branch: two old commits (cherry-picked onto main), main merged, one new.
fn promotion_shaped() -> (tempfile::TempDir, String) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    git(repo, &["init", "-b", "main"]);
    commit_file(repo, "base", "base");
    git(repo, &["switch", "-c", "promote/integration-9"]);
    let old_a = commit_file(repo, "a", "[AI:claude] fix(a): old a (BUG-1) (#11)");
    let old_b = commit_file(repo, "b", "[AI:claude] feat(b): old b (STORY-2) (#12)");
    git(repo, &["switch", "main"]);
    commit_file(repo, "unrelated", "unrelated main work (TASK-7)");
    git(repo, &["cherry-pick", &old_a]);
    git(repo, &["cherry-pick", &old_b]);
    git(repo, &["switch", "promote/integration-9"]);
    git(repo, &["merge", "--no-ff", "-m", "merge main", "main"]);
    commit_file(repo, "new", "[AI:claude] fix(new): new work (BUG-3) (#13)");
    let head = git(repo, &["rev-parse", "HEAD"]);
    (tmp, head)
}

#[test]
fn bug_2733_promotion_branch_resolves_only_the_new_commits_spec() {
    let (tmp, head) = promotion_shaped();
    let repo = tmp.path();
    let landed = already_on_base(repo, "main", &head).unwrap();
    assert_eq!(landed.len(), 2, "both old squashes are on main: {landed:?}");

    let body = crate::identity_gate::squash_commit_messages_body(repo, "main", &head).unwrap();
    assert!(
        !body.contains("BUG-1") && !body.contains("STORY-2"),
        "{body}"
    );
    let ids = crate::pr_train::member_spec_ids(
        "promote/integration-9",
        "promotion 9",
        "promotion 9 (#20)",
        &body,
    );
    assert_eq!(ids, ["BUG-3"]);
}

#[test]
fn bug_2733_branch_without_landed_history_keeps_every_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    git(repo, &["init", "-b", "main"]);
    commit_file(repo, "base", "base");
    git(repo, &["switch", "-c", "work"]);
    commit_file(repo, "one", "one (BUG-4)");
    commit_file(repo, "two", "two (BUG-5)");
    let head = git(repo, &["rev-parse", "HEAD"]);
    assert!(already_on_base(repo, "main", &head).unwrap().is_empty());
    let body = crate::identity_gate::squash_commit_messages_body(repo, "main", &head).unwrap();
    assert!(body.contains("BUG-4") && body.contains("BUG-5"), "{body}");
    // An unreadable history keeps every item.
    let mut items = vec!["abc".to_string()];
    retain_new(repo, "no-such-base", "HEAD", &mut items, String::as_str);
    assert_eq!(items, ["abc"]);
}

/// p38: main plus the bundle's squashes cherry-picked, no merges.
#[test]
fn bug_2733_linear_promotion_establishes_its_review_boundary() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    git(repo, &["init", "-b", "main"]);
    commit_file(repo, "base", "base");
    commit_file(
        repo,
        "prior",
        "promotion 37 [promotion 37] (Refs STORY-9) (#40)",
    );
    git(repo, &["switch", "-c", "promote/integration-38"]);
    commit_file(repo, "x", "[AI:claude] fix(x): x (BUG-21) (#41)");
    commit_file(
        repo,
        "y",
        "[AI:codex] feat(y): y slice (Refs STORY-22) (#42)",
    );
    let head = git(repo, &["rev-parse", "HEAD"]);

    let coverage = crate::review_gates::auto_queue_coverage(
        repo,
        "promote/integration-38",
        "promotion 38",
        "main",
        &head,
    )
    .unwrap();
    assert_eq!(coverage.delivered, ["BUG-21"]);
    assert_eq!(coverage.slice_refs, ["STORY-22"]);

    // A commit without a `(#N)` suffix is not a squash: still refused.
    commit_file(repo, "z", "local fixup (BUG-23)");
    let head = git(repo, &["rev-parse", "HEAD"]);
    let err = crate::review_gates::auto_queue_coverage(
        repo,
        "promote/integration-38",
        "promotion 38",
        "main",
        &head,
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("promotion boundary"), "{err:#}");
}
