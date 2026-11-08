#![cfg(target_os = "linux")]
// Real CLI transport, recovery discovery, and bounded slot reuse.
// trace:STORY-2662 | ai:codex
#[path = "support/test_git.rs"]
mod test_git;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn aida(cwd: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(cwd)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("AIDA_TELEMETRY", "0")
        .env("NO_COLOR", "1");
    for k in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_SESSION_ID",
        "AIDA_STORE_PATH",
        "AIDA_PROJECT_ROOT",
        "AIDA_USER",
    ] {
        cmd.env_remove(k);
    }
    cmd
}

fn show(o: &Output) -> String {
    format!(
        "exit {:?}\n--- stderr ---\n{}\n--- stdout ---\n{}",
        o.status.code(),
        String::from_utf8_lossy(&o.stderr),
        String::from_utf8_lossy(&o.stdout)
    )
}

fn git(repo: &Path, home: &Path, args: &[&str]) {
    let status = test_git::git_command()
        .current_dir(repo)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn precompact_handoff_cli_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &home, &["init", "-q", "-b", "main"]);
    git(&repo, &home, &["config", "user.email", "t@t.t"]);
    git(&repo, &home, &["config", "user.name", "t"]);
    git(&repo, &home, &["config", "commit.gpgsign", "false"]);
    git(
        &repo,
        &home,
        &["commit", "-q", "--allow-empty", "-m", "init"],
    );
    test_git::enroll_synthetic_home(&repo);
    let allocation = aida_core::allocation_authority::common_dir(&repo)
        .unwrap()
        .join(aida_core::allocation_authority::CONFIG_FILE);
    std::fs::set_permissions(allocation, std::fs::Permissions::from_mode(0o600)).unwrap();
    let init = aida(&repo, &home)
        .args([
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ])
        .output()
        .unwrap();
    assert!(init.status.success(), "{}", show(&init));
    let standing = aida(&repo, &home)
        .args([
            "session",
            "handoff",
            "--seat",
            "implementer",
            "--standing-orders",
            "add",
            "STANDING ONLY",
        ])
        .output()
        .unwrap();
    assert!(standing.status.success(), "{}", show(&standing));
    for generation in 0..3 {
        let mut child = aida(&repo, &home)
            .env("CLAUDE_CODE_SESSION_ID", format!("session-{generation}"))
            .args([
                "session",
                "handoff",
                "--seat",
                "implementer",
                "--snapshot",
                "--write",
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(format!("RECOVER generation {generation}").as_bytes())
            .unwrap();
        let written = child.wait_with_output().unwrap();
        assert!(written.status.success(), "{}", show(&written));
    }
    let slot = std::fs::read_dir(repo.join(".aida/handoff"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name();
    let snapshot_standing = aida(&repo, &home)
        .args([
            "session",
            "handoff",
            "--seat",
            slot.to_str().unwrap(),
            "--standing-orders",
            "add",
            "SNAPSHOT STANDING ONLY",
        ])
        .output()
        .unwrap();
    assert!(
        snapshot_standing.status.success(),
        "{}",
        show(&snapshot_standing)
    );
    let packet = aida(&repo, &home)
        .args([
            "session",
            "handoff",
            "--seat",
            "implementer",
            "--snapshot",
            "--show",
            "--body-only",
        ])
        .output()
        .unwrap();
    assert!(packet.status.success(), "{}", show(&packet));
    let packet = String::from_utf8_lossy(&packet.stdout);
    assert!(packet.contains("RECOVER generation 2"));
    assert!(!packet.contains("STANDING ONLY"));
    // Exact internal-seat reads must exercise body-only independently of routing.
    let direct = aida(&repo, &home)
        .args([
            "session",
            "handoff",
            "--seat",
            slot.to_str().unwrap(),
            "--show",
            "--body-only",
        ])
        .output()
        .unwrap();
    assert!(direct.status.success(), "{}", show(&direct));
    assert_eq!(direct.stdout, packet.as_bytes());
    let discover = aida(&repo, &home)
        .env("AIDA_SESSION_ROLE", "implementer")
        .env("CLAUDE_CODE_SESSION_ID", "relaunch")
        .args(["session", "handoff", "--show"])
        .output()
        .unwrap();
    assert!(discover.status.success(), "{}", show(&discover));
    let discover = String::from_utf8_lossy(&discover.stdout);
    assert!(discover.contains("RECOVER generation 2"));
    assert!(discover.contains("STANDING ONLY"));
    assert!(discover.contains("may be stale"));
    let dirs: Vec<_> = std::fs::read_dir(repo.join(".aida/handoff"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(dirs.len(), 1, "one reusable slot across native sessions");
    let files: Vec<_> = std::fs::read_dir(&dirs[0])
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(
        files.len(),
        3,
        "snapshot, latest and bounded diagnostic log"
    );
    let log = std::fs::read_to_string(dirs[0].join("log.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 1);
    let record: serde_json::Value = serde_json::from_str(log.trim()).unwrap();
    assert_eq!(record["event"], "compact-snapshot");
    // Subdirectories resolve to the same worktree slot on relaunch.
    let subdir = repo.join("nested");
    std::fs::create_dir(&subdir).unwrap();
    let nested = aida(&subdir, &home)
        .args([
            "session",
            "handoff",
            "--seat",
            "implementer",
            "--snapshot",
            "--show",
            "--body-only",
        ])
        .output()
        .unwrap();
    assert!(nested.status.success(), "{}", show(&nested));
    assert_eq!(nested.stdout, packet.as_bytes());
    // An actual sibling worktree must never receive this worktree's packet.
    let sibling = tmp.path().join("sibling");
    git(
        &repo,
        &home,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "sibling",
            sibling.to_str().unwrap(),
        ],
    );
    let other = aida(&sibling, &home)
        .args([
            "session",
            "handoff",
            "--seat",
            "implementer",
            "--snapshot",
            "--show",
            "--body-only",
        ])
        .output()
        .unwrap();
    assert!(other.status.success(), "{}", show(&other));
    assert!(other.stdout.is_empty());
    // Real write-boundary regression: primary checkout stays read-only while
    // only the lane worktree and private HOME are writable.
    let probe = Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--", "true"])
        .output();
    let base = aida(&sibling, &home);
    let mut bounded;
    let command = if probe.is_ok_and(|o| o.status.success()) {
        bounded = Command::new("bwrap");
        bounded
            .args(["--ro-bind", "/", "/", "--bind"])
            .arg(&sibling)
            .arg(&sibling)
            .arg("--bind")
            .arg(&home)
            .arg(&home)
            .arg("--")
            .arg(base.get_program())
            .current_dir(&sibling);
        for (key, value) in base.get_envs() {
            if let Some(value) = value {
                bounded.env(key, value);
            } else {
                bounded.env_remove(key);
            }
        }
        &mut bounded
    } else {
        eprintln!("SKIP boundary namespace: bwrap unavailable; local path assertions still run");
        bounded = base;
        &mut bounded
    };
    let mut child = command
        .args([
            "session",
            "handoff",
            "--seat",
            "implementer",
            "--snapshot",
            "--write",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"SIBLING RECOVERY")
        .unwrap();
    let written = child.wait_with_output().unwrap();
    assert!(written.status.success(), "{}", show(&written));
    assert_eq!(
        std::fs::read_dir(repo.join(".aida/handoff"))
            .unwrap()
            .count(),
        1
    );
    assert!(
        sibling.join(".aida/handoff").is_dir(),
        "snapshot must use the contained lane write boundary"
    );
    let recovered = aida(&sibling, &home)
        .env("AIDA_SESSION_ROLE", "implementer")
        .args(["session", "handoff", "--show"])
        .output()
        .unwrap();
    assert!(recovered.status.success(), "{}", show(&recovered));
    assert!(String::from_utf8_lossy(&recovered.stdout).contains("SIBLING RECOVERY"));
    git(
        &repo,
        &home,
        &["worktree", "remove", "--force", sibling.to_str().unwrap()],
    );
    let mut child = aida(&repo, &home)
        .args([
            "session",
            "handoff",
            "--seat",
            "implementer",
            "--snapshot",
            "--write",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"ROOT STILL LIVE")
        .unwrap();
    let written = child.wait_with_output().unwrap();
    assert!(written.status.success(), "{}", show(&written));
    assert_eq!(
        std::fs::read_dir(repo.join(".aida/handoff"))
            .unwrap()
            .count(),
        1,
        "worktree removal removes its local snapshot; primary slot remains"
    );
}
