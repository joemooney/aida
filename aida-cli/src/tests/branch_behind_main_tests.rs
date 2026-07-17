use super::*;
use std::process::Command;
use tempfile::TempDir;

/// Init a repo with `main` and a feature branch in one of three
/// configurations: behind / ahead / diverged / equal.
fn fixture(feat_ahead: u32, main_ahead: u32) -> (TempDir, &'static str) {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path();
    let run = |args: &[&str]| {
        let r = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            r.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&r.stderr)
        );
    };
    run(&["init", "--initial-branch=main", "--quiet"]);
    run(&["commit", "--allow-empty", "-m", "root", "--quiet"]);
    run(&["checkout", "-q", "-b", "feat-x"]);
    for i in 0..feat_ahead {
        run(&[
            "commit",
            "--allow-empty",
            "-m",
            &format!("feat{}", i),
            "--quiet",
        ]);
    }
    run(&["checkout", "-q", "main"]);
    for i in 0..main_ahead {
        run(&[
            "commit",
            "--allow-empty",
            "-m",
            &format!("main{}", i),
            "--quiet",
        ]);
    }
    run(&["checkout", "-q", "feat-x"]);
    (tmp, "feat-x")
}

/// Feature branch equal-to main → None (no rebase needed).
#[test]
fn equal_to_main_returns_none() {
    let (tmp, branch) = fixture(0, 0);
    assert!(branch_behind_main(tmp.path(), branch).is_none());
}

/// Feature strictly ahead → None (push, don't rebase).
#[test]
fn ahead_of_main_returns_none() {
    let (tmp, branch) = fixture(3, 0);
    assert!(branch_behind_main(tmp.path(), branch).is_none());
}

/// Feature behind main → Some(count, samples).
#[test]
fn behind_main_returns_count() {
    let (tmp, branch) = fixture(0, 2);
    let result = branch_behind_main(tmp.path(), branch);
    assert!(result.is_some());
    let (count, sample) = result.unwrap();
    assert_eq!(count, 2);
    assert_eq!(sample.len(), 2);
}

/// Diverged (both ahead) → Some — origin still has stuff we don't.
#[test]
fn diverged_returns_count() {
    let (tmp, branch) = fixture(1, 3);
    let result = branch_behind_main(tmp.path(), branch);
    assert!(result.is_some());
    assert_eq!(result.unwrap().0, 3);
}

/// On main itself → None (nothing to rebase onto).
#[test]
fn on_main_returns_none() {
    let (tmp, _) = fixture(0, 1);
    assert!(branch_behind_main(tmp.path(), "main").is_none());
}

/// No `main` branch at all → None.
#[test]
fn missing_main_returns_none() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path();
    Command::new("git")
        .arg("-C")
        .arg(p)
        .args(["init", "--initial-branch=trunk", "--quiet"])
        .status()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(p)
        .args(["commit", "--allow-empty", "-m", "x", "--quiet"])
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap();
    assert!(branch_behind_main(p, "trunk").is_none());
}
