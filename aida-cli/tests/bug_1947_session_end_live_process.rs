#![cfg(target_os = "linux")]

//! BUG-1947: `aida session end` must not delete a worktree a live process is
//! still using. A stale-session sweep once salvage-committed and then removed
//! worktrees in which implementers were running cargo.
//!
//! Driven through the real binary against a throwaway repo with an isolated
//! HOME and no ambient git config, so lease discovery, the `/proc` scan, and
//! the user-visible refusal are all exercised.
// trace:BUG-1947 | ai:claude

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn hermetic(cmd: &mut Command) {
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
}

fn git(repo: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo).args(args);
    hermetic(&mut cmd);
    let status = cmd.status().expect("run git");
    assert!(
        status.success(),
        "git {args:?} failed in {}",
        repo.display()
    );
}

fn aida(cwd: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(cwd).env_clear();
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", home)
        .env("USER", "aida-bug-1947-test")
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_OUTPUT_FORMAT", "human")
        .env("NO_COLOR", "1");
    hermetic(&mut cmd);
    cmd
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn run_ok(mut cmd: Command, label: &str) -> Output {
    let out = cmd.output().unwrap_or_else(|e| panic!("run {label}: {e}"));
    assert!(out.status.success(), "{label} failed:\n{}", text(&out));
    out
}

/// Kills the fake live process on drop, so a failing assertion never leaks a
/// `sleep 600`.
struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Session {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    spec: String,
    id: String,
    worktree: PathBuf,
}

impl Session {
    /// A fresh repo with one approved bug and a live session (lease and
    /// worktree) on it.
    fn start() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().canonicalize().expect("canonical tempdir");
        let repo = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Live Process Test"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let mut init = aida(&repo, &home);
        init.args([
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ]);
        run_ok(init, "aida init");
        std::fs::write(
            repo.join(".aida/config.toml"),
            "[agents]\nenabled = [\"codex\"]\n",
        )
        .unwrap();

        let grant = support::ensure_seat_for(&home, &repo, "aida-bug-1947-test", "advisor", &[])
            .expect("fixture repo is initialized");
        let mut add = aida(&repo, &home);
        add.env("AIDA_SESSION_ROLE", "advisor")
            .env("AIDA_SESSION_GRANT", grant)
            .args([
                "add",
                "--type",
                "bug",
                "--status",
                "approved",
                "--mode",
                "operator",
                "--title",
                "live process fixture",
                "--description",
                "## Acceptance\n- fixture acceptance",
            ]);
        let out = run_ok(add, "aida add");
        let spec = String::from_utf8_lossy(&out.stdout)
            .split(|c: char| c.is_whitespace() || c == ',')
            .find(|t| t.starts_with("BUG-"))
            .unwrap_or_else(|| panic!("no spec id in {}", text(&out)))
            .to_string();

        let mut enter = aida(&repo, &home);
        enter.args(["worktree", "enter", &spec]);
        let out = run_ok(enter, "worktree enter");
        let shell = String::from_utf8_lossy(&out.stdout).into_owned();
        let worktree = shell
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("cd '")?
                    .strip_suffix('\'')
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| panic!("no worktree cd payload in:\n{shell}"));
        let id = shell
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("export AIDA_SESSION_ID='")?
                    .strip_suffix('\'')
                    .map(str::to_string)
            })
            .unwrap_or_else(|| panic!("no session id export in:\n{shell}"));
        assert!(worktree.is_dir(), "worktree {} missing", worktree.display());
        Session {
            _temp: temp,
            repo,
            home,
            spec,
            id,
            worktree,
        }
    }

    fn end(&self, cwd: &Path, extra: &[&str]) -> Output {
        let mut cmd = aida(cwd, &self.home);
        cmd.args(["session", "end", &self.id, "--yes", "--skip-ci"])
            .args(extra);
        cmd.output().expect("run session end")
    }

    fn lease_exists(&self) -> bool {
        self.repo
            .join(".aida/sessions")
            .join(format!("{}.toml", self.id))
            .exists()
    }

    /// A process whose cwd is inside the worktree, like a running cargo build.
    fn spawn_live_process(&self) -> KillOnDrop {
        KillOnDrop(
            Command::new("sleep")
                .arg("600")
                .current_dir(&self.worktree)
                .spawn()
                .expect("spawn sleep"),
        )
    }

    fn status_line(&self) -> String {
        let mut show = aida(&self.repo, &self.home);
        show.args(["show", &self.spec]);
        let out = run_ok(show, "aida show");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("status:"))
            .unwrap_or_default()
            .to_string()
    }
}

// (a)+(d): a live process inside the worktree blocks the end, names its pid
// and command, and leaves the lease and worktree untouched.
#[test]
fn session_end_refuses_while_a_live_process_is_inside_the_worktree() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let s = Session::start();
    let live = s.spawn_live_process();
    let pid = live.0.id();

    let out = s.end(&s.repo, &[]);
    let msg = text(&out);
    assert!(!out.status.success(), "session end must refuse:\n{msg}");
    assert!(msg.contains(&format!("pid {pid}")), "names the pid:\n{msg}");
    assert!(msg.contains("sleep 600"), "names the command:\n{msg}");
    assert!(msg.contains("--force"), "names the override:\n{msg}");
    assert!(
        !msg.contains("BUG-1947"),
        "no internal ids in user prose:\n{msg}"
    );
    assert!(s.worktree.is_dir(), "worktree must survive the refusal");
    assert!(s.lease_exists(), "lease must survive the refusal");
    drop(live);
}

// (a): `--force` overrides the refusal and removes the worktree.
// (c): releasing the lease does not move the spec back out of In Progress.
#[test]
fn session_end_force_removes_a_worktree_with_a_live_process_inside() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let s = Session::start();
    let before = s.status_line();
    assert!(
        before.to_ascii_lowercase().contains("progress"),
        "taking the worktree starts work: {before:?}"
    );
    let live = s.spawn_live_process();

    let out = s.end(&s.repo, &["--force"]);
    assert!(
        out.status.success(),
        "--force must proceed:\n{}",
        text(&out)
    );
    assert!(
        !s.worktree.exists(),
        "worktree must be removed under --force"
    );
    assert!(!s.lease_exists(), "lease released");
    assert_eq!(
        s.status_line(),
        before,
        "releasing the lease must not revert the spec status"
    );
    drop(live);
}

// No live process: removal proceeds as before, including when the end is run
// from inside the worktree itself (the caller's own process is exempt).
#[test]
fn session_end_without_live_processes_removes_the_worktree_even_from_inside_it() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let s = Session::start();
    let out = s.end(&s.worktree, &[]);
    assert!(
        out.status.success(),
        "clean end must succeed:\n{}",
        text(&out)
    );
    assert!(!s.worktree.exists(), "worktree removed");
    assert!(!s.lease_exists(), "lease released");
}
