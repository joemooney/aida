use super::*;
use tempfile::TempDir;

fn init_repo() -> TempDir {
    let tmp = TempDir::new().unwrap();
    crate::test_git::git_command()
        .arg("-C")
        .arg(tmp.path())
        .args(["init", "--initial-branch=main", "--quiet"])
        .status()
        .unwrap();
    // Need at least one commit so subsequent branch creates work.
    crate::test_git::git_command()
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
    crate::test_git::git_command()
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

// trace:BUG-2451 | ai:codex
#[test]
fn auto_branch_refuses_collision_without_allocating_suffix() {
    let tmp = init_repo();
    create_branch(tmp.path(), "epic-20");
    let err = resolve_session_branch(tmp.path(), "epic-20", "auto").unwrap_err();
    assert!(err.to_string().contains("pass --branch explicitly"));
    assert!(!branch_exists_anywhere(tmp.path(), "epic-20-2"));
}

// trace:BUG-2451 | ai:codex
#[test]
fn explicit_date_style_permits_collision_suffix() {
    let tmp = init_repo();
    let dated = format!("epic-20-{}", chrono::Local::now().format("%Y-%m-%d"));
    create_branch(tmp.path(), &dated);
    assert_eq!(
        resolve_session_branch(tmp.path(), "epic-20", "date").unwrap(),
        format!("{dated}-2")
    );
}

// trace:BUG-2451 | ai:codex
#[test]
fn bug_2451_pickup_pins_existing_pr_even_without_a_seat() {
    let tmp = init_repo();
    create_branch(tmp.path(), "original-pr");
    let mut forge = crate::forge::fake::RecordingForge::new();
    forge.open_for_spec = crate::forge::ChangeLookup::Found(crate::forge::ChangeRef {
        id: 42,
        url: "https://example.invalid/42".into(),
        branch: "original-pr".into(),
        base: "main".into(),
        title: None,
    });
    assert_eq!(
        pickup_pr_branch(&forge, "BUG-2451", None).unwrap(),
        Some((42, "original-pr".into()))
    );
    assert!(pickup_pr_branch(&forge, "BUG-2451", Some("bug-2451-2")).is_err());
    let holder = tmp.path().join("holder");
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(tmp.path())
        .args(["worktree", "add"])
        .arg(&holder)
        .arg("original-pr")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (_, branch) = pickup_pr_branch(&forge, "BUG-2451", None).unwrap().unwrap();
    let err = crate::queue_cmd::preflight_queue_work_branch(tmp.path(), Some(&branch)).unwrap_err();
    assert!(err.to_string().contains(&holder.display().to_string()));
    assert!(!branch_exists_anywhere(tmp.path(), "bug-2451-2"));
}

// trace:BUG-2451 | ai:codex
#[test]
fn bug_2451_unknown_or_missing_head_never_allocates() {
    let mut forge = crate::forge::fake::RecordingForge::new();
    for lookup in [
        crate::forge::ChangeLookup::CliMissing,
        crate::forge::ChangeLookup::CliFailed("auth".into()),
        crate::forge::ChangeLookup::Unreachable("offline".into()),
        crate::forge::ChangeLookup::Found(crate::forge::ChangeRef {
            id: 42,
            url: String::new(),
            branch: String::new(),
            base: "main".into(),
            title: None,
        }),
    ] {
        forge.open_for_spec = lookup;
        assert!(pickup_pr_branch(&forge, "BUG-2451", None).is_err());
    }
    forge.open_for_spec = crate::forge::ChangeLookup::NoChange;
    assert_eq!(pickup_pr_branch(&forge, "BUG-2451", None).unwrap(), None);
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
