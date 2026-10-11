#![cfg(target_os = "linux")]
//! Black-box witnesses that ordinary store sync, the direct merge gate and the
//! canonical mailbox digest run as store-writer transactions under the
//! existing store write lock, and that the lock is released before any push.
//!
//! Every test drives the real `aida` binary in a disposable project with a
//! fake HOME, a local bare `origin`, git config isolated from the user, and
//! test-owned git hooks. Two kinds of deterministic interprocess barrier:
//! - a hook barrier: the hook, run by git inside the command under test,
//!   parks until the test releases it, and records what the `aida` process
//!   holds at that moment (lock state, open `*.lock` descriptors);
//! - an external holder: `flock(1)` takes the store lock the way another
//!   lock-respecting writer would, and the test watches the command under
//!   test wait for it (its descriptor on the lock file is open while it
//!   polls).
//!
//! trace:TASK-1717 | ai:claude

mod support;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(60);
/// How long a competitor must stay blocked to count as "waiting", not "slow".
const PARKED: Duration = Duration::from_millis(1500);

struct Fx {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    store: PathBuf,
    origin: PathBuf,
    hooks: PathBuf,
    /// Barrier/observation directory handed to the command under test only.
    obs: PathBuf,
    grant: Option<String>,
}

fn git_env(cmd: &mut Command) -> &mut Command {
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_TERMINAL_PROMPT", "0")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_env(Command::new("git").current_dir(dir).args(args))
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_try(dir: &Path, args: &[&str]) -> Output {
    git_env(Command::new("git").current_dir(dir).args(args))
        .output()
        .expect("run git")
}

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_aida"))
}

impl Fx {
    /// The `aida` command for this fixture. `observed` hands the barrier /
    /// observation directory to the process (and so to the hooks git runs for
    /// it); competitors run without it, so their own hooks stay inert.
    fn cmd(&self, cwd: &Path, args: &[&str], observed: bool) -> Command {
        let mut cmd = Command::new(bin());
        let path = format!(
            "{}:{}",
            bin().parent().unwrap().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        // A clean environment: nothing ambient (AIDA_*, CLAUDE_*, session or
        // grant variables of whoever runs the tests) reaches the binary.
        cmd.env_clear();
        git_env(&mut cmd);
        cmd.current_dir(cwd)
            .env("HOME", &self.home)
            .env("TMPDIR", std::env::temp_dir())
            .env("LANG", "C.UTF-8")
            .env("PATH", path)
            .env("AIDA_TELEMETRY", "0")
            .env("AIDA_USER", "t1717")
            .env("AIDA_OUTPUT_FORMAT", "human")
            .env("AIDA_STORE_LOCK_TIMEOUT_SECS", "60")
            .env("T1717_LOCK", lock_path(&self.store))
            .stdin(Stdio::null())
            .args(args);
        if let Some(grant) = &self.grant {
            cmd.env("AIDA_SESSION_GRANT", grant);
        }
        if observed {
            cmd.env("T1717_DIR", &self.obs);
        }
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(&self.repo, args, false)
            .output()
            .expect("run aida")
    }

    fn run_ok(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert!(out.status.success(), "aida {args:?} failed: {}", text(&out));
        out
    }

    fn spawn(&self, args: &[&str], observed: bool) -> Child {
        self.cmd(&self.repo, args, observed)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn aida")
    }

    fn head(&self) -> String {
        git(&self.store, &["rev-parse", "HEAD"])
    }

    fn subjects(&self, rev: &str) -> Vec<String> {
        git(&self.store, &["log", "--format=%s", rev])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn origin_head(&self) -> String {
        git(&self.origin, &["rev-parse", "refs/heads/aida-store"])
    }

    fn arm(&self, hook: &str) {
        std::fs::write(self.obs.join(format!("arm-{hook}")), "").unwrap();
    }

    fn wait_reached(&self, hook: &str) {
        wait_for(&self.obs.join(format!("reached-{hook}")), hook);
    }

    fn release(&self, hook: &str) {
        std::fs::write(self.obs.join(format!("release-{hook}")), "").unwrap();
    }

    fn obs_lines(&self, hook: &str) -> Vec<String> {
        std::fs::read_to_string(self.obs.join(format!("obs-{hook}.log")))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Install the barrier/observation hook under every name in `names`,
    /// replacing whatever profile is installed.
    fn install_barrier_hooks(&self, names: &[&str]) {
        for name in names {
            install(&self.hooks.join(name), BARRIER_HOOK);
        }
    }

    /// A peer clone of origin's store branch that commits `file` and pushes.
    fn peer_push(&self, file: &str, body: &str) -> String {
        let peer = self.base.join(format!("peer-{}", uuid::Uuid::new_v4()));
        git(
            &self.base,
            &[
                "clone",
                "-q",
                "-b",
                "aida-store",
                self.origin.to_str().unwrap(),
                peer.to_str().unwrap(),
            ],
        );
        std::fs::create_dir_all(peer.join("notes")).unwrap();
        std::fs::write(peer.join("notes").join(file), body).unwrap();
        git(&peer, &["add", "-A"]);
        git(&peer, &["commit", "-qm", &format!("chore: peer {file}")]);
        git(&peer, &["push", "-q", "origin", "aida-store"]);
        git(&peer, &["rev-parse", "HEAD"])
    }

    /// Commit a note directly in the store (an unpublished local commit).
    fn local_commit(&self, file: &str) -> String {
        self.write_store_file(&format!("notes/{file}"), "local\n");
        git(&self.store, &["add", "-A"]);
        git(
            &self.store,
            &["commit", "-qm", &format!("chore: local {file}")],
        );
        self.head()
    }

    fn write_store_file(&self, rel: &str, body: &str) {
        let path = self.store.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
}

const BARRIER_HOOK: &str = r#"#!/bin/sh
# TASK-1717 test hook: observe, then park while armed. Inert unless the git
# process belongs to the command under test (T1717_DIR is set only there).
name=$(basename "$0")
d="$T1717_DIR"
[ -n "$d" ] || { [ "$name" = pre-push ] && cat >/dev/null; exit 0; }
git_pid=$PPID
aida_pid=$(ps -o ppid= -p "$git_pid" | tr -d ' ')
locks() { for f in /proc/"$1"/fd/*; do readlink "$f"; done 2>/dev/null | grep '\.lock$' | sort | tr '\n' ' '; }
{
  if flock -n "$T1717_LOCK" true; then echo "lock=free"; else echo "lock=held"; fi
  echo "hook_locks=$(locks $$)"
  echo "git_locks=$(locks "$git_pid")"
  echo "aida_locks=$(locks "$aida_pid")"
  echo "aida_cmd=$(tr '\0' ' ' < /proc/"$aida_pid"/cmdline)"
} >> "$d/obs-$name.log"
if [ "$name" = pre-push ]; then
  cat >> "$d/prepush-refs.log"
fi
if [ -f "$d/arm-$name" ]; then
  rm -f "$d/arm-$name"
  : > "$d/reached-$name"
  i=0
  while [ ! -f "$d/release-$name" ] && [ $i -lt 1200 ]; do sleep 0.05; i=$((i+1)); done
  rm -f "$d/release-$name" "$d/reached-$name"
fi
exit 0
"#;

fn install(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn lock_path(store: &Path) -> PathBuf {
    store.join(".aida").join("store-write.lock")
}

fn text(out: &Output) -> String {
    format!(
        "status={:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn wait_for(path: &Path, what: &str) {
    let start = Instant::now();
    while !path.exists() {
        assert!(start.elapsed() < WAIT, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn finish(child: Child, what: &str) -> Output {
    let start = Instant::now();
    let mut child = child;
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if start.elapsed() > WAIT {
            let _ = child.kill();
            let out = child.wait_with_output().unwrap();
            panic!("{what} did not finish: {}", text(&out));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// True while `child` is alive and has the store lock file open: it is
/// inside `store_lock::acquire`, waiting for (or holding) the lock.
fn has_lock_fd(child: &Child, store: &Path) -> bool {
    let lock = std::fs::canonicalize(lock_path(store)).unwrap();
    let Ok(dir) = std::fs::read_dir(format!("/proc/{}/fd", child.id())) else {
        return false;
    };
    dir.flatten()
        .filter_map(|e| std::fs::read_link(e.path()).ok())
        .any(|p| p == lock)
}

/// Wait until `child` is blocked on the store lock, then assert it stays
/// blocked for [`PARKED`] (and has not exited).
fn assert_parked_on_lock(child: &mut Child, store: &Path, what: &str) {
    let start = Instant::now();
    while !has_lock_fd(child, store) {
        assert!(
            child.try_wait().unwrap().is_none(),
            "{what} exited instead of waiting for the store lock"
        );
        assert!(
            start.elapsed() < WAIT,
            "{what} never reached the store lock"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(PARKED);
    assert!(
        child.try_wait().unwrap().is_none(),
        "{what} finished while the store lock was held by another writer"
    );
}

/// An external lock-respecting writer: `flock(1)` holds the store lock until
/// released, optionally running `script` (sh) while it holds it.
struct Holder {
    child: Child,
    release: PathBuf,
}

fn hold_lock(fx: &Fx, script: &str) -> Holder {
    let tag = uuid::Uuid::new_v4().to_string();
    let held = fx.base.join(format!("held-{tag}"));
    let release = fx.base.join(format!("release-{tag}"));
    let body = format!(
        "{script}\n: > '{}'\nwhile [ ! -f '{}' ]; do sleep 0.02; done",
        held.display(),
        release.display()
    );
    let mut cmd = Command::new("flock");
    git_env(&mut cmd);
    let child = cmd
        .current_dir(&fx.store)
        .arg(lock_path(&fx.store))
        .args(["-c", &body])
        .spawn()
        .expect("spawn flock holder");
    wait_for(&held, "external lock holder");
    Holder { child, release }
}

impl Holder {
    fn release(mut self) {
        std::fs::write(&self.release, "").unwrap();
        assert!(self.child.wait().unwrap().success(), "holder failed");
    }

    /// SIGKILL the holder: `flock` and the shell it runs (which shares the
    /// locked descriptor), as a crashed writer would die.
    fn kill(&mut self) {
        let pid = self.child.id();
        let children =
            std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).unwrap_or_default();
        for c in children.split_whitespace() {
            let _ = Command::new("kill").args(["-9", c]).status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Holder {
    // A failing test must not leave a holder behind (it would keep the lock
    // and the test's output pipe open).
    fn drop(&mut self) {
        let _ = std::fs::write(&self.release, "");
        if matches!(self.child.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_millis(100));
            if matches!(self.child.try_wait(), Ok(None)) {
                self.kill();
            }
        }
    }
}

/// Disposable distributed project with an attached store and a local bare
/// origin carrying both branches. Hooks: the barrier hook only.
fn fixture() -> Fx {
    fixture_with(false)
}

/// `scaffolded`: run a default `aida init` (the real scaffolder writes its
/// generated git hooks into `.git/hooks`) and leave `core.hooksPath` unset;
/// otherwise `--no-hooks` plus the test's barrier hooks.
fn fixture_with(scaffolded: bool) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("home");
    let repo = base.join("repo");
    let origin = base.join("origin.git");
    let hooks = base.join("hooks");
    let obs = base.join("obs");
    for d in [&home, &repo, &hooks, &obs] {
        std::fs::create_dir_all(d).unwrap();
    }
    git(&base, &["init", "-q", "--bare", "-b", "main", "origin.git"]);
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let mut fx = Fx {
        _tmp: tmp,
        store: repo.join(".aida-store"),
        base,
        home,
        repo,
        origin,
        hooks,
        obs,
        grant: None,
    };
    let mut init = vec![
        "init",
        "--force",
        "--no-skills",
        "--no-agent-config",
        "--no-roles",
    ];
    if !scaffolded {
        init.push("--no-hooks");
    }
    fx.run_ok(&init);
    fx.run_ok(&["add", "seed spec", "--type", "task"]);
    if !scaffolded {
        git(
            &fx.repo,
            &["config", "core.hooksPath", fx.hooks.to_str().unwrap()],
        );
        fx.install_barrier_hooks(&["pre-commit", "post-commit", "post-rewrite", "pre-push"]);
    }
    git(
        &fx.repo,
        &["remote", "add", "origin", fx.origin.to_str().unwrap()],
    );
    git(&fx.repo, &["push", "-q", "origin", "main"]);
    // Commit whatever `add` left pending so every test starts clean.
    let _ = git_try(&fx.store, &["add", "-A"]);
    let _ = git_try(&fx.store, &["commit", "-qm", "chore: fixture settle"]);
    git(&fx.store, &["push", "-q", "origin", "aida-store"]);
    fx.grant = support::ensure_seat_for(&fx.home, &fx.repo, "t1717", "advisor", &[]);
    let _ = git_try(&fx.store, &["add", "-A"]);
    let _ = git_try(&fx.store, &["commit", "-qm", "chore: fixture roster"]);
    git(&fx.store, &["push", "-q", "origin", "aida-store"]);
    assert_eq!(git(&fx.store, &["status", "--porcelain"]), "");
    fx
}

/// The `*.lock` paths in an observation value that belong to this fixture
/// (its HOME, project and store). Descriptors the test process itself
/// inherited from outside (a build-slot lock) are not the command's.
fn fixture_locks(fx: &Fx, value: &str) -> Vec<String> {
    let base = fx.base.display().to_string();
    value
        .split_whitespace()
        .filter(|p| p.starts_with(&base))
        .map(str::to_string)
        .collect()
}

fn assert_hook_saw_lock_held_without_inheritance(fx: &Fx, hook: &str) {
    let lines = fx.obs_lines(hook);
    assert!(
        !lines.is_empty(),
        "{hook} never ran for the command under test"
    );
    for line in &lines {
        if let Some(v) = line.strip_prefix("lock=") {
            assert_eq!(v, "held", "{hook}: the store lock must be held: {lines:?}");
        }
        for prefix in ["hook_locks=", "git_locks="] {
            if let Some(v) = line.strip_prefix(prefix) {
                assert!(
                    fixture_locks(fx, v).is_empty(),
                    "{hook}: the lock descriptor must not be inherited: {lines:?}"
                );
            }
        }
        if let Some(v) = line.strip_prefix("aida_locks=") {
            assert_eq!(
                fixture_locks(fx, v),
                vec![lock_path(&fx.store).display().to_string()],
                "{hook}: the parent holds exactly the store lock, no seat/cache/tick lock: {lines:?}"
            );
        }
    }
}

fn assert_push_ran_unlocked(fx: &Fx) {
    let lines = fx.obs_lines("pre-push");
    assert!(
        !lines.is_empty(),
        "pre-push never ran for the command under test"
    );
    for line in &lines {
        if let Some(v) = line.strip_prefix("lock=") {
            assert_eq!(v, "free", "push must run outside the store lock: {lines:?}");
        }
        for prefix in ["hook_locks=", "git_locks=", "aida_locks="] {
            if let Some(v) = line.strip_prefix(prefix) {
                assert!(
                    fixture_locks(fx, v).is_empty(),
                    "no lock (store, seat, cache, tick) may be held across a push: {lines:?}"
                );
            }
        }
    }
}

fn pushed_shas(fx: &Fx) -> Vec<String> {
    std::fs::read_to_string(fx.obs.join("prepush-refs.log"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1).map(str::to_string))
        .collect()
}

// ── Group 1: sync versus spec edit ─────────────────────────────────────────

/// `db sync` parks inside its pending-changes commit; an actual `aida edit`
/// waits for the lock without touching HEAD or the index, then lands after
/// the sync and both survive.
#[test]
fn sync_vs_edit_serializes_at_the_pending_commit() {
    let fx = fixture();
    fx.write_store_file("notes/pending.txt", "pending\n");
    let before = fx.head();
    fx.arm("pre-commit");
    let sync = fx.spawn(&["db", "sync"], true);
    fx.wait_reached("pre-commit");

    let parked = snapshot(&fx);
    let mut edit = fx.spawn(&["edit", "TASK-1", "--title", "edited during sync"], false);
    assert_parked_on_lock(&mut edit, &fx.store, "aida edit");
    assert_eq!(
        fx.head(),
        before,
        "nothing may commit while sync holds the lock"
    );
    assert_eq!(
        snapshot(&fx),
        parked,
        "the waiting writer touched no bytes, index or admin state"
    );

    fx.release("pre-commit");
    let sync = finish(sync, "db sync");
    assert!(sync.status.success(), "{}", text(&sync));
    let edit = finish(edit, "aida edit");
    assert!(edit.status.success(), "{}", text(&edit));

    let subjects = fx.subjects("HEAD");
    assert_eq!(subjects[0], "update TASK-1", "{subjects:?}");
    assert_eq!(subjects[1], "chore: sync pending changes", "{subjects:?}");
    assert_eq!(git(&fx.store, &["status", "--porcelain"]), "");
    assert_hook_saw_lock_held_without_inheritance(&fx, "pre-commit");
}

/// `db sync --pull` parks between its commit and the rebase (post-commit) and
/// again inside the rebase (post-rewrite); the competitor edit waits through
/// both and lands on top of the rebased result. The peer commit survives.
#[test]
fn sync_vs_edit_serializes_across_commit_and_rebase() {
    let fx = fixture();
    let peer = fx.peer_push("peer.txt", "peer\n");
    fx.write_store_file("notes/pending.txt", "pending\n");
    fx.arm("post-commit");
    fx.arm("post-rewrite");
    let sync = fx.spawn(&["db", "sync", "--pull"], true);

    fx.wait_reached("post-commit");
    let mut edit = fx.spawn(
        &["edit", "TASK-1", "--title", "edited during rebase"],
        false,
    );
    assert_parked_on_lock(&mut edit, &fx.store, "aida edit");
    fx.release("post-commit");
    fx.wait_reached("post-rewrite");
    std::thread::sleep(PARKED);
    assert!(
        edit.try_wait().unwrap().is_none(),
        "edit ran inside the rebase"
    );
    fx.release("post-rewrite");

    let sync = finish(sync, "db sync --pull");
    assert!(sync.status.success(), "{}", text(&sync));
    let edit = finish(edit, "aida edit");
    assert!(edit.status.success(), "{}", text(&edit));
    let subjects = fx.subjects("HEAD");
    assert_eq!(subjects[0], "update TASK-1", "{subjects:?}");
    assert_eq!(subjects[1], "chore: sync pending changes", "{subjects:?}");
    git(&fx.store, &["merge-base", "--is-ancestor", &peer, "HEAD"]);
    assert_hook_saw_lock_held_without_inheritance(&fx, "post-commit");
    assert_hook_saw_lock_held_without_inheritance(&fx, "post-rewrite");
}

/// `db sync` reads no branch/index/dirty state before it owns the lock: with
/// another writer holding it, the pending file stays unstaged and HEAD stays.
#[test]
fn sync_touches_nothing_before_it_owns_the_lock() {
    let fx = fixture();
    fx.write_store_file("notes/pending.txt", "pending\n");
    let before = fx.head();
    let holder = hold_lock(&fx, "");
    let mut sync = fx.spawn(&["db", "sync"], true);
    assert_parked_on_lock(&mut sync, &fx.store, "db sync");
    assert_eq!(fx.head(), before);
    assert_eq!(
        git(&fx.store, &["diff", "--cached", "--name-only"]),
        "",
        "nothing staged before the lock"
    );
    holder.release();
    let sync = finish(sync, "db sync");
    assert!(sync.status.success(), "{}", text(&sync));
    assert_eq!(fx.subjects("HEAD")[0], "chore: sync pending changes");
}

/// A rebase already in progress (someone's manual recovery) is refused after
/// the lock is taken, and left exactly as it was: not aborted, not committed
/// into.
#[test]
fn sync_refuses_and_preserves_a_pre_existing_rebase() {
    let fx = fixture();
    let git_dir = PathBuf::from(git(&fx.store, &["rev-parse", "--absolute-git-dir"]));
    let marker = git_dir.join("rebase-merge");
    std::fs::create_dir_all(&marker).unwrap();
    std::fs::write(marker.join("owner"), "someone else\n").unwrap();
    fx.write_store_file("notes/pending.txt", "pending\n");
    let before = fx.head();
    let out = fx
        .cmd(&fx.repo, &["db", "sync", "--pull"], true)
        .output()
        .unwrap();
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("rebase in progress"), "{}", text(&out));
    assert_eq!(fx.head(), before);
    assert_eq!(
        std::fs::read_to_string(marker.join("owner")).unwrap(),
        "someone else\n"
    );
    assert_eq!(git(&fx.store, &["diff", "--cached", "--name-only"]), "");
}

// ── Group 2: sync versus sync, queue, mailbox and the direct merge gate ────

/// While one sync is parked inside its window, a second sync (started through
/// a symlinked path to the project), an actual queue write, a mailbox digest
/// and the direct merge gate all wait on the SAME lock file (same inode); a
/// different project's store makes progress meanwhile.
#[test]
fn sync_vs_sync_queue_mailbox_and_merge_gate_share_one_lock() {
    let fx = fixture();
    let other = fixture();
    let link = fx.base.join("repo-link");
    std::os::unix::fs::symlink(&fx.repo, &link).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!(
        std::fs::metadata(lock_path(&fx.store)).unwrap().ino(),
        std::fs::metadata(lock_path(&link.join(".aida-store")))
            .unwrap()
            .ino(),
        "canonical and symlinked discovery name one lock inode"
    );

    fx.write_store_file("notes/pending.txt", "pending\n");
    fx.arm("pre-commit");
    let first = fx.spawn(&["db", "sync"], true);
    fx.wait_reached("pre-commit");

    fx.write_store_file("notes/second.txt", "second\n");
    let mut second = fx
        .cmd(&link, &["db", "sync", "-m", "chore: second sync"], false)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert_parked_on_lock(&mut second, &fx.store, "second db sync (symlink path)");
    let mut queue = fx.spawn(&["queue", "add", "TASK-1"], false);
    assert_parked_on_lock(&mut queue, &fx.store, "aida queue add");
    let mut gate = fx.spawn(&["db", "merge-gate"], false);
    assert_parked_on_lock(&mut gate, &fx.store, "aida db merge-gate");
    let local = fx.repo.join(".aida/mailbox");
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(local.join("m-queued.json"), message_json("m-queued", false)).unwrap();
    let mut digest = fx.spawn(&["mailbox", "sync"], false);
    assert_parked_on_lock(&mut digest, &fx.store, "aida mailbox sync");
    assert!(
        !fx.store.join("mailbox/m-queued.json").exists(),
        "no canonical mailbox write before the lock"
    );

    // A distinct store is not serialized behind this one.
    other.write_store_file("notes/other.txt", "other\n");
    let started = Instant::now();
    let o = other.run(&["db", "sync"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(started.elapsed() < Duration::from_secs(20));

    fx.release("pre-commit");
    for (child, what) in [
        (first, "first sync"),
        (second, "second sync"),
        (queue, "queue add"),
        (gate, "merge-gate"),
        (digest, "mailbox sync"),
    ] {
        let out = finish(child, what);
        assert!(out.status.success(), "{what}: {}", text(&out));
    }
    let subjects = fx.subjects("HEAD");
    assert!(
        subjects.contains(&"chore: sync pending changes".to_string()),
        "{subjects:?}"
    );
    assert!(
        subjects.contains(&"chore: second sync".to_string()),
        "{subjects:?}"
    );
    assert!(
        subjects.iter().any(|s| s.starts_with("mailbox: digest 1")),
        "{subjects:?}"
    );
    let queue_file = git(&fx.store, &["ls-files", "registry/queues"]);
    assert!(
        !queue_file.is_empty(),
        "the queue write was committed, not lost"
    );
    assert_eq!(git(&fx.store, &["status", "--porcelain"]), "");
}

// ── Group 3: rejection, retry and peer preservation ────────────────────────

/// `db sync --push` pushes the commit it captured under the lock, by SHA,
/// with the lock released (a local edit lands while the push is parked). The
/// peer push makes the first attempt fail; the retry rebases the CURRENT store
/// (peer commit and the concurrent local edit both kept) and publishes it.
/// The mirror only ever receives the commit origin accepted.
#[test]
fn rejection_retry_preserves_peer_and_concurrent_local_commit() {
    let fx = fixture();
    let mirror = fx.base.join("mirror.git");
    git(
        &fx.base,
        &["init", "-q", "--bare", "-b", "main", "mirror.git"],
    );
    git(
        &fx.repo,
        &["remote", "add", "mirror", mirror.to_str().unwrap()],
    );
    let cfg = fx.repo.join(".aida/config.toml");
    let body = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        body.contains("\n[store.sync]\n"),
        "scaffolded config has [store.sync]"
    );
    let body = body.replacen(
        "\n[store.sync]\n",
        "\n[store.sync]\nmirror_remotes = [\"mirror\"]\n",
        1,
    );
    std::fs::write(&cfg, body).unwrap();

    fx.write_store_file("notes/pending.txt", "pending\n");
    // The retry's fetch uses a private ref and never writes FETCH_HEAD.
    let fetch_head = PathBuf::from(git(
        &fx.store,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "FETCH_HEAD",
        ],
    ));
    std::fs::write(&fetch_head, "sentinel FETCH_HEAD\n").unwrap();
    fx.arm("pre-push");
    let sync = fx.spawn(&["db", "sync", "--push"], true);
    fx.wait_reached("pre-push");
    let captured = pushed_shas(&fx);
    // The push is outside the lock: an actual edit commits right now.
    let edit = fx.run(&["edit", "TASK-1", "--title", "edited during push"]);
    assert!(edit.status.success(), "{}", text(&edit));
    let edited = fx.head();
    let peer = fx.peer_push("peer.txt", "peer\n");
    fx.release("pre-push");
    let sync = finish(sync, "db sync --push");
    assert!(sync.status.success(), "{}", text(&sync));
    assert!(
        text(&sync).contains("Push complete after rebase"),
        "{}",
        text(&sync)
    );

    let shas = pushed_shas(&fx);
    assert_eq!(shas.len(), 2 + 1, "origin twice, mirror once: {shas:?}");
    assert_ne!(shas[0], edited, "the first push named the captured commit");
    if !captured.is_empty() {
        assert_eq!(captured[0], shas[0]);
    }
    let origin = fx.origin_head();
    git(&fx.store, &["merge-base", "--is-ancestor", &peer, &origin]);
    let published = fx.subjects(&origin);
    assert!(
        published.contains(&"update TASK-1".to_string()),
        "{published:?}"
    );
    assert!(
        published.contains(&"chore: sync pending changes".to_string()),
        "{published:?}"
    );
    assert_eq!(
        git(&mirror, &["rev-parse", "refs/heads/aida-store"]),
        origin,
        "mirror gets exactly what origin accepted"
    );
    assert_eq!(
        std::fs::read_to_string(&fetch_head).unwrap(),
        "sentinel FETCH_HEAD\n"
    );
    assert_eq!(
        git(&fx.store, &["for-each-ref", "refs/aida/"]),
        "",
        "private fetch refs removed"
    );
    assert_push_ran_unlocked(&fx);
}

/// A second rejection is a failure: the sync exits non-zero, keeps its local
/// commit, never claims success and never mirrors.
#[test]
fn repeated_rejection_fails_and_keeps_local_commits() {
    let fx = fixture();
    fx.write_store_file("notes/pending.txt", "pending\n");
    fx.arm("pre-push");
    let sync = fx.spawn(&["db", "sync", "--push"], true);
    fx.wait_reached("pre-push");
    fx.peer_push("peer1.txt", "peer\n");
    fx.arm("pre-push");
    fx.release("pre-push");
    fx.wait_reached("pre-push");
    let peer2 = fx.peer_push("peer2.txt", "peer\n");
    fx.release("pre-push");
    let out = finish(sync, "db sync --push");
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("rejected again"), "{}", text(&out));
    assert!(!text(&out).contains("Push complete"), "{}", text(&out));
    assert_eq!(fx.origin_head(), peer2, "origin keeps the peer's tip");
    assert!(fx
        .subjects("HEAD")
        .contains(&"chore: sync pending changes".to_string()));
    assert_push_ran_unlocked(&fx);
}

// ── Group 4: pull, --sync, allocation and remote reconcile ─────────────────

/// `aida pull --store-only` publishes the mailbox, commits, rebases, scans and
/// gates in one window: an edit waits through the rebase, and the pre-existing
/// pending change, the peer commit and the edit all survive.
#[test]
fn pull_store_leg_is_one_window() {
    let fx = fixture();
    let peer = fx.peer_push("peer.txt", "peer\n");
    fx.write_store_file("notes/pending.txt", "pending\n");
    fx.arm("post-rewrite");
    let pull = fx.spawn(&["pull", "--store-only"], true);
    fx.wait_reached("post-rewrite");
    let mut edit = fx.spawn(&["edit", "TASK-1", "--title", "edited during pull"], false);
    assert_parked_on_lock(&mut edit, &fx.store, "aida edit");
    fx.release("post-rewrite");
    let pull = finish(pull, "aida pull");
    assert!(pull.status.success(), "{}", text(&pull));
    let edit = finish(edit, "aida edit");
    assert!(edit.status.success(), "{}", text(&edit));
    git(&fx.store, &["merge-base", "--is-ancestor", &peer, "HEAD"]);
    let subjects = fx.subjects("HEAD");
    assert_eq!(subjects[0], "update TASK-1");
    assert!(subjects.contains(&"chore: sync pending changes".to_string()));
    assert_hook_saw_lock_held_without_inheritance(&fx, "post-rewrite");
}

/// A read command's `--sync` pull holds the lock across the rebase, and a
/// failed rebase it did not start is never aborted by it.
#[test]
fn read_sync_pull_holds_the_lock_and_leaves_foreign_rebase_alone() {
    let fx = fixture();
    let peer = fx.peer_push("peer.txt", "peer\n");
    // An unpublished local commit, so the rebase rewrites (post-rewrite runs).
    fx.local_commit("local.txt");
    fx.arm("post-rewrite");
    let list = fx.spawn(&["list", "--sync"], true);
    fx.wait_reached("post-rewrite");
    let mut edit = fx.spawn(
        &["edit", "TASK-1", "--title", "edited during --sync"],
        false,
    );
    assert_parked_on_lock(&mut edit, &fx.store, "aida edit");
    fx.release("post-rewrite");
    let list = finish(list, "list --sync");
    assert!(list.status.success(), "{}", text(&list));
    assert!(finish(edit, "edit").status.success());
    git(&fx.store, &["merge-base", "--is-ancestor", &peer, "HEAD"]);

    let git_dir = PathBuf::from(git(&fx.store, &["rev-parse", "--absolute-git-dir"]));
    std::fs::create_dir_all(git_dir.join("rebase-merge")).unwrap();
    std::fs::write(git_dir.join("rebase-merge/owner"), "someone else\n").unwrap();
    let out = fx.run(&["list", "--sync"]);
    assert!(text(&out).contains("--sync pull failed"), "{}", text(&out));
    assert!(
        git_dir.join("rebase-merge/owner").exists(),
        "a rebase this command did not start is not aborted"
    );
}

/// `aida add` against a moved origin: the pre-allocation pull rebases under
/// the lock (an edit waits), and the post-allocation push publishes the
/// captured commit outside the lock.
#[test]
fn allocation_pull_and_push_follow_the_window_discipline() {
    let fx = fixture();
    let peer = fx.peer_push("peer.txt", "peer\n");
    fx.local_commit("local.txt");
    fx.arm("post-rewrite");
    let add = fx.spawn(&["add", "allocated during test", "--type", "task"], true);
    fx.wait_reached("post-rewrite");
    let mut edit = fx.spawn(
        &["edit", "TASK-1", "--title", "edited during allocation"],
        false,
    );
    assert_parked_on_lock(&mut edit, &fx.store, "aida edit");
    fx.release("post-rewrite");
    let add = finish(add, "aida add");
    assert!(add.status.success(), "{}", text(&add));
    assert!(finish(edit, "edit").status.success());
    let origin = fx.origin_head();
    git(&fx.store, &["merge-base", "--is-ancestor", &peer, &origin]);
    assert!(
        fx.subjects(&origin)
            .iter()
            .any(|s| s.contains("allocated during test")),
        "the new spec reached origin"
    );
    assert_hook_saw_lock_held_without_inheritance(&fx, "post-rewrite");
    assert_push_ran_unlocked(&fx);
}

/// Remote reconcile discovers hubs without the lock, then recomputes the
/// frontier and its rollback preimage against the CURRENT head under the
/// lock: a local commit that lands between discovery and the lock survives
/// both a refused (unknown-conflict) reconcile and a successful one.
#[test]
fn remote_reconcile_uses_the_current_locked_preimage() {
    let fx = fixture();
    // Diverge origin and local on a non-mergeable path.
    fx.peer_push("clash.txt", "origin side\n");
    fx.write_store_file("notes/clash.txt", "local side\n");
    git(&fx.store, &["add", "-A"]);
    git(&fx.store, &["commit", "-qm", "chore: local clash"]);

    // Hold the lock; reconcile discovers, then waits for it.
    let holder = hold_lock(&fx, "");
    let mut rec = fx.spawn(&["remote", "reconcile", "--execute", "--yes"], true);
    assert_parked_on_lock(&mut rec, &fx.store, "remote reconcile --execute");
    // Another writer's local commit lands after discovery, under the lock.
    fx.write_store_file("notes/late.txt", "late\n");
    git(&fx.store, &["add", "-A"]);
    git(&fx.store, &["commit", "-qm", "chore: late local commit"]);
    let late = fx.head();
    holder.release();
    let out = finish(rec, "remote reconcile");
    assert!(
        !out.status.success(),
        "unknown conflict must refuse: {}",
        text(&out)
    );
    assert!(text(&out).contains("union merge"), "{}", text(&out));
    assert_eq!(
        fx.head(),
        late,
        "rollback goes to the locked preimage, not discovery's"
    );
    assert_eq!(git(&fx.store, &["status", "--porcelain"]), "");

    // Healthy positive: a mergeable divergence reconciles and keeps both.
    let fx = fixture();
    let peer = fx.peer_push("peer.txt", "peer\n");
    fx.write_store_file("notes/local.txt", "local\n");
    git(&fx.store, &["add", "-A"]);
    git(&fx.store, &["commit", "-qm", "chore: local only"]);
    let holder = hold_lock(&fx, "");
    let mut rec = fx.spawn(&["remote", "reconcile", "--execute", "--yes"], true);
    assert_parked_on_lock(&mut rec, &fx.store, "remote reconcile --execute");
    fx.write_store_file("notes/late.txt", "late\n");
    git(&fx.store, &["add", "-A"]);
    git(&fx.store, &["commit", "-qm", "chore: late local commit"]);
    let late = fx.head();
    holder.release();
    let out = finish(rec, "remote reconcile");
    assert!(out.status.success(), "{}", text(&out));
    let origin = fx.origin_head();
    for keep in [&peer, &late] {
        git(&fx.store, &["merge-base", "--is-ancestor", keep, &origin]);
    }
    assert_eq!(fx.head(), origin);
    assert_push_ran_unlocked(&fx);
}

// ── Mailbox ────────────────────────────────────────────────────────────────

fn message_json(id: &str, deleted: bool) -> String {
    serde_json::json!({
        "id": id,
        "thread_id": id,
        "from": "alice",
        "to": { "kind": "agent", "agent": "bob" },
        "timestamp": 1_700_000_000_000i64,
        "body": format!("body of {id}"),
        "urgent": false,
        "intent": "fyi",
        "retracted": false,
        "deleted": deleted,
        "archived": false,
    })
    .to_string()
}

/// The digest re-reads canonical state under the lock: a newer canonical
/// tombstone committed by a peer is not overwritten by the stale local copy,
/// a new local message is written, and stage+commit share the window (an
/// edit waits while the digest commit is parked). Re-running digests nothing.
#[test]
fn mailbox_digest_rereads_canonical_and_commits_in_one_window() {
    let fx = fixture();
    let local = fx.repo.join(".aida/mailbox");
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(local.join("m-old.json"), message_json("m-old", false)).unwrap();
    std::fs::write(local.join("m-new.json"), message_json("m-new", false)).unwrap();
    // A peer's tombstone for m-old is already canonical.
    fx.write_store_file("mailbox/m-old.json", &message_json("m-old", true));
    git(&fx.store, &["add", "-A"]);
    git(&fx.store, &["commit", "-qm", "chore: peer tombstone"]);

    fx.arm("pre-commit");
    let digest = fx.spawn(&["mailbox", "sync"], true);
    fx.wait_reached("pre-commit");
    let mut edit = fx.spawn(
        &["edit", "TASK-1", "--title", "edited during digest"],
        false,
    );
    assert_parked_on_lock(&mut edit, &fx.store, "aida edit");
    fx.release("pre-commit");
    let out = finish(digest, "mailbox sync");
    assert!(out.status.success(), "{}", text(&out));
    assert!(finish(edit, "edit").status.success());

    let old: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fx.store.join("mailbox/m-old.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(old["deleted"], true, "the newer tombstone wins");
    assert!(fx.store.join("mailbox/m-new.json").exists());
    let subjects = fx.subjects("HEAD");
    assert_eq!(subjects[0], "update TASK-1");
    assert_eq!(subjects[1], "mailbox: digest 1 message(s)");
    assert_eq!(git(&fx.store, &["status", "--porcelain"]), "");
    assert_hook_saw_lock_held_without_inheritance(&fx, "pre-commit");

    let again = fx.run(&["mailbox", "sync"]);
    assert!(again.status.success());
    assert_eq!(fx.subjects("HEAD")[0], "update TASK-1", "idempotent");
}

/// Local send needs no store ownership: it completes while another writer
/// holds the store lock, and writes nothing canonical.
#[test]
fn local_mailbox_send_never_waits_for_the_store() {
    let fx = fixture();
    let holder = hold_lock(&fx, "");
    let started = Instant::now();
    let out = fx.run(&["mailbox", "send", "--to", "bob", "hello"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(!fx.store.join("mailbox").exists());
    holder.release();
}

// ── Group 5: real hook profiles ────────────────────────────────────────────

fn template(name: &str) -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../aida-core/templates/hooks");
    std::fs::read_to_string(dir.join(name)).unwrap()
}

/// The shipped template profile around a full `db sync --pull --push` and a
/// spec edit: store pre-commit's early exit + sentinel, post-commit's
/// sentinel match, the store-pair trailer transform, linked-store
/// post-rewrite exit, and a pre-push that observes the lock free. All finish;
/// the trailer pins the pre-commit store head; the selected binary is the
/// one under test.
#[test]
fn template_hook_profile_completes_with_healthy_effects() {
    let fx = fixture();
    for (hook, tpl) in [
        ("pre-commit", "aida-pre-commit.sh"),
        ("post-commit", "aida-post-commit.sh"),
        ("prepare-commit-msg", "aida-store-pair.sh"),
        ("commit-msg", "aida-commit-msg"),
        ("post-rewrite", "aida-sync-agent-skills.sh"),
    ] {
        install(&fx.hooks.join(hook), &template(tpl));
    }
    fx.install_barrier_hooks(&["pre-push"]);
    fx.peer_push("peer.txt", "peer\n");
    fx.write_store_file("notes/pending.txt", "pending\n");
    let before = fx.head();
    let out = fx
        .cmd(&fx.repo, &["db", "sync", "--pull", "--push"], true)
        .env("AIDA_FIELD_STUDY", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let msg = git(&fx.store, &["log", "--format=%B", "-1", "HEAD"]);
    assert!(
        msg.contains(&format!("Aida-Store: {before}")),
        "store-pair trailer pins the pre-commit head: {msg}"
    );
    assert_eq!(fx.origin_head(), fx.head());
    assert!(
        !fx.home.join(".aida/rule-violations.jsonl").exists(),
        "matching sentinel: no bypass recorded"
    );
    assert_push_ran_unlocked(&fx);
    let edit = fx.run(&["edit", "TASK-1", "--title", "after profile"]);
    assert!(edit.status.success(), "{}", text(&edit));
}

/// Missing-sentinel path: post-commit without the pre-commit hook invokes
/// `aida internal record-no-verify-bypass` while the parent holds the store
/// lock. It finishes (no hook/cache/lock cycle) and its observation lands.
#[test]
fn post_commit_bypass_detector_runs_under_the_held_lock() {
    let fx = fixture();
    install(
        &fx.hooks.join("post-commit"),
        &template("aida-post-commit.sh"),
    );
    let _ = std::fs::remove_file(fx.hooks.join("pre-commit"));
    // Stale sentinel from an earlier commit: must be treated as a bypass too.
    let common = PathBuf::from(git(
        &fx.store,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ));
    std::fs::write(
        common.join("aida-precommit-sentinel"),
        "0000000000000000000000000000000000000000\n",
    )
    .unwrap();
    fx.write_store_file("notes/pending.txt", "pending\n");
    let started = Instant::now();
    // The field study rides telemetry; both write only under the fake HOME.
    let out = fx
        .cmd(&fx.repo, &["db", "sync"], true)
        .env("AIDA_TELEMETRY", "1")
        .env("AIDA_FIELD_STUDY", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert!(started.elapsed() < Duration::from_secs(30));
    assert_eq!(fx.subjects("HEAD")[0], "chore: sync pending changes");
    assert!(
        !common.join("aida-precommit-sentinel").exists(),
        "sentinel consumed"
    );
    let log = std::fs::read_to_string(fx.home.join(".aida/rule-violations.jsonl"))
        .expect("the detector's observation is recorded under the fake HOME");
    assert!(log.contains("no-verify-bypass"), "{log}");
}

/// The hook profile installed in the AIDA development checkout on
/// 2026-10-08, byte for byte: pre-commit, commit-msg, post-rewrite,
/// post-checkout and post-merge are symlinks there to the shipped templates
/// (installed here as the template bytes); post-commit, pre-push and
/// prepare-commit-msg are the installed files under `tests/fixtures/
/// task_1717_installed_hooks/` (sha256 7bfca18a…546a, d536291d…0ace,
/// 93e7c9eb…c3a), with their installed modes. The only edit: the
/// prepare-commit-msg's hard-coded store path is replaced by this fixture's.
/// The installed post-commit is mode 0644, so git ignores it, as it does
/// there.
#[test]
fn installed_hook_profile_completes() {
    use std::os::unix::fs::PermissionsExt;
    let fx = fixture();
    for (hook, tpl) in [
        ("pre-commit", "aida-pre-commit.sh"),
        ("commit-msg", "aida-commit-msg"),
        ("post-rewrite", "aida-sync-agent-skills.sh"),
        ("post-checkout", "aida-sync-agent-skills.sh"),
        ("post-merge", "aida-sync-agent-skills.sh"),
    ] {
        install(&fx.hooks.join(hook), &template(tpl));
    }
    let installed =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/task_1717_installed_hooks");
    let prepare = std::fs::read_to_string(installed.join("prepare-commit-msg"))
        .unwrap()
        .replace("/home/joe/ai/aida/.aida-store", fx.store.to_str().unwrap());
    install(&fx.hooks.join("prepare-commit-msg"), &prepare);
    install(
        &fx.hooks.join("pre-push"),
        &std::fs::read_to_string(installed.join("pre-push")).unwrap(),
    );
    let post_commit = fx.hooks.join("post-commit");
    std::fs::write(
        &post_commit,
        std::fs::read(installed.join("post-commit")).unwrap(),
    )
    .unwrap();
    std::fs::set_permissions(&post_commit, std::fs::Permissions::from_mode(0o644)).unwrap();

    fx.peer_push("peer.txt", "peer\n");
    fx.write_store_file("notes/pending.txt", "pending\n");
    let out = fx.run(&["db", "sync", "--pull", "--push"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(fx.origin_head(), fx.head());
    assert_eq!(git(&fx.store, &["status", "--porcelain"]), "");
    let pull = fx.run(&["pull", "--store-only"]);
    assert!(pull.status.success(), "{}", text(&pull));
    let edit = fx.run(&["edit", "TASK-1", "--title", "after installed profile"]);
    assert!(edit.status.success(), "{}", text(&edit));
}

// ── F1: hook profiles that call back into an aida writer ───────────────────

/// A hook that runs an aida store writer is refused BEFORE the transaction
/// changes anything: the pending file stays unstaged, HEAD and the spec are
/// unchanged, and the refusal is immediate (not a lock timeout).
#[test]
fn reentrant_writer_hook_is_refused_before_any_change() {
    let fx = fixture();
    install(
        &fx.hooks.join("pre-commit"),
        "#!/bin/sh\naida edit TASK-1 --title reentered || exit 1\n",
    );
    fx.write_store_file("notes/pending.txt", "pending\n");
    let before = fx.head();
    let spec_before = git(&fx.store, &["ls-files", "-s", "objects"]);
    for args in [
        &["db", "sync"][..],
        &["push", "--store-only"],
        &["pull", "--store-only"],
        &["mailbox", "sync"],
    ] {
        let started = Instant::now();
        let out = fx.run(args);
        let t = text(&out);
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "{args:?} waited: {t}"
        );
        assert!(!out.status.success(), "{args:?}: {t}");
        assert!(t.contains("is not a shipped AIDA hook"), "{args:?}: {t}");
        assert_eq!(fx.head(), before, "{args:?}");
        assert_eq!(
            git(&fx.store, &["diff", "--cached", "--name-only"]),
            "",
            "{args:?}"
        );
        assert_eq!(git(&fx.store, &["ls-files", "-s", "objects"]), spec_before);
        assert!(fx.store.join("notes/pending.txt").exists());
    }
    let gate = fx.run(&["db", "merge-gate"]);
    assert!(!gate.status.success(), "{}", text(&gate));
    assert!(
        text(&gate).contains("is not a shipped AIDA hook"),
        "{}",
        text(&gate)
    );
    let shown = fx.run(&["show", "TASK-1"]);
    assert!(!text(&shown).contains("reentered"), "{}", text(&shown));
}

/// The shipped commit-msg sources `.aida/commit-config` as shell. Plain
/// `NAME=value` settings are supported; anything executable is refused
/// before any change.
#[test]
fn commit_config_must_be_literal_settings() {
    let fx = fixture();
    install(&fx.hooks.join("commit-msg"), &template("aida-commit-msg"));
    let config = fx.store.join(".aida/commit-config");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        "AIDA_COMMIT_STRICT=false\nREQUIRE_AI_TAG='false'\n# note\n",
    )
    .unwrap();
    fx.write_store_file("notes/pending.txt", "pending\n");
    let ok = fx.run(&["db", "sync"]);
    assert!(
        ok.status.success(),
        "literal config is supported: {}",
        text(&ok)
    );
    assert_eq!(fx.subjects("HEAD")[0], "chore: sync pending changes");

    std::fs::write(
        &config,
        "AIDA_COMMIT_STRICT=false\naida edit TASK-1 --title from-config\n",
    )
    .unwrap();
    fx.write_store_file("notes/second.txt", "second\n");
    let before = fx.head();
    let out = fx.run(&["db", "sync"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("not plain"), "{}", text(&out));
    assert_eq!(fx.head(), before);
    assert_eq!(git(&fx.store, &["diff", "--cached", "--name-only"]), "");
}

// ── F2: strict canonical reads ─────────────────────────────────────────────

/// An unparsable canonical mailbox record is an error for the digest, never
/// "absent": the stale local copy does not replace it, explicit and
/// automatic digests leave its bytes, the index and HEAD untouched.
#[test]
fn mailbox_digest_refuses_an_unreadable_canonical_record() {
    let fx = fixture();
    let local = fx.repo.join(".aida/mailbox");
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(local.join("m-old.json"), message_json("m-old", false)).unwrap();
    fx.write_store_file("mailbox/m-old.json", "{ not json");
    git(&fx.store, &["add", "-A"]);
    git(
        &fx.store,
        &["commit", "-qm", "chore: damaged canonical message"],
    );
    let before = fx.head();

    let out = fx.run(&["mailbox", "sync"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("m-old.json"), "{}", text(&out));
    assert_eq!(
        std::fs::read_to_string(fx.store.join("mailbox/m-old.json")).unwrap(),
        "{ not json"
    );
    assert_eq!(fx.head(), before);

    // The automatic digest in a pull is best-effort: warned, nothing written.
    let pull = fx.run(&["pull", "--store-only"]);
    assert!(pull.status.success(), "{}", text(&pull));
    assert!(
        text(&pull).contains("mailbox publish skipped"),
        "{}",
        text(&pull)
    );
    assert_eq!(
        std::fs::read_to_string(fx.store.join("mailbox/m-old.json")).unwrap(),
        "{ not json"
    );
    assert_eq!(git(&fx.store, &["status", "--porcelain"]), "");
}

/// A peer delivers an unparsable object: the sync's post-pull validation
/// reports the incomplete read and publishes nothing.
#[test]
fn sync_refuses_to_publish_after_pulling_an_unparsable_object() {
    let fx = fixture();
    let peer = fx.base.join("peer-bad");
    git(
        &fx.base,
        &[
            "clone",
            "-q",
            "-b",
            "aida-store",
            fx.origin.to_str().unwrap(),
            peer.to_str().unwrap(),
        ],
    );
    let object = task_1_object(&peer);
    std::fs::write(peer.join(&object), "id: [unclosed\n").unwrap();
    git(&peer, &["commit", "-qam", "chore: peer breaks an object"]);
    git(&peer, &["push", "-q", "origin", "aida-store"]);
    let peer_head = git(&peer, &["rev-parse", "HEAD"]);
    fx.write_store_file("notes/pending.txt", "pending\n");
    let out = fx.run(&["db", "sync", "--pull", "--push"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("did not read completely"),
        "{}",
        text(&out)
    );
    assert_eq!(fx.origin_head(), peer_head, "nothing published");
}

// ── F3: a failed local commit is never reported or pushed as done ──────────

fn objects_mention(fx: &Fx, needle: &str) -> bool {
    fn walk(dir: &Path, needle: &str) -> bool {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, needle)
                } else {
                    std::fs::read_to_string(&p).is_ok_and(|b| b.contains(needle))
                }
            })
    }
    walk(&fx.store.join("objects"), needle)
}

/// The tracked path of TASK-1's object in a store checkout.
fn task_1_object(store: &Path) -> String {
    git(store, &["ls-files", "objects"])
        .lines()
        .find(|l| l.ends_with("/TASK-1.yaml"))
        .expect("TASK-1 object is tracked")
        .to_string()
}

fn refusing_pre_commit(fx: &Fx) {
    install(
        &fx.hooks.join("pre-commit"),
        "#!/bin/sh\necho T1717-REFUSE >&2\nexit 1\n",
    );
}

#[test]
fn push_store_leg_reports_a_failed_commit_and_pushes_nothing() {
    let fx = fixture();
    refusing_pre_commit(&fx);
    fx.write_store_file("notes/pending.txt", "pending\n");
    let origin = fx.origin_head();
    let before = fx.head();
    let out = fx.run(&["push", "--store-only"]);
    let t = text(&out);
    assert!(!out.status.success(), "{t}");
    assert!(
        t.contains("T1717-REFUSE"),
        "the hook's own error survives: {t}"
    );
    assert!(
        !t.contains("Committed:") && !t.contains("store push complete"),
        "{t}"
    );
    assert_eq!(fx.origin_head(), origin);
    assert_eq!(fx.head(), before);
    assert!(fx.store.join("notes/pending.txt").exists());

    // No origin: still a failed leg, not a silent success.
    git(&fx.repo, &["remote", "remove", "origin"]);
    let out = fx.run(&["push", "--store-only"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("T1717-REFUSE"), "{}", text(&out));

    // Healthy controls: the same leg commits, with and without origin.
    fx.install_barrier_hooks(&["pre-commit"]);
    let out = fx.run(&["push", "--store-only"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("Committed:"), "{}", text(&out));
    assert_eq!(git(&fx.store, &["status", "--porcelain"]), "");
    git(
        &fx.repo,
        &["remote", "add", "origin", fx.origin.to_str().unwrap()],
    );
    fx.write_store_file("notes/more.txt", "more\n");
    let out = fx.run(&["push", "--store-only"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(fx.origin_head(), fx.head());
}

#[test]
fn pull_and_allocation_report_a_failed_pending_commit() {
    let fx = fixture();
    fx.peer_push("peer.txt", "peer\n");
    refusing_pre_commit(&fx);
    fx.write_store_file("notes/pending.txt", "pending\n");
    let before = fx.head();
    let pull = fx.run(&["pull", "--store-only"]);
    assert!(!pull.status.success(), "{}", text(&pull));
    assert!(text(&pull).contains("T1717-REFUSE"), "{}", text(&pull));
    assert!(
        !text(&pull).contains("store pull complete"),
        "{}",
        text(&pull)
    );
    assert_eq!(fx.head(), before, "no pull after the failed commit");

    let add = fx.run(&["add", "never allocated", "--type", "task"]);
    assert!(!add.status.success(), "{}", text(&add));
    assert!(
        text(&add).contains("before id allocation"),
        "{}",
        text(&add)
    );
    assert_eq!(fx.head(), before);
    assert!(
        !objects_mention(&fx, "never allocated"),
        "no spec was allocated"
    );

    fx.install_barrier_hooks(&["pre-commit"]);
    let pull = fx.run(&["pull", "--store-only"]);
    assert!(pull.status.success(), "{}", text(&pull));
    assert_eq!(fx.subjects("HEAD")[0], "chore: sync pending changes");
}

// ── F4: remote reconcile failure and no-op paths ───────────────────────────

/// After a clean merge, an unreadable block registry fails validation; the
/// failure goes through the checked rollback (store back at the locked
/// preimage, said truthfully). With the rollback itself made to fail, the
/// report says so instead of claiming the store is unchanged.
#[test]
fn remote_reconcile_checks_its_rollback() {
    let fx = fixture();
    let peer = fx.base.join("peer-blocks");
    git(
        &fx.base,
        &[
            "clone",
            "-q",
            "-b",
            "aida-store",
            fx.origin.to_str().unwrap(),
            peer.to_str().unwrap(),
        ],
    );
    std::fs::create_dir_all(peer.join("registry")).unwrap();
    std::fs::write(peer.join("registry/blocks.yaml"), "blocks: [unclosed\n").unwrap();
    git(&peer, &["add", "-A"]);
    git(&peer, &["commit", "-qm", "chore: peer breaks blocks"]);
    git(&peer, &["push", "-q", "origin", "aida-store"]);
    let pre = fx.local_commit("local.txt");
    let out = fx.run(&["remote", "reconcile", "--execute", "--yes"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("back at"), "{}", text(&out));
    assert_eq!(fx.head(), pre);
    assert_eq!(git(&fx.store, &["status", "--porcelain"]), "");

    // Make the rollback fail: the post-merge hook leaves an index lock.
    let git_dir = PathBuf::from(git(&fx.store, &["rev-parse", "--absolute-git-dir"]));
    install(
        &fx.hooks.join("post-merge"),
        &format!(
            "#!/bin/sh\n: > '{}'\nexit 0\n",
            git_dir.join("index.lock").display()
        ),
    );
    let out = fx.run(&["remote", "reconcile", "--execute", "--yes"]);
    let _ = std::fs::remove_file(git_dir.join("index.lock"));
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("rollback to"), "{}", text(&out));
    assert!(text(&out).contains("FAILED"), "{}", text(&out));
    assert!(!text(&out).contains("back at"), "{}", text(&out));
}

/// Discovery sees every hub in sync; a local commit lands before the lock.
/// The locked decision sees it and publishes it instead of reporting
/// "nothing to reconcile".
#[test]
fn remote_reconcile_decides_noop_under_the_lock() {
    let fx = fixture();
    let holder = hold_lock(&fx, "");
    let mut rec = fx.spawn(&["remote", "reconcile", "--execute", "--yes"], true);
    assert_parked_on_lock(&mut rec, &fx.store, "remote reconcile --execute");
    let late = fx.local_commit("late.txt");
    holder.release();
    let out = finish(rec, "remote reconcile");
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        !text(&out).contains("nothing to reconcile"),
        "{}",
        text(&out)
    );
    assert_eq!(fx.origin_head(), late);
    // And a genuine no-op still says so.
    let out = fx.run(&["remote", "reconcile", "--execute", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("nothing to reconcile"),
        "{}",
        text(&out)
    );
}

// ── F6: remaining routes and lifetimes ─────────────────────────────────────

/// Per-write auto-push: the edit's commit is captured under the lock and the
/// timed push of that SHA runs with the lock free.
#[test]
fn per_write_auto_push_publishes_the_captured_commit_unlocked() {
    let fx = fixture();
    let cfg = fx.repo.join(".aida/config.toml");
    let body = std::fs::read_to_string(&cfg).unwrap().replacen(
        "auto_push = \"manual\"",
        "auto_push = \"per-write\"",
        1,
    );
    assert!(
        body.contains("auto_push = \"per-write\""),
        "scaffolded auto_push key"
    );
    std::fs::write(&cfg, body).unwrap();
    let out = fx
        .cmd(
            &fx.repo,
            &["edit", "TASK-1", "--title", "auto pushed"],
            true,
        )
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("auto-pushed"), "{}", text(&out));
    assert_eq!(fx.origin_head(), fx.head());
    assert_eq!(pushed_shas(&fx), vec![fx.head()]);
    assert_push_ran_unlocked(&fx);
}

/// A sibling linked worktree of the project discovers the same store and so
/// the same lock (distinct from the symlink case).
#[test]
fn sibling_worktree_discovers_the_same_lock() {
    let fx = fixture();
    let sibling = fx.base.join("sibling");
    git(
        &fx.repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            sibling.to_str().unwrap(),
        ],
    );
    fx.write_store_file("notes/pending.txt", "pending\n");
    fx.arm("pre-commit");
    let first = fx.spawn(&["db", "sync"], true);
    fx.wait_reached("pre-commit");
    let mut second = fx
        .cmd(
            &sibling,
            &["db", "sync", "-m", "chore: sibling sync"],
            false,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert_parked_on_lock(&mut second, &fx.store, "db sync from a sibling worktree");
    fx.release("pre-commit");
    assert!(finish(first, "first").status.success());
    let out = finish(second, "sibling");
    assert!(out.status.success(), "{}", text(&out));
}

/// A writer killed while holding the lock releases it with the process; the
/// permanent lock file (same inode) stays and the next writer proceeds.
#[test]
fn killed_holder_releases_the_lock_and_the_sidecar_survives() {
    use std::os::unix::fs::MetadataExt;
    let fx = fixture();
    let inode = std::fs::metadata(lock_path(&fx.store)).unwrap().ino();
    let mut holder = hold_lock(&fx, "");
    fx.write_store_file("notes/pending.txt", "pending\n");
    let mut sync = fx.spawn(&["db", "sync"], false);
    assert_parked_on_lock(&mut sync, &fx.store, "db sync");
    holder.kill();
    let out = finish(sync, "db sync");
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        std::fs::metadata(lock_path(&fx.store)).unwrap().ino(),
        inode
    );
    assert_eq!(fx.subjects("HEAD")[0], "chore: sync pending changes");
}

/// Missing sentinel (no pre-commit, no sentinel file): the detector runs
/// under the held lock and records the bypass. Matching sentinel with
/// telemetry ON: nothing recorded.
#[test]
fn missing_and_matching_sentinel_paths() {
    let fx = fixture();
    install(
        &fx.hooks.join("post-commit"),
        &template("aida-post-commit.sh"),
    );
    let _ = std::fs::remove_file(fx.hooks.join("pre-commit"));
    let common = PathBuf::from(git(
        &fx.store,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ));
    let _ = std::fs::remove_file(common.join("aida-precommit-sentinel"));
    fx.write_store_file("notes/pending.txt", "pending\n");
    let out = fx
        .cmd(&fx.repo, &["db", "sync"], true)
        .env("AIDA_TELEMETRY", "1")
        .env("AIDA_FIELD_STUDY", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let log = fx.home.join(".aida/rule-violations.jsonl");
    assert!(std::fs::read_to_string(&log)
        .unwrap()
        .contains("no-verify-bypass"));
    std::fs::remove_file(&log).unwrap();

    install(
        &fx.hooks.join("pre-commit"),
        &template("aida-pre-commit.sh"),
    );
    fx.write_store_file("notes/second.txt", "second\n");
    let out = fx
        .cmd(&fx.repo, &["db", "sync"], true)
        .env("AIDA_TELEMETRY", "1")
        .env("AIDA_FIELD_STUDY", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert!(!log.exists(), "matching sentinel records nothing");
}

/// Full observable store state: HEAD, index entries, status, admin markers
/// and every worktree file's content hash (runtime `.aida/` excluded).
fn snapshot(fx: &Fx) -> String {
    use std::hash::{Hash, Hasher};
    let out = |args: &[&str]| {
        let o = git_try(&fx.store, args);
        format!(
            "{:?}|{}|{}",
            o.status.code(),
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    };
    let git_dir = PathBuf::from(git(&fx.store, &["rev-parse", "--absolute-git-dir"]));
    let admin: Vec<String> = [
        "rebase-merge",
        "rebase-apply",
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "index.lock",
    ]
    .iter()
    .map(|f| format!("{f}={}", git_dir.join(f).exists()))
    .collect();
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            let rel = p.strip_prefix(root).unwrap().display().to_string();
            if rel == ".git" || rel == ".aida" {
                continue;
            }
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                std::fs::read(&p).unwrap_or_default().hash(&mut h);
                out.push(format!("{rel}:{:x}", h.finish()));
            }
        }
    }
    let mut files = Vec::new();
    walk(&fx.store, &fx.store, &mut files);
    format!(
        "{}\n{}\n{}\n{}\n{}",
        out(&["rev-parse", "HEAD"]),
        out(&["ls-files", "-s"]),
        out(&["status", "--porcelain", "-uall"]),
        admin.join(","),
        files.join("\n")
    )
}

fn retitle_on_peer(fx: &Fx, name: &str, title: &str) -> String {
    let peer = fx.base.join(name);
    git(
        &fx.base,
        &[
            "clone",
            "-q",
            "-b",
            "aida-store",
            fx.origin.to_str().unwrap(),
            peer.to_str().unwrap(),
        ],
    );
    let object = task_1_object(&peer);
    let body: String = std::fs::read_to_string(peer.join(&object))
        .unwrap()
        .lines()
        .map(|l| {
            if l.starts_with("title:") {
                format!("title: {title}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(peer.join(&object), body).unwrap();
    git(&peer, &["commit", "-qam", "chore: peer retitle"]);
    git(&peer, &["push", "-q", "origin", "aida-store"]);
    object
}

fn set_auto_push(fx: &Fx, mode: &str) {
    let cfg = fx.repo.join(".aida/config.toml");
    let body: String = std::fs::read_to_string(&cfg)
        .unwrap()
        .lines()
        .map(|l| {
            if l.starts_with("auto_push = ") {
                format!("auto_push = \"{mode}\"")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert!(body.contains(&format!("auto_push = \"{mode}\"")));
    std::fs::write(&cfg, body).unwrap();
}

// ── B1: the profile the real scaffolder installs ───────────────────────────

/// Default `aida init` (generated-header hooks in `.git/hooks`, no
/// `core.hooksPath`): every A4 route runs to success and the hooks' effects
/// are intact — the store-pair trailer pins the prior head, and every store
/// commit ran its pre-commit (sentinel written and consumed; with the field
/// study on, no bypass is recorded).
#[test]
fn scaffolded_hook_profile_runs_every_route() {
    let fx = fixture_with(true);
    let pre_commit = std::fs::read_to_string(fx.repo.join(".git/hooks/pre-commit")).unwrap();
    assert!(
        pre_commit.contains("# AIDA Generated: v"),
        "real scaffolded hook: {pre_commit:.200}"
    );
    assert!(git_try(&fx.repo, &["config", "core.hooksPath"])
        .stdout
        .is_empty());
    let study = |args: &[&str]| {
        let out = fx
            .cmd(&fx.repo, args, false)
            .env("AIDA_TELEMETRY", "1")
            .env("AIDA_FIELD_STUDY", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {}", text(&out));
        out
    };
    fx.peer_push("peer.txt", "peer\n");
    fx.write_store_file("notes/pending.txt", "pending\n");
    let before = fx.head();
    study(&["db", "sync", "--pull", "--push"]);
    assert_eq!(fx.origin_head(), fx.head());
    let pending_commit = git(
        &fx.store,
        &[
            "log",
            "--format=%B",
            "--grep",
            "chore: sync pending changes",
            "-1",
        ],
    );
    assert!(
        pending_commit.contains(&format!("Aida-Store: {before}")),
        "trailer: {pending_commit}"
    );
    let common = PathBuf::from(git(
        &fx.store,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ));
    assert!(
        !common.join("aida-precommit-sentinel").exists(),
        "sentinel written and consumed"
    );
    // Pre-existing detector behavior (not this change): git's rebase
    // sequencer runs post-commit, but not pre-commit, for each commit it
    // replays, so the shipped detector records the one replayed local commit
    // as a bypass. Exactly one record: the sync's own commit ran pre-commit.
    let log = fx.home.join(".aida/rule-violations.jsonl");
    let replayed = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(replayed.lines().count(), 1, "{replayed}");
    std::fs::remove_file(&log).unwrap();

    fx.write_store_file("notes/pull.txt", "pull\n");
    study(&["pull", "--store-only"]);
    fx.write_store_file("notes/push.txt", "push\n");
    study(&["push", "--store-only"]);
    assert_eq!(fx.origin_head(), fx.head());
    fx.peer_push("peer2.txt", "peer\n");
    study(&["add", "scaffolded add", "--type", "task"]);
    assert!(fx
        .subjects(&fx.origin_head())
        .iter()
        .any(|s| s.contains("scaffolded add")));
    let local = fx.repo.join(".aida/mailbox");
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(
        local.join("m-scaffold.json"),
        message_json("m-scaffold", false),
    )
    .unwrap();
    study(&["mailbox", "sync"]);
    assert_eq!(fx.subjects("HEAD")[0], "mailbox: digest 1 message(s)");
    study(&["db", "merge-gate"]);
    let violations =
        std::fs::read_to_string(fx.home.join(".aida/rule-violations.jsonl")).unwrap_or_default();
    assert!(
        violations.is_empty(),
        "every later store commit ran its pre-commit: no bypass recorded: {violations}"
    );
    assert_eq!(git(&fx.store, &["status", "--porcelain"]), "");
}

/// An AIDA-generated hook whose body is not this build's shipped hook
/// (another version, or edited) is refused before any change, naming the
/// upgrade path; an older header over the shipped body is accepted.
#[test]
fn drifted_generated_hook_is_refused_with_upgrade_guidance() {
    let fx = fixture_with(true);
    let hook = fx.repo.join(".git/hooks/pre-commit");
    let original = std::fs::read_to_string(&hook).unwrap();
    let older = original.replacen("# AIDA Generated: v", "# AIDA Generated: v0.0.1-", 1);
    std::fs::write(&hook, &older).unwrap();
    fx.write_store_file("notes/pending.txt", "pending\n");
    let ok = fx.run(&["db", "sync"]);
    assert!(
        ok.status.success(),
        "older header, shipped body: {}",
        text(&ok)
    );

    std::fs::write(&hook, format!("{}\necho drifted\n", older.trim_end())).unwrap();
    fx.write_store_file("notes/second.txt", "second\n");
    let before = snapshot(&fx);
    let out = fx.run(&["db", "sync"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("aida upgrade"), "{}", text(&out));
    assert_eq!(snapshot(&fx), before, "nothing changed");
}

// ── B2: a failing `git status` is never "clean" ────────────────────────────

/// Make the store's index unreadable for the current (non-root) user; HEAD
/// stays readable, so `git status` exits non-zero with empty output.
fn break_index(fx: &Fx) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let index = PathBuf::from(git(
        &fx.store,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    ));
    std::fs::set_permissions(&index, std::fs::Permissions::from_mode(0o000)).unwrap();
    let st = git_try(&fx.store, &["status", "--porcelain"]);
    assert!(
        !st.status.success(),
        "this host reads a mode-000 index (root?); the failure cannot be induced"
    );
    index
}

fn mend_index(index: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(index, std::fs::Permissions::from_mode(0o644)).unwrap();
}

#[test]
fn unreadable_status_fails_push_and_auto_push_without_false_success() {
    let fx = fixture();
    let local = fx.repo.join(".aida/mailbox");
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(local.join("m-b2.json"), message_json("m-b2", false)).unwrap();
    let origin = fx.origin_head();
    let index = break_index(&fx);
    let before = snapshot(&fx);
    let out = fx.run(&["push", "--store-only"]);
    let t = text(&out);
    assert!(!out.status.success(), "{t}");
    assert!(t.contains("cannot read the store status"), "{t}");
    for lie in ["published", "store push complete", "Committed"] {
        assert!(!t.contains(lie), "false `{lie}`: {t}");
    }
    assert!(
        !fx.store.join("mailbox/m-b2.json").exists(),
        "no canonical write"
    );
    assert_eq!(snapshot(&fx), before);
    assert_eq!(fx.origin_head(), origin);

    // Per-write auto-push defers with the real error, never "auto-pushed".
    set_auto_push(&fx, "per-write");
    let out = fx.run(&["edit", "TASK-1", "--title", "while broken"]);
    let t = text(&out);
    assert!(!t.contains("auto-pushed"), "{t}");
    assert_eq!(fx.origin_head(), origin);

    // Allocation and reconcile stop on the same failure.
    let add = fx.run(&["add", "not while broken", "--type", "task"]);
    assert!(!add.status.success(), "{}", text(&add));
    assert!(text(&add).contains("git status failed"), "{}", text(&add));
    fx.peer_push("peer.txt", "peer\n");
    let rec = fx.run(&["remote", "reconcile", "--execute", "--yes"]);
    assert!(!rec.status.success(), "{}", text(&rec));
    assert!(text(&rec).contains("git status failed"), "{}", text(&rec));

    // Healthy control once the index is readable again.
    mend_index(&index);
    set_auto_push(&fx, "manual");
    let pull = fx.run(&["pull", "--store-only"]);
    assert!(pull.status.success(), "{}", text(&pull));
    let out = fx.run(&["push", "--store-only"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(fx.store.join("mailbox/m-b2.json").exists());
    assert_eq!(fx.origin_head(), fx.head());
}

// ── Section 7 gaps ─────────────────────────────────────────────────────────

/// Group 1: the rebase this sync started conflicts and its automatic abort
/// FAILS. Both failures are reported, the rebase state is kept as git left
/// it, the next writer refuses it without changing anything, and manual
/// recovery restores the local commit.
#[test]
fn owned_rebase_abort_failure_is_reported_and_preserved() {
    let fx = fixture();
    fx.peer_push("clash.txt", "origin side\n");
    fx.write_store_file("notes/clash.txt", "local side\n");
    git(&fx.store, &["add", "-A"]);
    git(&fx.store, &["commit", "-qm", "chore: local clash"]);
    let local = fx.head();
    install(
        &fx.hooks.join("reference-transaction"),
        "#!/bin/sh\n[ \"$1\" = prepared ] || exit 0\ncase \"$(ps -o args= -p $PPID)\" in *rebase*--abort*) echo T1717-ABORT-BLOCKED >&2; exit 1;; esac\nexit 0\n",
    );
    let out = fx.run(&["db", "sync", "--pull"]);
    let t = text(&out);
    assert!(!out.status.success(), "{t}");
    assert!(t.contains("rebase --abort` also failed"), "{t}");
    let git_dir = PathBuf::from(git(&fx.store, &["rev-parse", "--absolute-git-dir"]));
    assert!(
        git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists(),
        "{t}"
    );
    let kept = snapshot(&fx);
    let edit = fx.run(&["edit", "TASK-1", "--title", "onto a broken rebase"]);
    assert!(!edit.status.success(), "{}", text(&edit));
    assert!(
        text(&edit).contains("rebase in progress"),
        "{}",
        text(&edit)
    );
    assert_eq!(snapshot(&fx), kept, "the refused writer changed nothing");
    std::fs::remove_file(fx.hooks.join("reference-transaction")).unwrap();
    git(&fx.store, &["rebase", "--abort"]);
    assert_eq!(fx.head(), local, "the local commit survives");
}

/// Group 4: a configured but unreachable origin is not "no origin": `add`
/// files locally and says so; per-write auto-push keeps the commit and
/// defers.
#[test]
fn unreachable_origin_files_locally_and_defers() {
    let fx = fixture();
    git(
        &fx.repo,
        &[
            "remote",
            "set-url",
            "origin",
            "/nonexistent/t1717/origin.git",
        ],
    );
    let add = fx.run(&["add", "filed offline", "--type", "task"]);
    assert!(add.status.success(), "{}", text(&add));
    assert!(text(&add).contains("unreachable"), "{}", text(&add));
    assert!(fx
        .subjects("HEAD")
        .iter()
        .any(|s| s.contains("filed offline")));
    set_auto_push(&fx, "per-write");
    let out = fx.run(&["edit", "TASK-1", "--title", "edited offline"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("push deferred"), "{}", text(&out));
    assert_eq!(fx.subjects("HEAD")[0], "update TASK-1");
}

/// Group 4: both sides edit the same spec object; the pull's window unions
/// the structural conflict and the rebase completes.
#[test]
fn pull_window_unions_a_same_spec_conflict() {
    let fx = fixture();
    let object = retitle_on_peer(&fx, "peer-same", "peer side");
    fx.run_ok(&["edit", "TASK-1", "--title", "local side"]);
    let out = fx.run(&["pull", "--store-only"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("auto-merged"), "{}", text(&out));
    let merged = std::fs::read_to_string(fx.store.join(&object)).unwrap();
    assert!(
        merged.contains("local side") || merged.contains("peer side"),
        "{merged}"
    );
    assert_eq!(git(&fx.store, &["status", "--porcelain"]), "");
    let git_dir = PathBuf::from(git(&fx.store, &["rev-parse", "--absolute-git-dir"]));
    assert!(!git_dir.join("rebase-merge").exists());
}

/// Group 5: with another process holding the cache refresh lock, a pulling
/// sync completes without waiting on it (no cache refresh inside the store
/// window); a cache-backed `list` meanwhile does not show the pulled title
/// (the holder is real); once released, `list` serves it.
#[test]
fn stale_cache_with_a_competing_refresh_holder() {
    let fx = fixture();
    fx.run_ok(&["list"]);
    retitle_on_peer(&fx, "peer-title", "peer retitled");
    let cache = aida_core::CachedGitBackend::default_cache_path(&fx.store);
    let refresh = aida_core::db::cache_sidecar_path(&cache, "refresh.lock");
    let release = fx.base.join("release-refresh");
    let held = fx.base.join("held-refresh");
    let mut holder = Command::new("flock")
        .arg(&refresh)
        .args([
            "-c",
            &format!(
                ": > '{}'; while [ ! -f '{}' ]; do sleep 0.02; done",
                held.display(),
                release.display()
            ),
        ])
        .spawn()
        .unwrap();
    wait_for(&held, "refresh holder");
    let started = Instant::now();
    let out = fx.run(&["db", "sync", "--pull"]);
    let sync_time = started.elapsed();
    let during = fx.run(&["list"]);
    std::fs::write(&release, "").unwrap();
    holder.wait().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        sync_time < Duration::from_secs(20),
        "the store window never waits on a cache refresh"
    );
    assert!(
        !text(&during).contains("peer retitled"),
        "refresh was really held: {}",
        text(&during)
    );
    let after = fx.run(&["list"]);
    assert!(after.status.success(), "{}", text(&after));
    assert!(text(&after).contains("peer retitled"), "{}", text(&after));
}

/// Group 5: opportunistic `git gc --auto` runs after the store window: its
/// pre-auto-gc hook sees the store lock free and no store lock held by aida.
#[test]
fn gc_runs_after_the_store_window() {
    let fx = fixture();
    fx.install_barrier_hooks(&["pre-auto-gc"]);
    git(&fx.store, &["config", "gc.autoPackLimit", "1"]);
    for f in ["p1.txt", "p2.txt"] {
        fx.local_commit(f);
        git(&fx.store, &["repack", "-q"]);
    }
    git(&fx.store, &["push", "-q", "origin", "aida-store"]);
    fx.peer_push("peer.txt", "peer\n");
    let out = fx
        .cmd(&fx.repo, &["db", "sync", "--pull"], true)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let lines = fx.obs_lines("pre-auto-gc");
    assert!(!lines.is_empty(), "gc --auto ran its hook");
    for line in &lines {
        if let Some(v) = line.strip_prefix("lock=") {
            assert_eq!(v, "free", "{lines:?}");
        }
        if let Some(v) = line.strip_prefix("aida_locks=") {
            assert!(fixture_locks(&fx, v).is_empty(), "{lines:?}");
        }
    }
}
