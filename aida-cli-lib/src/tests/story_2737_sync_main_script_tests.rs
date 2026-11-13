//! STORY-2737: `scripts/integration-sync-main.sh` against a real hub.
// trace:STORY-2737 | ai:claude

use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .current_dir(dir)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// Promotion-4 review: with more main commits than the pipe buffer holds,
// `git log | head -20` died of SIGPIPE (exit 141) under `set -o pipefail`
// and the sync aborted silently, even in --dry-run.
// trace:STORY-2737 | ai:claude
#[cfg(unix)]
#[test]
fn story_2737_sync_dry_run_survives_a_long_independent_main() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/integration-sync-main.sh");
    let tmp = tempfile::tempdir().unwrap();
    let hub = tmp.path().join("hub.git");
    let work = tmp.path().join("work");
    git(tmp.path(), &["init", "-q", "--bare", hub.to_str().unwrap()]);
    git(
        tmp.path(),
        &["init", "-q", "-b", "main", work.to_str().unwrap()],
    );
    git(&work, &["remote", "add", "origin", hub.to_str().unwrap()]);
    git(&work, &["commit", "-q", "--allow-empty", "-m", "init"]);
    git(&work, &["push", "-q", "origin", "main"]);
    git(&work, &["checkout", "-q", "-b", "integration"]);
    git(&work, &["push", "-q", "origin", "integration"]);
    git(&work, &["checkout", "-q", "main"]);
    // 130 content commits with ~700-byte subjects: ~90 KiB of log output.
    let long = "x".repeat(700);
    for n in 0..130 {
        std::fs::write(work.join("f"), n.to_string()).unwrap();
        git(&work, &["add", "f"]);
        git(&work, &["commit", "-q", "-m", &format!("c{n} {long}")]);
    }
    git(&work, &["push", "-q", "origin", "main"]);
    git(&work, &["checkout", "-q", "integration"]);
    let out = Command::new("bash")
        .arg(&script)
        .arg("--dry-run")
        .current_dir(&work)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "exit {:?}\n{stdout}\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("real merge: main"), "{stdout}");
    assert_eq!(
        stdout.lines().filter(|l| l.contains(&long)).count(),
        20,
        "{stdout}"
    );
    assert!(stdout.contains("(dry run) would run: git merge --no-ff"));
}
