// `aida ps` end to end over a review-watch state fixture: the slots a
// superseded review still holds (TASK-2151) and each reviewed change's last
// ship-readiness report (TASK-1945 S3a), in `--json`, agent and human
// output. The state file is written by hand in the watcher's on-disk shape.
// trace:TASK-1945 | ai:claude
#![cfg(target_os = "linux")]

#[path = "support/test_git.rs"]
mod test_git;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

mod support;

fn aida(repo: &Path, home: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .env_remove("AIDA_PERMISSION_MODE")
        .env_remove("AIDA_AGENT_OUTPUT")
        .env_remove("AIDA_OUTPUT_FORMAT");
    if let Some(grant) = support::ensure_seat(home, repo, "advisor", &[]) {
        cmd.env("AIDA_SESSION_GRANT", grant);
    }
    cmd.args(args).output().expect("run aida")
}

fn git(repo: &Path, args: &[&str]) {
    assert!(test_git::git_command()
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git")
        .success());
}

fn init_repo() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    // trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
    test_git::enroll_synthetic_home(&repo);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Watch Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let init = aida(
        &repo,
        &home,
        &[
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
    );
    assert!(
        init.status.success(),
        "init: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    (tmp, repo, home)
}

const HELD_HEAD: &str = "1111111111111111111111111111111111111111";
const NEW_HEAD: &str = "2222222222222222222222222222222222222222";
const READY_HEAD: &str = "3333333333333333333333333333333333333333";

/// PR-5 held by a superseded review; PR-8 reported ready.
fn write_state(repo: &Path) {
    let dir = repo.join(".aida/review-watch");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let state = serde_json::json!({
        "version": 1,
        "repository": "github:github.com/owner/repo",
        "max_concurrent": 2,
        "entries": {
            "PR-5": {
                "attempts": [{
                    "head": HELD_HEAD,
                    "token": "6f1c1b7e-8a51-4a8e-9a3a-0d6b8e1f2c3d",
                    "vendor": null,
                    "phase": "superseding",
                    "identity": null,
                    "supersede": {
                        "by": NEW_HEAD,
                        "requested_at": "2026-10-22T07:00:00Z",
                        "diagnostic": "process 4242 in the review's process group 4242 is still running"
                    }
                }],
                "notices": {}
            },
            "PR-8": {
                "attempts": [],
                "notices": {},
                "readiness": {
                    "head": READY_HEAD,
                    "line": "ready",
                    "checked_at": "2026-10-22T07:30:00Z"
                }
            }
        }
    });
    let path = dir.join("state.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn stdout(out: &Output) -> String {
    assert!(
        out.status.success(),
        "aida ps failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn ps_renders_held_slots_and_last_readiness() {
    let (_tmp, repo, home) = init_repo();
    // No watcher state: both lists are empty, and ps creates none.
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&aida(&repo, &home, &["ps", "--json"]))).unwrap();
    assert_eq!(json["review_watch_held"], serde_json::json!([]));
    assert_eq!(json["review_watch_ready"], serde_json::json!([]));
    assert!(!repo.join(".aida/review-watch").exists());

    write_state(&repo);
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&aida(&repo, &home, &["ps", "--json"]))).unwrap();
    let held = json["review_watch_held"].as_array().unwrap();
    assert_eq!(held.len(), 1, "{json}");
    assert_eq!(held[0]["change"], "PR-5");
    let cause = held[0]["cause"].as_str().unwrap();
    assert!(
        cause.contains("superseded for head 222222222222") && cause.contains("process 4242"),
        "{cause}"
    );
    let ready = json["review_watch_ready"].as_array().unwrap();
    assert_eq!(ready.len(), 1, "{json}");
    assert_eq!(ready[0]["change"], "PR-8");
    assert_eq!(
        ready[0]["readiness"],
        "at 333333333333: ready (checked 2026-10-22 07:30 UTC)"
    );

    let agent = stdout(&aida(&repo, &home, &["ps", "--format", "toon"]));
    assert!(
        agent.contains("review_watch_held") && agent.contains("PR-5"),
        "{agent}"
    );
    assert!(
        agent.contains("review_watch_ready") && agent.contains("PR-8"),
        "{agent}"
    );

    let human = stdout(&aida(&repo, &home, &["ps", "--format", "human"]));
    let held_section = human
        .split("Review-watch slots held")
        .nth(1)
        .unwrap_or_else(|| panic!("no held section: {human}"));
    assert!(held_section.contains("PR-5"), "{human}");
    let ready_section = human
        .split("Review-watch readiness")
        .nth(1)
        .unwrap_or_else(|| panic!("no readiness section: {human}"));
    assert!(
        ready_section.contains("PR-8") && ready_section.contains("ready (checked"),
        "{human}"
    );
}
