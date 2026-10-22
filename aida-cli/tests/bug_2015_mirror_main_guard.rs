#![cfg(unix)]
//! BUG-2015: a repo whose pre-push hook refuses every push to the default
//! branch must still let `aida remote mirror-sync` bring a registered mirror
//! level with origin's reviewed tip, while every other default-branch push
//! (a local-only commit to the mirror, a direct push to origin) is refused.
//!
//! The guard is AIDA's shipped, opt-in main guard
//! (`aida remote mirror-guard-check --install-guard`), chained in as the real
//! pre-push hook and calling the freshly built `aida`. Hermetic: isolated
//! HOME, temp bare hubs, no network.
// trace:BUG-2015 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    origin: PathBuf,
    mirror: PathBuf,
    path_with_aida: String,
}

fn text(out: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn with_env(f: &Fixture, mut c: Command) -> Command {
    c.env("HOME", &f.home)
        .env("PATH", &f.path_with_aida)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_OUTPUT_FORMAT", "human")
        .env_remove("AIDA_MIRROR_SYNC_ATTEST")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE");
    c
}

fn git(f: &Fixture, dir: &Path, args: &[&str]) -> Output {
    let mut c = with_env(f, test_git::git_command());
    c.arg("-C").arg(dir).args(args).output().unwrap()
}

fn git_ok(f: &Fixture, dir: &Path, args: &[&str]) -> String {
    let out = git(f, dir, args);
    assert!(out.status.success(), "git {args:?} failed\n{}", text(&out));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn aida(f: &Fixture, args: &[&str]) -> Output {
    let mut c = with_env(f, Command::new(env!("CARGO_BIN_EXE_aida")));
    c.current_dir(&f.project).args(args).output().unwrap()
}

fn tip(f: &Fixture, bare: &Path, branch: &str) -> Option<String> {
    let out = git(
        f,
        bare,
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
    );
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `git push` from the project with an optional (forged) attestation.
fn push(f: &Fixture, attest: Option<String>, args: &[&str]) -> Output {
    let mut c = with_env(f, test_git::git_command());
    if let Some(a) = attest {
        c.env("AIDA_MIRROR_SYNC_ATTEST", a);
    }
    c.arg("-C")
        .arg(&f.project)
        .arg("push")
        .args(args)
        .output()
        .unwrap()
}

fn commit(f: &Fixture, dir: &Path, name: &str) -> String {
    std::fs::write(dir.join(name), format!("{name}\n")).unwrap();
    git_ok(f, dir, &["add", name]);
    git_ok(f, dir, &["commit", "-q", "-m", name]);
    git_ok(f, dir, &["rev-parse", "HEAD"])
}

/// Project on `branch` with `origin` and `mirror` bare hubs, `branch` on
/// origin, and `mirror` registered in `[store.sync] mirror_remotes`. The
/// mirror hub never gets a HEAD symref pointing at `branch`. Origin's HEAD
/// symref is set when `origin_head`. No guard yet.
fn setup(branch: &str, origin_head: bool) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_aida"), bin.join("aida")).unwrap();
    let orig = std::env::var_os("PATH").unwrap_or_default();
    let path_with_aida = std::env::join_paths(
        std::iter::once(bin.clone())
            .chain(std::env::split_paths(&orig).filter(|d| !d.join("aida").exists())),
    )
    .unwrap()
    .to_string_lossy()
    .into_owned();
    let f = Fixture {
        project: tmp.path().join("project"),
        origin: tmp.path().join("origin.git"),
        mirror: tmp.path().join("mirror.git"),
        home,
        path_with_aida,
        _tmp: tmp,
    };
    let root = f.project.parent().unwrap().to_path_buf();
    git_ok(&f, &root, &["init", "-q", "--bare", "origin.git"]);
    git_ok(&f, &root, &["init", "-q", "--bare", "mirror.git"]);
    git_ok(&f, &root, &["init", "-q", "-b", branch, "project"]);
    commit(&f, &f.project, "a.txt");
    git_ok(
        &f,
        &f.project,
        &["remote", "add", "origin", f.origin.to_str().unwrap()],
    );
    git_ok(
        &f,
        &f.project,
        &["remote", "add", "mirror", f.mirror.to_str().unwrap()],
    );
    // Seed origin before the guard exists, the way a reviewed merge lands.
    git_ok(&f, &f.project, &["push", "-q", "origin", branch]);
    if origin_head {
        git_ok(&f, &f.project, &["remote", "set-head", "origin", branch]);
    }
    std::fs::create_dir_all(f.project.join(".aida")).unwrap();
    std::fs::write(
        f.project.join(".aida/config.toml"),
        "[store.sync]\nmirror_remotes = [\"mirror\"]\n",
    )
    .unwrap();
    f
}

fn install_guard(f: &Fixture, extra: &[&str]) -> Output {
    let mut args = vec!["remote", "mirror-guard-check", "--install-guard"];
    args.extend(extra);
    aida(f, &args)
}

/// [`setup`] on `main` with origin's HEAD set and the guard installed.
fn fixture() -> Fixture {
    let f = setup("main", true);
    let out = install_guard(&f, &[]);
    assert!(out.status.success(), "{}", text(&out));
    f
}

// trace:BUG-2015.ac6fb370 | ai:claude
#[test]
fn bug_2015_guarded_mirror_sync_mirrors_origin_tip_and_refuses_local_only_main() {
    let f = fixture();
    let origin_tip = tip(&f, &f.origin, "main").unwrap();

    // mirror-sync, no override, through the refusing guard.
    let sync = aida(&f, &["remote", "mirror-sync"]);
    assert!(sync.status.success(), "{}", text(&sync));
    assert_eq!(tip(&f, &f.mirror, "main"), Some(origin_tip.clone()));

    // A local-only main commit is still refused to the mirror and to origin,
    // even with a hand-forged attestation for it.
    let local = commit(&f, &f.project, "b.txt");
    for remote in ["mirror", "origin"] {
        let out = push(
            &f,
            Some(format!("{remote} {local} refs/heads/main")),
            &[remote, "main"],
        );
        assert!(!out.status.success(), "{remote}: {}", text(&out));
        assert!(text(&out).contains("REFUSING"), "{remote}: {}", text(&out));
    }
    assert_eq!(tip(&f, &f.mirror, "main"), Some(origin_tip.clone()));
    assert_eq!(tip(&f, &f.origin, "main"), Some(origin_tip));
}

// trace:BUG-2015.ac6fb370 | ai:claude
#[test]
fn bug_2015_guard_refuses_a_stale_origin_tip_even_when_attested() {
    let f = fixture();
    let old = tip(&f, &f.origin, "main").unwrap();
    // Origin moves on (another clone merges); this clone's tracking ref
    // still names `old`.
    let root = f.project.parent().unwrap().to_path_buf();
    let origin = f.origin.to_str().unwrap().to_string();
    git_ok(
        &f,
        &root,
        &["clone", "-q", "--branch", "main", &origin, "other"],
    );
    let other = root.join("other");
    commit(&f, &other, "c.txt");
    git_ok(&f, &other, &["push", "-q", "origin", "main"]);
    assert_eq!(
        git_ok(&f, &f.project, &["rev-parse", "refs/remotes/origin/main"]),
        old
    );

    let out = push(
        &f,
        Some(format!("mirror {old} refs/heads/main")),
        &["mirror", &format!("{old}:refs/heads/main")],
    );
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("not origin/main's tip"),
        "{}",
        text(&out)
    );
    assert_eq!(tip(&f, &f.mirror, "main"), None);
}

// The protected branch is origin's default, not the pushed remote's: a repo
// whose default is `trunk`, pushing to a mirror with no HEAD symref, guards
// `trunk` (and lets an unrelated `main` through).
// trace:BUG-2015.ac6fb370 | ai:claude
#[test]
fn bug_2015_guard_protects_origins_non_main_default_on_a_headless_mirror() {
    let f = setup("trunk", true);
    let out = install_guard(&f, &[]);
    assert!(out.status.success(), "{}", text(&out));
    let origin_tip = tip(&f, &f.origin, "trunk").unwrap();
    assert!(git(&f, &f.mirror, &["symbolic-ref", "HEAD"])
        .status
        .success());
    assert_ne!(
        git_ok(&f, &f.mirror, &["symbolic-ref", "HEAD"]),
        "refs/heads/trunk",
        "the mirror must not advertise trunk as its HEAD"
    );

    let sync = aida(&f, &["remote", "mirror-sync"]);
    assert!(sync.status.success(), "{}", text(&sync));
    assert_eq!(tip(&f, &f.mirror, "trunk"), Some(origin_tip.clone()));

    let local = commit(&f, &f.project, "b.txt");
    let out = push(
        &f,
        Some(format!("mirror {local} refs/heads/trunk")),
        &["mirror", "trunk"],
    );
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("REFUSING to push to trunk"),
        "{}",
        text(&out)
    );
    assert_eq!(tip(&f, &f.mirror, "trunk"), Some(origin_tip));

    let out = push(&f, None, &["mirror", "trunk:refs/heads/main"]);
    assert!(out.status.success(), "{}", text(&out));
}

// Without origin's HEAD symref the guard cannot tell what to protect, so it
// refuses with the one-line fix instead of guessing.
// trace:BUG-2015.ac6fb370 | ai:claude
#[test]
fn bug_2015_guard_fails_closed_without_origin_head() {
    let f = setup("trunk", false);
    let out = install_guard(&f, &[]);
    assert!(out.status.success(), "{}", text(&out));
    commit(&f, &f.project, "b.txt");
    let out = push(&f, None, &["mirror", "trunk"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("cannot tell origin's default branch"),
        "{}",
        text(&out)
    );
    assert!(
        text(&out).contains("git remote set-head origin --auto"),
        "{}",
        text(&out)
    );
    assert_eq!(tip(&f, &f.mirror, "trunk"), None);
}

// Installing the guard chains it in front of the mirror fan-out hook: the
// fan-out still runs and still sees the ref lines, a refresh by
// `aida remote mirror` keeps the guard, and a foreign hook is refused
// without --force.
// trace:BUG-2015.ac6fb370 | ai:claude
#[test]
fn bug_2015_install_guard_keeps_the_mirror_fan_out_hook_running() {
    let f = setup("main", true);
    let out = aida(&f, &["remote", "mirror", "mirror"]);
    assert!(out.status.success(), "{}", text(&out));
    let out = install_guard(&f, &[]);
    assert!(out.status.success(), "{}", text(&out));
    let hook = f.project.join(".git/hooks/pre-push");
    let body = std::fs::read_to_string(&hook).unwrap();
    assert!(body.contains("aida main guard"), "{body}");
    assert!(body.contains("aida remote mirror-push"), "{body}");

    // A feature push reaches origin and the fan-out ran with its ref line.
    git_ok(&f, &f.project, &["switch", "-q", "-c", "feature"]);
    let sha = commit(&f, &f.project, "f.txt");
    let out = push(&f, None, &["origin", "feature"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains(&format!("mirror-push: refs/heads/feature@{sha}")),
        "{}",
        text(&out)
    );
    assert_eq!(tip(&f, &f.origin, "feature"), Some(sha));
    // The guard still refuses main.
    let out = push(&f, None, &["origin", "feature:refs/heads/main"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("REFUSING"), "{}", text(&out));

    // `aida remote mirror` refreshing its hook keeps the chain.
    let out = aida(&f, &["remote", "mirror", "mirror"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(std::fs::read_to_string(&hook).unwrap(), body);

    // A foreign hook needs --force, and keeps its own body after chaining.
    std::fs::write(&hook, "#!/bin/sh\necho custom-hook-ran >&2\nexit 0\n").unwrap();
    let out = install_guard(&f, &[]);
    assert!(!out.status.success(), "{}", text(&out));
    let out = install_guard(&f, &["--force"]);
    assert!(out.status.success(), "{}", text(&out));
    commit(&f, &f.project, "g.txt");
    let out = push(&f, None, &["origin", "feature"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("custom-hook-ran"), "{}", text(&out));
    let out = push(&f, None, &["origin", "feature:refs/heads/main"]);
    assert!(text(&out).contains("REFUSING"), "{}", text(&out));
}
