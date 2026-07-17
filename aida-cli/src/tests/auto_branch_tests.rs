use super::*;
use std::process::Command;
use tempfile::TempDir;

fn init_repo() -> TempDir {
    let tmp = TempDir::new().unwrap();
    Command::new("git")
        .arg("-C")
        .arg(tmp.path())
        .args(["init", "--initial-branch=main", "--quiet"])
        .status()
        .unwrap();
    // Need at least one commit so subsequent branch creates work.
    Command::new("git")
        .arg("-C")
        .arg(tmp.path())
        .args(["commit", "--allow-empty", "-m", "init", "--quiet"])
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap();
    tmp
}

fn create_branch(repo: &std::path::Path, name: &str) {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["branch", name])
        .status()
        .unwrap();
}

// First call returns the slug as-is. trace:STORY-65 | ai:claude
#[test]
fn auto_branch_slug_when_free() {
    let tmp = init_repo();
    let got = resolve_session_branch(tmp.path(), "epic-20", "auto").unwrap();
    assert_eq!(got, "epic-20");
}

// Slug taken → `-2`. trace:STORY-65 | ai:claude
#[test]
fn auto_branch_appends_2_on_first_collision() {
    let tmp = init_repo();
    create_branch(tmp.path(), "epic-20");
    let got = resolve_session_branch(tmp.path(), "epic-20", "auto").unwrap();
    assert_eq!(got, "epic-20-2");
}

/// Slug + slug-2..-10 all taken → falls through to dated form.
// trace:STORY-65 | ai:claude
#[test]
fn auto_branch_falls_back_to_dated_form() {
    let tmp = init_repo();
    create_branch(tmp.path(), "epic-20");
    for n in 2..=10 {
        create_branch(tmp.path(), &format!("epic-20-{}", n));
    }
    let got = resolve_session_branch(tmp.path(), "epic-20", "auto").unwrap();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    assert_eq!(got, format!("epic-20-{}", today));
}

/// `--branch-style date` skips the slug attempt and goes straight to
// the dated form. trace:STORY-65 | ai:claude
#[test]
fn date_branch_style_uses_date_form_even_when_slug_free() {
    let tmp = init_repo();
    let got = resolve_session_branch(tmp.path(), "epic-20", "date").unwrap();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    assert_eq!(got, format!("epic-20-{}", today));
}

// Unknown style errors clearly. trace:STORY-65 | ai:claude
#[test]
fn unknown_branch_style_errors() {
    let tmp = init_repo();
    let err = resolve_session_branch(tmp.path(), "epic-20", "wat").unwrap_err();
    assert!(err.to_string().contains("unknown --branch-style"));
}
