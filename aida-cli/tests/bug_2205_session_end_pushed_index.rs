// BUG-688: gated to Linux like every other binary-driving e2e suite here.
// trace:BUG-688 | ai:claude
#![cfg(target_os = "linux")]
//! BUG-2205: `aida session end` must not refuse a worktree whose only change
//! is a staged index equal to a commit already on `origin/<branch>` (the
//! branch moved on from another worktree); that tree loses nothing. Any other
//! staged change still refuses.
//!
//! `staged_tree_is_pushed` has its own unit tests; this drives the real
//! binary so the call in `session end`'s dirty gate is pinned too.
// trace:TASK-1955 | ai:claude (BUG-2205)

#[path = "support/test_git.rs"]
mod test_git;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

/// Pin every ambient input a spawned `git` (directly or under `aida`) reads:
/// the temp HOME, no global/system git config, and no inherited `GIT_DIR` /
/// `GIT_WORK_TREE` / `GIT_INDEX_FILE`.
fn isolate(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home);
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    for k in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        cmd.env_remove(k);
    }
}

fn aida(cwd: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(cwd);
    isolate(&mut cmd, home);
    cmd.env("AIDA_TELEMETRY", "0");
    for k in [
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_PERMISSION_MODE",
        "AIDA_AGENT_OUTPUT",
        "AIDA_HEADLESS_VENDOR",
        "AIDA_AGENT_MODEL",
        "AIDA_AGENT_CMD",
        "AIDA_SESSION_ID",
        "AIDA_STORE_PATH",
        "AIDA_PROJECT_ROOT",
    ] {
        cmd.env_remove(k);
    }
    cmd
}

fn git_out(dir: &Path, home: &Path, args: &[&str]) -> String {
    let mut cmd = test_git::git_command();
    isolate(&mut cmd, home);
    let out = cmd.current_dir(dir).args(args).output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn show(o: &Output) -> String {
    format!(
        "exit {:?}\n--- stderr ---\n{}\n--- stdout ---\n{}",
        o.status.code(),
        String::from_utf8_lossy(&o.stderr),
        String::from_utf8_lossy(&o.stdout)
    )
}

fn is_spec_id(token: &str) -> bool {
    token.split_once('-').is_some_and(|(kind, num)| {
        !kind.is_empty()
            && !num.is_empty()
            && kind.chars().all(|c| c.is_ascii_uppercase())
            && num.chars().all(|c| c.is_ascii_digit())
    })
}

/// An initialized AIDA project (no worktree pool) with a bare `origin`, plus
/// an isolated HOME.
fn project(base: &Path) -> (PathBuf, PathBuf) {
    let repo = base.join("repo");
    let home = base.join("home");
    let remote = base.join("remote.git");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git_out(base, &home, &["init", "-q", "--bare", "remote.git"]);
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "t@t.t"],
        &["config", "user.name", "t"],
        &["config", "commit.gpgsign", "false"],
        &["commit", "-q", "--allow-empty", "-m", "init"],
        &["remote", "add", "origin", remote.to_str().unwrap()],
        &["push", "-q", "origin", "main"],
    ] {
        git_out(&repo, &home, args);
    }

    let init = aida(&repo, &home)
        .args([
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ])
        .output()
        .expect("run aida init");
    assert!(init.status.success(), "aida init failed: {}", show(&init));
    let cfg = repo.join(".aida").join("config.toml");
    let body = std::fs::read_to_string(&cfg).expect("read scaffolded config");
    const SCAFFOLD: &str = "[worktree_pool]\nenabled = true\n";
    assert!(body.contains(SCAFFOLD), "init scaffold changed:\n{body}");
    std::fs::write(
        &cfg,
        body.replace(SCAFFOLD, "[worktree_pool]\nenabled = false\n"),
    )
    .unwrap();
    (repo, home)
}

fn add_spec(repo: &Path, home: &Path, title: &str) -> String {
    let add = aida(repo, home)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env(
            "AIDA_SESSION_GRANT",
            support::ensure_seat(home, repo, "advisor", &[]).expect("fixture repo is initialized"),
        )
        .args([
            "add", "--type", "task", "--status", "approved", "--title", title,
        ])
        .output()
        .expect("run aida add");
    assert!(add.status.success(), "aida add failed: {}", show(&add));
    let out = String::from_utf8_lossy(&add.stdout);
    out.split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| is_spec_id(t))
        .unwrap_or_else(|| panic!("no spec id in:\n{out}"))
        .to_string()
}

struct Lease {
    id: String,
    worktree: PathBuf,
}

/// Start a worktree session for `spec` and return its lease.
fn start_session(repo: &Path, home: &Path, spec: &str) -> Lease {
    let run = aida(repo, home)
        .args(["queue", "work", spec, "--no-pull", "--no-launch"])
        .output()
        .expect("run queue work");
    assert!(run.status.success(), "queue work failed: {}", show(&run));
    let sessions = repo.join(".aida").join("sessions");
    for entry in std::fs::read_dir(&sessions).expect("sessions dir") {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.ends_with(".toml")
            || name.ends_with(".manifest.toml")
            || name.starts_with("mcp-claim.")
        {
            continue;
        }
        let Ok(v) = std::fs::read_to_string(&path)
            .unwrap()
            .parse::<toml::Value>()
        else {
            continue;
        };
        if v.get("scope").and_then(|s| s.as_str()) == Some(spec) {
            let lease = Lease {
                id: v["id"].as_str().unwrap().to_string(),
                worktree: PathBuf::from(v["worktree_path"].as_str().unwrap()),
            };
            assert!(lease.worktree.is_dir(), "worktree not created");
            return lease;
        }
    }
    panic!("no lease for {spec} under {}", sessions.display());
}

/// Commit `file` on the session branch, push it to `origin/<branch>`, then
/// soft-reset it away: the index alone now holds the pushed commit's tree.
fn stage_a_pushed_commit(lease: &Lease, home: &Path, file: &str) {
    let wt = &lease.worktree;
    let branch = git_out(wt, home, &["rev-parse", "--abbrev-ref", "HEAD"]);
    std::fs::write(wt.join(file), "pushed\n").unwrap();
    git_out(wt, home, &["add", file]);
    git_out(wt, home, &["commit", "-q", "-m", "pushed work"]);
    git_out(
        wt,
        home,
        &["push", "-q", "origin", &format!("HEAD:refs/heads/{branch}")],
    );
    git_out(wt, home, &["fetch", "-q", "origin"]);
    git_out(wt, home, &["reset", "-q", "--soft", "HEAD~1"]);
    assert_eq!(
        git_out(wt, home, &["status", "--porcelain", "--", file]),
        format!("A  {file}"),
        "fixture precondition: the pushed change is staged only"
    );
}

fn session_end(repo: &Path, home: &Path, id: &str) -> Output {
    aida(repo, home)
        .args(["session", "end", id, "--yes", "--skip-ci"])
        .output()
        .expect("run session end")
}

#[test]
fn session_end_treats_a_staged_index_equal_to_a_pushed_commit_as_clean() {
    let base = tempfile::tempdir().expect("tempdir");
    let base = base.path().canonicalize().expect("canonicalize");
    let (repo, home) = project(&base);

    // Only the pushed commit's tree is staged: the end goes through, says
    // why, and removes the worktree without --force.
    let pushed = start_session(&repo, &home, &add_spec(&repo, &home, "pushed index"));
    stage_a_pushed_commit(&pushed, &home, "pushed.txt");
    let out = session_end(&repo, &home, &pushed.id);
    assert!(
        out.status.success(),
        "end refused a pushed index: {}",
        show(&out)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("match a commit already pushed to origin/"),
        "the end must say why the staged tree is clean: {}",
        show(&out)
    );
    assert!(
        !pushed.worktree.exists(),
        "worktree left behind: {}",
        show(&out)
    );

    // One more staged file than the pushed commit holds: still refused, and
    // the tree is untouched.
    let extra = start_session(&repo, &home, &add_spec(&repo, &home, "extra index"));
    stage_a_pushed_commit(&extra, &home, "pushed-too.txt");
    std::fs::write(extra.worktree.join("unpushed.txt"), "not pushed\n").unwrap();
    git_out(&extra.worktree, &home, &["add", "unpushed.txt"]);
    let out = session_end(&repo, &home, &extra.id);
    assert!(
        !out.status.success(),
        "end must refuse a staged change that is not pushed: {}",
        show(&out)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("uncommitted changes"),
        "{}",
        show(&out)
    );
    assert!(
        extra.worktree.join("unpushed.txt").is_file(),
        "a refused end must leave the tree intact: {}",
        show(&out)
    );
}
