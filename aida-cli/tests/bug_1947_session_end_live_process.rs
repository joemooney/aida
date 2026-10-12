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
    /// worktree) on it, taken with `worktree enter` (never pooled).
    fn start() -> Self {
        Self::start_with(false)
    }

    /// Like [`Session::start`], but through `session start --pool`, so the
    /// session runs in a warm-pool tree that `session end` RETURNS to the
    /// pool (a forced checkout plus a hard reset) instead of removing.
    fn start_pooled() -> Self {
        Self::start_with(true)
    }

    fn start_with(pooled: bool) -> Self {
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

        if pooled {
            let mut start = aida(&repo, &home);
            start
                .env("AIDA_SUPPRESS_SESSION_NEXT", "1")
                .args(["session", "start", "--owns", &spec, "--pool"]);
            let out = run_ok(start, "session start --pool");
            let id = String::from_utf8_lossy(&out.stdout)
                .lines()
                .find_map(|l| {
                    l.trim()
                        .strip_prefix("export AIDA_SESSION_ID='")?
                        .strip_suffix('\'')
                        .map(str::to_string)
                })
                .unwrap_or_else(|| panic!("no session id export in:\n{}", text(&out)));
            let lease =
                std::fs::read_to_string(repo.join(".aida/sessions").join(format!("{id}.toml")))
                    .expect("lease written");
            let worktree = lease
                .lines()
                .find_map(|l| {
                    l.trim()
                        .strip_prefix("worktree_path = \"")?
                        .strip_suffix('"')
                        .map(PathBuf::from)
                })
                .unwrap_or_else(|| panic!("no worktree_path in lease:\n{lease}"));
            assert!(worktree.is_dir(), "worktree {} missing", worktree.display());
            return Session {
                _temp: temp,
                repo,
                home,
                spec,
                id,
                worktree,
            };
        }

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

    /// The worktree's current branch (`HEAD` when detached).
    fn worktree_branch(&self) -> String {
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.worktree)
            .args(["rev-parse", "--abbrev-ref", "HEAD"]);
        hermetic(&mut cmd);
        let out = cmd.output().expect("run git rev-parse");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
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
    assert!(
        msg.contains("--include-in-use"),
        "names the override:\n{msg}"
    );
    assert!(
        !msg.contains("BUG-1947"),
        "no internal ids in user prose:\n{msg}"
    );
    assert!(s.worktree.is_dir(), "worktree must survive the refusal");
    assert!(s.lease_exists(), "lease must survive the refusal");
    drop(live);
}

// BUG-1986: `--force` (skip the prompt, discard dirt) alone no longer
// overrides the live-process refusal; a sweep that passes it for those
// reasons must still keep a tree that is in use.
// trace:BUG-1986 | ai:claude
#[test]
fn session_end_force_alone_does_not_override_the_live_process_refusal() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let s = Session::start();
    let live = s.spawn_live_process();
    let out = s.end(&s.repo, &["--force"]);
    let msg = text(&out);
    assert!(!out.status.success(), "--force alone must refuse:\n{msg}");
    assert!(
        msg.contains("--include-in-use"),
        "names the override:\n{msg}"
    );
    assert!(s.worktree.is_dir(), "worktree must survive");
    assert!(s.lease_exists(), "lease must survive");
    drop(live);
}

// (a): `--include-in-use` overrides the refusal and removes the worktree.
// (c): releasing the lease does not move the spec back out of In Progress.
#[test]
fn session_end_include_in_use_removes_a_worktree_with_a_live_process_inside() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let s = Session::start();
    let before = s.status_line();
    assert!(
        before.to_ascii_lowercase().contains("progress"),
        "taking the worktree starts work: {before:?}"
    );
    let live = s.spawn_live_process();

    let out = s.end(&s.repo, &["--include-in-use"]);
    assert!(
        out.status.success(),
        "--include-in-use must proceed:\n{}",
        text(&out)
    );
    assert!(
        !s.worktree.exists(),
        "worktree must be removed under --include-in-use"
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

// Pooling is the default, and returning a pooled tree runs a forced checkout
// and a hard reset, which rewrites the files under a running build just as a
// removal deletes them. The return must refuse the same way, and
// `--include-in-use` must override it the same way.
#[test]
fn session_end_refuses_to_return_a_pooled_worktree_a_live_process_is_using() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let s = Session::start_pooled();
    let live = s.spawn_live_process();
    let pid = live.0.id();
    let branch_before = s.worktree_branch();
    assert!(
        branch_before != "HEAD",
        "a live pooled session is on its own branch"
    );

    let out = s.end(&s.repo, &[]);
    let msg = text(&out);
    assert!(!out.status.success(), "pool return must refuse:\n{msg}");
    assert!(msg.contains(&format!("pid {pid}")), "names the pid:\n{msg}");
    assert!(msg.contains("sleep 600"), "names the command:\n{msg}");
    assert!(
        msg.contains("--include-in-use"),
        "names the override:\n{msg}"
    );
    assert!(s.lease_exists(), "lease must survive the refusal");
    assert_eq!(
        s.worktree_branch(),
        branch_before,
        "the tree must not be detached and reset under the live process"
    );

    let out = s.end(&s.repo, &["--include-in-use"]);
    assert!(
        out.status.success(),
        "--include-in-use must proceed:\n{}",
        text(&out)
    );
    assert!(!s.lease_exists(), "lease released");
    // The tree went back to the pool rather than being deleted: proof this
    // test exercised the pool-return path, not the removal path.
    assert!(
        s.worktree.is_dir(),
        "a pooled tree is returned, not removed:\n{}",
        text(&out)
    );
    drop(live);
}

// BUG-1986: `aida worktree pool return` resets the tree the same way, so it
// applies the strict holder check, and `--include-in-use` overrides it.
// trace:BUG-1986 | ai:claude
#[test]
fn pool_return_refuses_a_worktree_a_live_process_is_using() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let s = Session::start_pooled();
    let live = s.spawn_live_process();
    let pid = live.0.id();
    let branch_before = s.worktree_branch();

    let wt = s.worktree.to_str().unwrap().to_string();
    let mut ret = aida(&s.repo, &s.home);
    ret.args(["worktree", "pool", "return", &wt]);
    let out = ret.output().expect("run pool return");
    let msg = text(&out);
    assert!(!out.status.success(), "pool return must refuse:\n{msg}");
    assert!(msg.contains(&format!("pid {pid}")), "names the pid:\n{msg}");
    assert!(
        msg.contains("--include-in-use"),
        "names the override:\n{msg}"
    );
    assert_eq!(
        s.worktree_branch(),
        branch_before,
        "the tree must not be reset under the live process"
    );

    let mut ret = aida(&s.repo, &s.home);
    ret.args(["worktree", "pool", "return", &wt, "--include-in-use"]);
    let out = ret.output().expect("run pool return --include-in-use");
    assert!(
        out.status.success(),
        "--include-in-use must proceed:\n{}",
        text(&out)
    );
    assert_eq!(s.worktree_branch(), "HEAD", "returned trees are detached");
    drop(live);
}

// BUG-1986: an agent ends its own session from inside its worktree while one
// of its own helpers (an MCP server: same session, cwd in the tree) is still
// running. The helper is exempt as part of the caller's in-worktree run, and
// the final recheck must not lose that exemption by running after the chdir
// to the project root. The end must succeed and remove the tree.
// trace:BUG-1986 | ai:claude
#[test]
#[cfg(unix)]
fn session_end_from_inside_with_its_own_helper_running_ends_cleanly() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let s = Session::start();
    // The `sh` stands in for the agent: its cwd is the worktree, it starts a
    // helper in the tree, then runs `session end` from there.
    // It gets the same hermetic environment as every `aida` run here.
    let env_donor = aida(&s.worktree, &s.home);
    let mut cmd = Command::new("sh");
    cmd.current_dir(&s.worktree).env_clear();
    for (k, v) in env_donor.get_envs() {
        if let Some(v) = v {
            cmd.env(k, v);
        }
    }
    cmd.env("AIDA_BIN", env!("CARGO_BIN_EXE_aida"))
        .env("END_ID", &s.id)
        .args([
            "-c",
            "sleep 600 & helper=$!; \
             \"$AIDA_BIN\" session end \"$END_ID\" --yes --skip-ci; rc=$?; \
             kill $helper; exit $rc",
        ]);
    let out = cmd.output().expect("run agent shell");
    let msg = text(&out);
    assert!(
        out.status.success(),
        "ending your own session with your own helper running must succeed:\n{msg}"
    );
    assert!(!s.worktree.exists(), "worktree removed:\n{msg}");
    assert!(!s.lease_exists(), "lease released:\n{msg}");
}

/// Runs `session end` with the debug-build pause at `stage`, starts a live
/// process in the tree while it waits, then lets it continue. Returns the
/// output and the pid of the live process (still running).
// trace:BUG-1986 | ai:claude
// BUG-1988: the pause hook only exists in debug builds, so under
// `--profile agent` (debug-assertions off) these would wait 60 s for a pause
// that never comes and fail. trace:BUG-1988 | ai:claude
#[cfg(debug_assertions)]
fn end_with_a_holder_started_at(s: &Session, stage: &str) -> (Output, KillOnDrop) {
    let pause = tempfile::tempdir().expect("pause dir");
    let mut cmd = aida(&s.repo, &s.home);
    cmd.env("AIDA_TEST_SESSION_END_PAUSE_AT", stage)
        .env("AIDA_TEST_SESSION_END_PAUSE_DIR", pause.path())
        .args(["session", "end", &s.id, "--yes", "--skip-ci"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = cmd.spawn().expect("spawn session end");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !pause.path().join("paused").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "session end never reached the {stage} pause"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // Nothing held the tree at the first check; this process appears after it.
    let live = s.spawn_live_process();
    std::fs::write(pause.path().join("resume"), "").unwrap();
    let out = child.wait_with_output().expect("wait for session end");
    (out, live)
}

// BUG-1986: a process that starts using the tree while the operator sits at
// the confirmation is caught by the recheck inside `session end`, which
// refuses before anything is touched.
// trace:BUG-1986 | ai:claude
#[test]
#[cfg(debug_assertions)] // trace:BUG-1988 | ai:claude
fn a_holder_started_after_the_confirmation_stops_the_end_cleanly() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let s = Session::start();
    let (out, live) = end_with_a_holder_started_at(&s, "after-confirm");
    let msg = text(&out);
    assert!(!out.status.success(), "the recheck must refuse:\n{msg}");
    assert!(
        msg.contains(&format!("pid {}", live.0.id())),
        "names the pid:\n{msg}"
    );
    assert!(
        msg.contains("NOT ended"),
        "says the session was not ended:\n{msg}"
    );
    assert!(s.worktree.is_dir(), "worktree must survive");
    assert!(s.lease_exists(), "lease must survive");
}

// BUG-1986: a holder that appears just before the teardown (after the dirty
// gate) is caught by the final recheck. The lease and the pool entry are
// left intact, so once the holder exits a plain re-run ends the session and
// returns the tree to the pool.
// trace:BUG-1986 | ai:claude
#[test]
#[cfg(debug_assertions)] // trace:BUG-1988 | ai:claude
fn a_holder_started_just_before_the_teardown_keeps_the_lease_and_tree() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let s = Session::start_pooled();
    let branch_before = s.worktree_branch();
    let (out, live) = end_with_a_holder_started_at(&s, "before-teardown");
    let msg = text(&out);
    assert!(
        !out.status.success(),
        "the final recheck must refuse:\n{msg}"
    );
    assert!(
        msg.contains(&format!("pid {}", live.0.id())),
        "names the pid:\n{msg}"
    );
    assert!(
        msg.contains("NOT ended"),
        "says the session was not ended:\n{msg}"
    );
    assert!(s.worktree.is_dir(), "worktree must survive");
    assert_eq!(s.worktree_branch(), branch_before, "tree must not be reset");
    assert!(s.lease_exists(), "lease must survive the late refusal");
    // The session's runtime links must survive too, or `aida` run from
    // inside the tree would see a bare `.aida/` (a fresh local cache, no
    // lease).
    for link in [".aida-store", ".aida/sessions", ".aida/cache.db"] {
        assert!(
            s.worktree.join(link).is_symlink(),
            "{link} must still be a symlink after the late refusal"
        );
    }

    // Re-run from INSIDE the tree: it must find its lease through the
    // tree's own links and finish the end.
    drop(live);
    let out = s.end(&s.worktree, &[]);
    assert!(
        out.status.success(),
        "once the holder exits, a re-run must end the session:\n{}",
        text(&out)
    );
    assert!(!s.lease_exists(), "lease released on the re-run");
    assert!(s.worktree.is_dir(), "pooled tree returned, not removed");
}

// BUG-1988: a dirty pooled tree whose pre-return salvage fails stops the end
// before the lease is deleted: the session is not ended, the tree keeps its
// work and its runtime links, and the error names the command that finishes
// the end. Once the salvage can be written, that command does.
// trace:BUG-1988 | ai:claude
#[test]
fn a_failed_salvage_keeps_the_lease_and_names_the_recovery_command() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let s = Session::start_pooled();
    let branch_before = s.worktree_branch();
    std::fs::write(s.worktree.join("work.txt"), "uncommitted work").unwrap();
    // Make the salvage dir uncreatable: a regular file sits where it goes.
    let blocker = s.repo.join(".aida").join("salvage");
    let _ = std::fs::remove_dir_all(&blocker);
    std::fs::write(&blocker, "not a dir").unwrap();

    let out = s.end(&s.repo, &[]);
    let msg = text(&out);
    assert!(
        !out.status.success(),
        "a failed salvage must stop the end:\n{msg}"
    );
    assert!(
        msg.contains("NOT ended"),
        "says the session was not ended:\n{msg}"
    );
    assert!(
        msg.contains(&format!("aida session end {}", s.id)),
        "names the recovery command:\n{msg}"
    );
    assert!(s.lease_exists(), "lease must survive a failed salvage");
    assert!(s.worktree.is_dir(), "worktree must survive");
    assert_eq!(s.worktree_branch(), branch_before, "tree must not be reset");
    assert_eq!(
        std::fs::read_to_string(s.worktree.join("work.txt")).unwrap(),
        "uncommitted work",
        "the uncommitted work must survive"
    );
    for link in [".aida-store", ".aida/sessions", ".aida/cache.db"] {
        assert!(
            s.worktree.join(link).is_symlink(),
            "{link} must be restored after the failed salvage"
        );
    }

    // Fix the cause and run the named command: the end goes through and the
    // work is salvaged before the tree returns to the pool.
    std::fs::remove_file(&blocker).unwrap();
    let out = s.end(&s.repo, &[]);
    assert!(
        out.status.success(),
        "the recovery command must end the session:\n{}",
        text(&out)
    );
    assert!(!s.lease_exists(), "lease released on the re-run");
    assert!(s.worktree.is_dir(), "pooled tree returned, not removed");
    let salvaged: Vec<String> = std::fs::read_dir(&blocker)
        .expect("salvage dir created on the re-run")
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap_or_default())
        .collect();
    assert!(
        salvaged.iter().any(|p| p.contains("uncommitted work")),
        "the work was salvaged before the reset: {salvaged:?}"
    );
}
