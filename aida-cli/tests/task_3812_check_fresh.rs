// trace:TASK-3812 | ai:codex

#[path = "support/test_git.rs"]
mod test_git;

use std::path::Path;
use std::process::{Command, Output};

// Public process codes consumed by scripts/lane-run.sh.
const EXIT_STALE: i32 = 10;
const EXIT_UNKNOWN: i32 = 3;

#[test]
fn check_fresh_reports_all_three_process_exit_codes() {
    let fixture = tempfile::tempdir().unwrap();
    let repo = fixture.path();

    let isolated = |cmd: &mut Command| {
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", repo)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("AIDA_TELEMETRY", "0")
            .current_dir(repo);
    };
    let aida = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
        isolated(&mut cmd);
        cmd.args(args).output().unwrap()
    };
    let git = |cwd: &Path, args: &[&str]| {
        let mut cmd = test_git::git_command();
        isolated(&mut cmd);
        stdout(
            cmd.current_dir(cwd)
                .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
                .args(args)
                .output()
                .unwrap(),
        )
    };

    // Use the commit stamped into this exact executable, even if the source
    // checkout moved since the build. The fixture borrows objects only.
    let version = stdout(aida(&["--version"]));
    let (_, stamp) = version.trim().rsplit_once(", sha ").unwrap();
    let stamp = stamp.strip_suffix(')').unwrap();
    let built = stamp.strip_suffix("+dirty").unwrap_or(stamp);
    let objects = git(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "objects",
        ],
    );
    git(repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join(".git/objects/info/alternates"), objects).unwrap();

    let assert_code = |state, code| {
        let out = aida(&[
            "dev",
            "check-fresh",
            "--repo",
            repo.to_str().unwrap(),
            "--max-behind",
            "0",
        ]);
        assert_eq!(
            out.status.code(),
            Some(code),
            "{state}: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    };

    assert_code("unknown without origin/main", EXIT_UNKNOWN);
    git(repo, &["update-ref", "refs/remotes/origin/main", built]);
    assert_code("fresh at the recorded build commit", 0);

    let tree = git(repo, &["rev-parse", &format!("{built}^{{tree}}")]);
    let next = git(
        repo,
        &["commit-tree", tree.trim(), "-p", built, "-m", "next"],
    );
    git(
        repo,
        &["update-ref", "refs/remotes/origin/main", next.trim()],
    );
    assert_code("stale one commit behind origin/main", EXIT_STALE);
}

fn stdout(out: Output) -> String {
    assert!(
        out.status.success(),
        "fixture command failed: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8(out.stdout).unwrap()
}
