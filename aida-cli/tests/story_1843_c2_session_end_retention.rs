// BUG-688: gated to Linux like every other binary-driving e2e suite here.
#![cfg(target_os = "linux")]
//! STORY-1843 C2 (C SKETCH section 6, rework F5 a): `aida session end` on a
//! worktree whose mail cannot be retired ends nothing. The lease (stamped
//! ending-retained), its manifest and claim, the tree and its mail stay; the
//! runtime links are put back; the lease delete and the BUG-1988 cleanup tail
//! never run. `aida session leases` reads the session as ending-retained,
//! and the next end, once the cause is gone, ends it normally.
// trace:STORY-1843 | ai:claude

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
    // Captured stdout is not a TTY, so unpinned `aida` runs in agent mode and
    // routes a refusal to STDOUT as a TOON `error:` block; the assertions read
    // the human STDERR channel, so pin it (the pin outranks AIDA_AGENT_OUTPUT).
    // trace:TASK-1955 | ai:claude (BUG-2205)
    cmd.env("AIDA_OUTPUT_FORMAT", "human");
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

    // trace:BUG-2017 | ai:codex — explicit positive HOME fixture.
    test_git::enroll_synthetic_home(&repo);
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

fn session_end(repo: &Path, home: &Path, id: &str) -> Output {
    aida(repo, home)
        .args(["session", "end", id, "--yes", "--skip-ci"])
        .output()
        .expect("run session end")
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn a_refused_session_end_ends_nothing_and_a_retry_ends_it() {
    let base = tempfile::tempdir().expect("tempdir");
    let base = base.path().canonicalize().expect("canonicalize");
    let (repo, home) = project(&base);
    let lease = start_session(&repo, &home, &add_spec(&repo, &home, "kept mail"));
    let mailbox = lease.worktree.join(".aida").join("mailbox");
    std::fs::create_dir_all(&mailbox).unwrap();
    std::fs::write(mailbox.join("m.json"), "{}").unwrap();
    let sessions = repo.join(".aida").join("sessions");
    let sessions_before = files_in(&sessions);
    let claims = repo.join(".aida-store").join("coordination").join("leases");
    let claims_before = files_in(&claims);
    assert!(
        lease.worktree.join(".aida").join("sessions").exists(),
        "fixture precondition: the runtime links are in place"
    );

    let out = session_end(&repo, &home, &lease.id);
    assert!(!out.status.success(), "the end must refuse: {}", show(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("was NOT ended"), "{}", show(&out));
    assert!(
        stderr.contains("aida mailbox retention activate"),
        "{}",
        show(&out)
    );

    // Nothing that ends the session ran.
    let lease_file = sessions.join(format!("{}.toml", lease.id));
    let body = std::fs::read_to_string(&lease_file).expect("the lease is kept");
    assert!(body.contains("ending_retained_at"), "{body}");
    for name in &sessions_before {
        assert!(
            sessions.join(name).exists(),
            "{name} (lease, manifest, activity) is kept: {}",
            show(&out)
        );
    }
    for name in &claims_before {
        assert!(claims.join(name).exists(), "the claim {name} is kept");
    }
    assert!(lease.worktree.is_dir(), "the tree is kept");
    assert!(mailbox.join("m.json").is_file(), "its mail is kept");
    assert!(
        lease.worktree.join(".aida").join("sessions").exists(),
        "the runtime links are restored: {}",
        show(&out)
    );

    // Every reader says ending-retained, not active.
    let leases = aida(&repo, &home)
        .args(["session", "leases", "--all"])
        .output()
        .expect("run session leases");
    assert!(
        String::from_utf8_lossy(&leases.stdout).contains("ending-retained"),
        "{}",
        show(&leases)
    );

    // Once the cause is gone, the next end retries and ends it.
    std::fs::remove_file(mailbox.join("m.json")).unwrap();
    let out = session_end(&repo, &home, &lease.id);
    assert!(
        out.status.success(),
        "the retry must end it: {}",
        show(&out)
    );
    assert!(!lease.worktree.exists(), "{}", show(&out));
    assert!(!lease_file.exists(), "{}", show(&out));
}
