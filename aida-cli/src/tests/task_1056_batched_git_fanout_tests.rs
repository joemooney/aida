use super::*;

fn git_run(root: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} spawn failed: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_stdout(root: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} spawn failed: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Build a repo with a `main` baseline plus N work branches each carrying a
/// distinct number of commits ahead of main, and synthesize matching
/// `refs/remotes/origin/*` for a subset (no network). Returns the work
/// branch names.
fn build_branchy_repo(root: &std::path::Path) -> Vec<String> {
    git_run(root, &["init", "-q", "-b", "main"]);
    git_run(root, &["config", "user.email", "t@t"]);
    git_run(root, &["config", "user.name", "t"]);
    git_run(root, &["commit", "--allow-empty", "-qm", "main root"]);

    let names = ["task-101", "story-202", "bug-303"];
    for (i, name) in names.iter().enumerate() {
        git_run(root, &["checkout", "-q", "-b", name, "main"]);
        // i+1 commits ahead of main, so each branch's ahead-count differs.
        for c in 0..=i {
            git_run(
                root,
                &[
                    "commit",
                    "--allow-empty",
                    "-qm",
                    &format!("{name} commit {c}"),
                ],
            );
        }
    }
    git_run(root, &["checkout", "-q", "main"]);
    // Synthesize remote-tracking refs for two of the three branches.
    for name in &names[..2] {
        let sha = git_stdout(root, &["rev-parse", name]);
        git_run(
            root,
            &["update-ref", &format!("refs/remotes/origin/{name}"), &sha],
        );
    }
    names.iter().map(|s| s.to_string()).collect()
}

// The batched local-branch tip-time map must equal a per-branch
// `git log -1 --format=%ct` — the exact value the recency filter read
// before. One `for-each-ref` replaces the per-branch `git log` loop.
#[test]
fn local_branch_commit_times_match_per_branch_log() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let names = build_branchy_repo(root);

    let map = collect_local_branch_commit_times(root);
    // Every branch (incl. main) is present — proves a single call enumerated
    // them all, not a per-branch probe that could miss one.
    assert!(map.contains_key("main"), "main missing: {map:?}");
    for name in &names {
        let want: i64 = git_stdout(root, &["log", "-1", "--format=%ct", name])
            .parse()
            .unwrap();
        assert_eq!(map.get(name).copied(), Some(want), "tip time for {name}");
    }
}

// The batched remote-branch name set membership must equal a per-branch
// `git rev-parse --verify --quiet origin/<branch>` — present where the
// ref exists, absent where it does not.
#[test]
fn remote_branch_set_matches_rev_parse_verify() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let names = build_branchy_repo(root);

    let set = collect_remote_branch_name_set(root);
    for name in &names {
        let verify_ok = std::process::Command::new("git")
            .current_dir(root)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("origin/{name}"),
            ])
            .output()
            .unwrap()
            .status
            .success();
        assert_eq!(
            set.contains(name),
            verify_ok,
            "remote membership disagrees with rev-parse for {name}"
        );
    }
    // The synthesized refs (first two) are present; the third is not.
    assert!(set.contains(&names[0]));
    assert!(set.contains(&names[1]));
    assert!(!set.contains(&names[2]));
}

// The batched ahead-count map must equal a per-branch
// `branch_ahead_of(.., branch, "main")` for every branch — the value the
// worktree + cleanup sections render. One `for-each-ref` replaces the
// per-branch `git rev-list --count` loop. (On git < 2.41 the helper returns
// an empty map and callers fall back, so only assert equivalence for the
// entries the batch actually produced.)
#[test]
fn branch_ahead_of_map_matches_per_branch_rev_list() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let names = build_branchy_repo(root);

    let map = collect_branch_ahead_of(root, "main");
    // On a modern git the batch covers every branch; assert it is non-empty
    // so we know the batched path (not just the fallback) is exercised here.
    assert!(
        !map.is_empty(),
        "expected the batched ahead-count path to populate on this git"
    );
    for name in &names {
        let per_branch = branch_ahead_of(root, name, "main");
        if let Some(batched) = map.get(name).copied() {
            assert_eq!(
                Some(batched),
                per_branch,
                "ahead count for {name} batched vs per-branch"
            );
        }
    }
    // The constructed ahead-counts (1, 2, 3) are recovered exactly.
    assert_eq!(map.get("task-101").copied(), Some(1));
    assert_eq!(map.get("story-202").copied(), Some(2));
    assert_eq!(map.get("bug-303").copied(), Some(3));
    assert_eq!(map.get("main").copied(), Some(0));
}
