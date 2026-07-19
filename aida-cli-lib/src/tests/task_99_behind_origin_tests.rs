use super::*;

// trace:TASK-99 | ai:claude
#[test]
fn zero_behind_is_silent() {
    assert_eq!(behind_origin_warning(0, "main"), None);
}

// trace:TASK-99 | ai:claude
#[test]
fn one_behind_is_singular() {
    let msg = behind_origin_warning(1, "main").expect("warns when behind");
    assert!(msg.contains("1 commit behind origin/main"), "{msg}");
    assert!(!msg.contains("commits"), "singular form expected: {msg}");
    assert!(msg.contains("aida rebase"), "points at rebase: {msg}");
}

// trace:TASK-99 | ai:claude
#[test]
fn many_behind_is_plural() {
    let msg = behind_origin_warning(5, "main").expect("warns when behind");
    assert!(msg.contains("5 commits behind origin/main"), "{msg}");
}

// trace:TASK-99 | ai:claude
#[test]
fn names_the_supplied_branch() {
    let msg = behind_origin_warning(2, "develop").expect("warns when behind");
    assert!(msg.contains("origin/develop"), "{msg}");
}

/// Fully isolated: own tempdir, never touches the shared CWD or repo.
/// `origin/main` is absent in a bare local init, so the behind-count
/// helper returns `None` (stay silent on missing remote-tracking data)
// rather than erroring or warning spuriously. trace:TASK-99 | ai:claude
#[test]
fn no_origin_main_is_silent() {
    fn git(repo: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git on PATH");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = tmp.path();
    git(repo, &["init", "--initial-branch=main", "--quiet"]);
    git(repo, &["config", "user.email", "t@example.com"]);
    git(repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("f.txt"), "hi").unwrap();
    git(repo, &["add", "f.txt"]);
    git(repo, &["commit", "-m", "init", "--quiet"]);
    // No remote, so no origin/main → helper stays silent.
    assert_eq!(commits_behind_origin_main(repo, "main"), None);
}

/// Fully isolated: a local clone whose `main` is N commits behind its
/// origin counts exactly N behind. Own tempdir; no shared state.
// trace:TASK-99 | ai:claude
#[test]
fn counts_behind_against_origin_main() {
    fn git(repo: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git on PATH");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = tmp.path().join("origin");
    let clone = tmp.path().join("clone");
    std::fs::create_dir_all(&origin).unwrap();

    git(&origin, &["init", "--initial-branch=main", "--quiet"]);
    git(&origin, &["config", "user.email", "t@example.com"]);
    git(&origin, &["config", "user.name", "T"]);
    std::fs::write(origin.join("f.txt"), "0").unwrap();
    git(&origin, &["add", "f.txt"]);
    git(&origin, &["commit", "-m", "c0", "--quiet"]);

    // Clone — clone's main now matches origin/main (0 behind).
    git(
        tmp.path(),
        &[
            "clone",
            "--quiet",
            origin.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    git(&clone, &["config", "user.email", "t@example.com"]);
    git(&clone, &["config", "user.name", "T"]);
    assert_eq!(commits_behind_origin_main(&clone, "main"), Some(0));

    // Advance origin by 2 commits, fetch into the clone WITHOUT merging.
    for i in 1..=2 {
        std::fs::write(origin.join("f.txt"), i.to_string()).unwrap();
        git(&origin, &["add", "f.txt"]);
        git(&origin, &["commit", "-m", &format!("c{i}"), "--quiet"]);
    }
    git(&clone, &["fetch", "--quiet", "origin"]);
    // Clone's local main is now 2 behind origin/main.
    assert_eq!(commits_behind_origin_main(&clone, "main"), Some(2));
    // And the decision fn turns that into a plural warning.
    let behind = commits_behind_origin_main(&clone, "main").unwrap();
    assert!(behind_origin_warning(behind, "main")
        .unwrap()
        .contains("2 commits behind"));
}
