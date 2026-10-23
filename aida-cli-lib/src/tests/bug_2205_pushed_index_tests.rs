//! BUG-2205: a dead lease's worktree whose staged index reproduces a commit
//! already pushed to its branch is not dirty for `aida session end`; any
//! other change still is.
// trace:TASK-1955 | ai:claude (BUG-2205)

use super::staged_tree_is_pushed;
use std::path::Path;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn commit(dir: &Path, file: &str, body: &str) {
    std::fs::write(dir.join(file), body).unwrap();
    git(dir, &["add", file]);
    git(
        dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            file,
        ],
    );
}

fn entries(dir: &Path) -> Vec<String> {
    crate::status_sections::worktree_dirty_entries(dir)
}

#[test]
fn staged_index_equal_to_a_pushed_commit_is_clean_and_anything_else_is_not() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    let wt = tmp.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    git(
        tmp.path(),
        &["init", "-q", "--bare", "-b", "main", "remote.git"],
    );
    git(&wt, &["init", "-q", "-b", "pr-branch"]);
    git(&wt, &["remote", "add", "origin", remote.to_str().unwrap()]);
    commit(&wt, "a.txt", "a\n");
    commit(&wt, "b.txt", "b\n");
    git(&wt, &["push", "-q", "origin", "pr-branch"]);
    // The branch moved on (pushed b.txt) while this tree's HEAD stayed one
    // commit back with b.txt staged: the false-dirty shape.
    git(&wt, &["reset", "-q", "--soft", "HEAD~1"]);
    let staged = entries(&wt);
    assert!(!staged.is_empty(), "the fixture must read dirty");
    assert!(
        staged_tree_is_pushed(&wt, "pr-branch", &staged),
        "{staged:?}"
    );
    assert!(
        !staged_tree_is_pushed(&wt, "other-branch", &staged),
        "only the lease's own branch counts"
    );

    // An untracked file is work nobody pushed.
    std::fs::write(wt.join("new.txt"), "n\n").unwrap();
    assert!(!staged_tree_is_pushed(&wt, "pr-branch", &entries(&wt)));
    std::fs::remove_file(wt.join("new.txt")).unwrap();

    // An unstaged edit is too.
    std::fs::write(wt.join("a.txt"), "edited\n").unwrap();
    assert!(!staged_tree_is_pushed(&wt, "pr-branch", &entries(&wt)));
    git(&wt, &["checkout", "--", "a.txt"]);

    // A staged change beyond the pushed history is too.
    std::fs::write(wt.join("c.txt"), "c\n").unwrap();
    git(&wt, &["add", "c.txt"]);
    assert!(!staged_tree_is_pushed(&wt, "pr-branch", &entries(&wt)));
}
