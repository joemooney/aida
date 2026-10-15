#![cfg(unix)]
//! BUG-1926: the identity push gate must govern pushes from the linked
//! `.aida-store` worktree with the OWNING project's `[identity]
//! allowed_emails`. Before the fix, `aida identity check-push` resolved the
//! store worktree as the project root, found no `.aida/config.toml` there, and
//! passed every push; the generated hook's no-`aida` fallback read the same
//! cwd-relative (absent) config.
//!
//! Paired controls over three entry points — the generated hook running the
//! real `aida identity check-push`, the hook's fallback with no `aida` on
//! PATH, and `aida_core::git_ops::push(store_root, ..)` — plus the explicit
//! no-config policy. Everything is hermetic: isolated HOME, temp repos, a
//! bare temp "remote", no network.
// trace:BUG-1926 | ai:claude

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;

const ALLOWED: &str = "allowed@example.org";
const FORBIDDEN: &str = "someone@work.example.com";

/// `git_ops::push` inherits the process environment, so the test that drives
/// it mutates HOME/PATH; every test holds this lock so none observes it.
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    store: PathBuf,
    remote: PathBuf,
    /// PATH with the freshly built `aida` first.
    path_with_aida: String,
    /// PATH with no `aida` anywhere (exercises the hook fallback).
    path_without_aida: String,
}

/// PATH entries that do not hold an `aida` binary, optionally led by `first`.
fn sanitized_path(first: Option<&Path>) -> String {
    let orig = std::env::var_os("PATH").unwrap_or_default();
    let dirs = first
        .map(Path::to_path_buf)
        .into_iter()
        .chain(std::env::split_paths(&orig).filter(|d| !d.join("aida").exists()));
    std::env::join_paths(dirs)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn git_cmd(f: &Fixture, dir: &Path, path_env: &str) -> Command {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(dir)
        .env("HOME", &f.home)
        .env("PATH", path_env)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_OUTPUT_FORMAT", "human")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("AIDA_STORE")
        .env_remove("AIDA_DEBUG_IDENTITY");
    c
}

fn text(out: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn git_ok(f: &Fixture, dir: &Path, args: &[&str]) {
    let out = git_cmd(f, dir, &f.path_with_aida)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", text(&out));
}

/// Project repo (optionally carrying an identity allowlist), an attached
/// orphan `.aida-store` worktree on `aida-store` with NO config of its own,
/// and a bare fake remote. `install_hook` runs the real `aida identity
/// install-hook` from the project.
fn fixture(allowlist: Option<&str>, install_hook: bool) -> Fixture {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let home = root.join("home");
    let bin = root.join("bin");
    let project = root.join("project");
    let remote = root.join("remote.git");
    for d in [&home, &bin, &project] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_aida"), bin.join("aida")).unwrap();
    let path_without_aida = sanitized_path(None);
    let path_with_aida = sanitized_path(Some(&bin));
    let f = Fixture {
        _tmp: tmp,
        home,
        store: project.join(".aida-store"),
        project,
        remote,
        path_with_aida,
        path_without_aida,
    };
    git_ok(&f, &root, &["init", "-q", "--bare", "remote.git"]);
    let p = f.project.clone();
    git_ok(&f, &p, &["init", "-q", "-b", "main"]);
    git_ok(&f, &p, &["config", "user.name", "Tester"]);
    git_ok(&f, &p, &["config", "user.email", ALLOWED]);
    git_ok(
        &f,
        &p,
        &["remote", "add", "origin", f.remote.to_str().unwrap()],
    );
    std::fs::create_dir_all(p.join(".aida")).unwrap();
    let cfg = match allowlist {
        Some(email) => format!("[identity]\nallowed_emails = [\"{email}\"]\n"),
        None => "[hints]\nworkflow_hints = true\n".to_string(),
    };
    std::fs::write(p.join(".aida/config.toml"), cfg).unwrap();
    std::fs::write(p.join(".gitignore"), ".aida-store/\n").unwrap();
    git_ok(&f, &p, &["add", "-A"]);
    git_ok(&f, &p, &["commit", "-q", "-m", "init"]);
    // Attached orphan store worktree, as `aida init` lays it out.
    git_ok(
        &f,
        &p,
        &["worktree", "add", "-q", "--detach", ".aida-store"],
    );
    let s = f.store.clone();
    git_ok(&f, &s, &["checkout", "-q", "--orphan", "aida-store"]);
    git_ok(&f, &s, &["rm", "-rfq", "."]);
    std::fs::create_dir_all(s.join("objects")).unwrap();
    std::fs::write(s.join("objects/.keep"), "").unwrap();
    git_ok(&f, &s, &["add", "-A"]);
    git_ok(&f, &s, &["commit", "-q", "-m", "store init"]);
    assert!(
        !s.join(".aida/config.toml").exists(),
        "the store worktree must not carry the project config"
    );
    if install_hook {
        let out = Command::new(env!("CARGO_BIN_EXE_aida"))
            .current_dir(&p)
            .env("HOME", &f.home)
            .env("PATH", &f.path_with_aida)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("AIDA_TELEMETRY", "0")
            .env_remove("AIDA_STORE")
            .args(["identity", "install-hook"])
            .output()
            .unwrap();
        assert!(out.status.success(), "install-hook: {}", text(&out));
    }
    f
}

/// Commit in the store worktree as `email` (author and committer).
fn store_commit_as(f: &Fixture, email: &str) {
    std::fs::write(f.store.join(format!("objects/{email}.yaml")), "x: 1\n").unwrap();
    git_ok(f, &f.store, &["add", "-A"]);
    let out = git_cmd(f, &f.store, &f.path_with_aida)
        .env("GIT_AUTHOR_EMAIL", email)
        .env("GIT_COMMITTER_EMAIL", email)
        .args(["commit", "-q", "-m", "store write"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
}

fn push_store(f: &Fixture, path_env: &str) -> Output {
    git_cmd(f, &f.store, path_env)
        .args(["push", "origin", "aida-store"])
        .output()
        .unwrap()
}

fn remote_has_store_branch(f: &Fixture) -> bool {
    Command::new("git")
        .arg("--git-dir")
        .arg(&f.remote)
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/aida-store"])
        .output()
        .unwrap()
        .status
        .success()
}

/// Restores the captured process environment variables on drop (including
/// during a panic unwind).
struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvGuard {
    fn capture(keys: &[&'static str]) -> Self {
        EnvGuard(keys.iter().map(|k| (*k, std::env::var_os(k))).collect())
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- generated hook -> real `aida identity check-push` ------------------

#[test]
fn hook_refuses_forbidden_store_push_under_project_allowlist() {
    let _g = lock();
    let f = fixture(Some(ALLOWED), true);
    // A store-local config that WOULD allow the forbidden email must be
    // ignored: the owning project's allowlist governs.
    std::fs::create_dir_all(f.store.join(".aida")).unwrap();
    std::fs::write(
        f.store.join(".aida/config.toml"),
        format!("[identity]\nallowed_emails = [\"{FORBIDDEN}\"]\n"),
    )
    .unwrap();
    store_commit_as(&f, FORBIDDEN);
    let out = push_store(&f, &f.path_with_aida);
    let t = text(&out);
    assert!(!out.status.success(), "forbidden push must be refused: {t}");
    assert!(t.contains(FORBIDDEN), "{t}");
    assert!(
        t.contains("owning project of the store worktree"),
        "refusal must name the allowlist's provenance: {t}"
    );
    assert!(!remote_has_store_branch(&f));
}

#[test]
fn hook_allows_allowlisted_store_push_under_project_allowlist() {
    let _g = lock();
    let f = fixture(Some(ALLOWED), true);
    store_commit_as(&f, ALLOWED);
    let out = push_store(&f, &f.path_with_aida);
    assert!(out.status.success(), "{}", text(&out));
    assert!(remote_has_store_branch(&f));
}

// ---- generated hook fallback (no `aida` on PATH) ------------------------

#[test]
fn hook_fallback_refuses_store_push_when_owning_project_has_allowlist() {
    let _g = lock();
    let f = fixture(Some(ALLOWED), true);
    store_commit_as(&f, FORBIDDEN);
    let out = push_store(&f, &f.path_without_aida);
    let t = text(&out);
    assert!(!out.status.success(), "fallback must fail closed: {t}");
    let owner_cfg = f.project.join(".aida/config.toml");
    assert!(
        t.contains(&format!("configured in {}", owner_cfg.display())),
        "fallback must read the owning project's config: {t}"
    );
    assert!(!remote_has_store_branch(&f));
}

#[test]
fn hook_fallback_allows_store_push_when_owning_project_has_no_allowlist() {
    let _g = lock();
    let f = fixture(None, true);
    store_commit_as(&f, FORBIDDEN);
    let out = push_store(&f, &f.path_without_aida);
    assert!(out.status.success(), "{}", text(&out));
    assert!(remote_has_store_branch(&f));
}

fn is_path_isolated_process() -> bool {
    let Ok(name) = std::env::var("AIDA_TEST_PATH_CHILD") else {
        return false;
    };
    let args: Vec<_> = std::env::args().skip(1).collect();
    !name.is_empty()
        && args.len() == 4
        && args[0] == "--exact"
        && args[1] == "--nocapture"
        && args[2] == "--test-threads=1"
        && args[3] == name
}

fn require_path_mutation_scope(key: &str) {
    if key == "PATH" || key == "HOME" {
        assert!(
            is_path_isolated_process(),
            "{key} mutation requires run_path_isolated_test() before any fixture or env guard"
        );
    }
}

fn run_path_isolated_test() -> bool {
    let thread = std::thread::current();
    let name = thread.name().expect("libtest thread name");
    if is_path_isolated_process() {
        assert_eq!(std::env::var("AIDA_TEST_PATH_CHILD").unwrap(), name);
        return false;
    }
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".gitconfig"),
        "[user]\nname = AIDA Test\nemail = test@example.invalid\n[init]\ndefaultBranch = main\n",
    )
    .unwrap();
    let mut command = std::process::Command::new(
        std::env::current_exe().expect("resolve current test executable"),
    );
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("AIDA_TEST_PATH_CHILD", name)
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .arg(name);
    let output = command.output().expect("spawn isolated test runner");
    if !output.status.success() {
        panic!(
            "path-isolated test `{name}` failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    true
}

// ---- aida_core::git_ops::push(store_root, ..) ---------------------------

#[test]
fn git_ops_push_from_store_root_is_gated_by_project_allowlist() {
    if run_path_isolated_test() {
        return;
    }
    let _g = lock();
    let forbidden = fixture(Some(ALLOWED), true);
    let allowed = fixture(Some(ALLOWED), true);
    store_commit_as(&forbidden, FORBIDDEN);
    store_commit_as(&allowed, ALLOWED);

    // git_ops::push spawns git with the process environment; the guard
    // restores every variable it touches even if a push panics.
    let env = EnvGuard::capture(&[
        "HOME",
        "PATH",
        "GIT_CONFIG_NOSYSTEM",
        "AIDA_TELEMETRY",
        "AIDA_STORE",
    ]);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("AIDA_TELEMETRY", "0");
    std::env::remove_var("AIDA_STORE");

    require_path_mutation_scope("HOME");
    std::env::set_var("HOME", &forbidden.home);
    require_path_mutation_scope("PATH");
    std::env::set_var("PATH", &forbidden.path_with_aida);
    let refused = aida_core::git_ops::push(&forbidden.store, "origin", "aida-store");

    require_path_mutation_scope("HOME");
    std::env::set_var("HOME", &allowed.home);
    require_path_mutation_scope("PATH");
    std::env::set_var("PATH", &allowed.path_with_aida);
    let accepted = aida_core::git_ops::push(&allowed.store, "origin", "aida-store");
    drop(env);

    assert!(
        !matches!(refused, Ok(true)),
        "forbidden store push must not succeed: {refused:?}"
    );
    assert!(!remote_has_store_branch(&forbidden));
    assert!(
        matches!(accepted, Ok(true)),
        "allowlisted store push must succeed: {accepted:?}"
    );
    assert!(remote_has_store_branch(&allowed));
}

// ---- explicit no-config policy ------------------------------------------

/// DECISION (BUG-1926): a project with no `[identity] allowed_emails`
/// (config absent, or present without the key) has the identity gate OFF —
/// it is opt-in per project. Pushes of any author pass, from the store
/// worktree exactly as from the project checkout. This pins that policy so
/// it cannot change by accident.
#[test]
fn no_config_policy_gate_is_off_for_store_and_code_pushes() {
    let _g = lock();
    let f = fixture(None, true);
    // Fully absent config (the fallback-test fixture covers present-without-key).
    std::fs::remove_file(f.project.join(".aida/config.toml")).unwrap();
    store_commit_as(&f, FORBIDDEN);
    let out = push_store(&f, &f.path_with_aida);
    assert!(out.status.success(), "{}", text(&out));
    assert!(remote_has_store_branch(&f));

    let out = git_cmd(&f, &f.project, &f.path_with_aida)
        .env("GIT_AUTHOR_EMAIL", FORBIDDEN)
        .env("GIT_COMMITTER_EMAIL", FORBIDDEN)
        .args(["commit", "-q", "--allow-empty", "-m", "code write"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let out = git_cmd(&f, &f.project, &f.path_with_aida)
        .args(["push", "origin", "main"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
}
