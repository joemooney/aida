use super::*;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

// trace:BUG-1434 | ai:claude

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git invocation");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write_file(dir: &Path, name: &str, contents: &str) {
    std::fs::write(dir.join(name), contents).unwrap();
}

/// Builds a repo shaped like a real stale PR:
///
/// ```text
/// main:    base ── c1(c.txt) ── c2(b.txt)      <- refs/remotes/origin/main
///            \
/// feature:    f1(b.txt)                        <- checked out
/// ```
///
/// `origin/main` is set via a bare `git update-ref` rather than an actual
/// remote/fetch — the function under test only reads the ref, so faking it
/// this way keeps the test hermetic and fast.
fn build_stale_repo() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["checkout", "-q", "-b", "main"]);

    write_file(dir, "a.txt", "base\n");
    git(dir, &["add", "a.txt"]);
    git(dir, &["commit", "-q", "-m", "base"]);

    git(dir, &["checkout", "-q", "-b", "feature"]);
    write_file(dir, "b.txt", "feature change\n");
    git(dir, &["add", "b.txt"]);
    git(dir, &["commit", "-q", "-m", "feature touches b.txt"]);

    git(dir, &["checkout", "-q", "main"]);
    write_file(dir, "c.txt", "unrelated main change\n");
    git(dir, &["add", "c.txt"]);
    git(dir, &["commit", "-q", "-m", "main touches c.txt"]);

    write_file(dir, "b.txt", "main also edits the same file\n");
    git(dir, &["add", "b.txt"]);
    git(dir, &["commit", "-q", "-m", "main touches b.txt too"]);

    let head = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "main"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    git(
        dir,
        &["update-ref", "refs/remotes/origin/main", head.trim()],
    );

    git(dir, &["checkout", "-q", "feature"]);
    tmp
}

#[test]
fn commits_behind_counts_intervening_commits_on_origin_default() {
    let tmp = build_stale_repo();
    let n = commits_behind_default(tmp.path(), "feature", "main");
    assert_eq!(
        n,
        Some(2),
        "feature trails origin/main by exactly 2 commits"
    );
}

#[test]
fn commits_behind_is_zero_when_level_with_default() {
    let tmp = build_stale_repo();
    // feature and origin/main have diverged (each has commits the other
    // lacks), so land feature exactly on origin/main to make it current.
    git(tmp.path(), &["reset", "-q", "--hard", "origin/main"]);
    let n = commits_behind_default(tmp.path(), "feature", "main");
    assert_eq!(n, Some(0));
}

#[test]
fn commits_behind_is_none_when_origin_ref_is_unresolvable() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
    write_file(dir, "a.txt", "x\n");
    git(dir, &["add", "a.txt"]);
    git(dir, &["commit", "-q", "-m", "only commit"]);

    // No refs/remotes/origin/main exists at all in this repo.
    let n = commits_behind_default(dir, "HEAD", "main");
    assert_eq!(
        n, None,
        "unresolvable origin/<base> must yield None, never a false 0"
    );
}

#[test]
fn stale_base_file_overlap_names_files_both_sides_touch() {
    let tmp = build_stale_repo();
    let overlap = stale_base_file_overlap(tmp.path(), "feature", "main");
    assert_eq!(
        overlap,
        vec!["b.txt".to_string()],
        "only b.txt is touched by both feature's own commit and the intervening main commits"
    );
}

#[test]
fn stale_base_note_states_the_figure_and_overlap_when_behind() {
    let tmp = build_stale_repo();
    let note = stale_base_note(tmp.path(), "feature", "main").expect("behind ⇒ Some");
    assert!(note.contains("2 commits behind"), "note: {note}");
    assert!(note.contains("`main`"), "note: {note}");
    assert!(note.contains("b.txt"), "note: {note}");
}

/// Acceptance #5: a PR level with the default branch produces no extra
/// output — not even an "up to date" line.
#[test]
fn stale_base_note_is_none_when_level_with_default() {
    let tmp = build_stale_repo();
    git(tmp.path(), &["reset", "-q", "--hard", "origin/main"]);
    assert_eq!(stale_base_note(tmp.path(), "feature", "main"), None);
}

/// PRIN-5: absent evidence must never masquerade as good evidence. An
/// unresolvable ref renders as an explicit "unknown", never a silent
/// omission and never a false "0 commits behind".
#[test]
fn stale_base_note_says_unknown_rather_than_zero_when_uncomputable() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
    write_file(dir, "a.txt", "x\n");
    git(dir, &["add", "a.txt"]);
    git(dir, &["commit", "-q", "-m", "only commit"]);

    let note = stale_base_note(dir, "HEAD", "main").expect("uncomputable ⇒ still Some(text)");
    assert!(note.contains("unknown"), "note: {note}");
    assert!(
        !note.contains("0 commit"),
        "note must not read as a false zero: {note}"
    );
}

#[test]
fn git_diff_name_only_is_empty_best_effort_on_bad_refs() {
    let tmp = build_stale_repo();
    assert!(git_diff_name_only(tmp.path(), "no-such-ref", "main").is_empty());
}
