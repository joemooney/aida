#![cfg(unix)]
//! BUG-2015: a repo whose pre-push hook refuses every push to the default
//! branch must still let `aida remote mirror-sync` bring a registered mirror
//! level with origin's reviewed tip, while every other default-branch push
//! (a local-only commit to the mirror, a direct push to origin) is refused.
//!
//! The guard is AIDA's shipped, opt-in main guard
//! (`aida remote mirror-guard-check --print-guard`), installed as the real
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

fn tip(f: &Fixture, bare: &Path) -> Option<String> {
    let out = git(f, bare, &["rev-parse", "--verify", "refs/heads/main"]);
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Project with `origin` and `mirror` bare hubs, `main` on origin,
/// `mirror` registered in `[store.sync] mirror_remotes`, and AIDA's main
/// guard installed as the pre-push hook.
fn fixture() -> Fixture {
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
    git_ok(&f, &root, &["init", "-q", "-b", "main", "project"]);
    std::fs::write(f.project.join("a.txt"), "a\n").unwrap();
    git_ok(&f, &f.project, &["add", "a.txt"]);
    git_ok(&f, &f.project, &["commit", "-q", "-m", "init"]);
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
    git_ok(&f, &f.project, &["push", "-q", "origin", "main"]);
    std::fs::create_dir_all(f.project.join(".aida")).unwrap();
    std::fs::write(
        f.project.join(".aida/config.toml"),
        "[store.sync]\nmirror_remotes = [\"mirror\"]\n",
    )
    .unwrap();

    let printed = aida(&f, &["remote", "mirror-guard-check", "--print-guard"]);
    assert!(printed.status.success(), "{}", text(&printed));
    let hook = f.project.join(".git/hooks/pre-push");
    std::fs::write(&hook, &printed.stdout).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    f
}

// trace:BUG-2015.ac6fb370 | ai:claude
#[test]
fn bug_2015_guarded_mirror_sync_mirrors_origin_tip_and_refuses_local_only_main() {
    let f = fixture();
    let origin_tip = tip(&f, &f.origin).unwrap();

    // mirror-sync, no override, through the refusing guard.
    let sync = aida(&f, &["remote", "mirror-sync"]);
    assert!(sync.status.success(), "{}", text(&sync));
    assert_eq!(tip(&f, &f.mirror), Some(origin_tip.clone()));

    // A local-only main commit is still refused to the mirror and to origin,
    // even with a hand-forged attestation for it.
    std::fs::write(f.project.join("b.txt"), "b\n").unwrap();
    git_ok(&f, &f.project, &["add", "b.txt"]);
    git_ok(&f, &f.project, &["commit", "-q", "-m", "local only"]);
    let local = git_ok(&f, &f.project, &["rev-parse", "HEAD"]);
    for remote in ["mirror", "origin"] {
        let mut c = with_env(&f, test_git::git_command());
        let out = c
            .env(
                "AIDA_MIRROR_SYNC_ATTEST",
                format!("{remote} {local} refs/heads/main"),
            )
            .arg("-C")
            .arg(&f.project)
            .args(["push", remote, "main"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{remote}: {}", text(&out));
        assert!(text(&out).contains("REFUSING"), "{remote}: {}", text(&out));
    }
    assert_eq!(tip(&f, &f.mirror), Some(origin_tip.clone()));
    assert_eq!(tip(&f, &f.origin), Some(origin_tip));
}

// trace:BUG-2015.ac6fb370 | ai:claude
#[test]
fn bug_2015_guard_refuses_a_stale_origin_tip_even_when_attested() {
    let f = fixture();
    let old = tip(&f, &f.origin).unwrap();
    // Origin moves on (another clone merges); this clone's tracking ref
    // still names `old`.
    let root = f.project.parent().unwrap().to_path_buf();
    git_ok(
        &f,
        &root,
        &[
            "clone",
            "-q",
            "--branch",
            "main",
            f.origin.to_str().unwrap(),
            "other",
        ],
    );
    let other = root.join("other");
    std::fs::write(other.join("c.txt"), "c\n").unwrap();
    git_ok(&f, &other, &["add", "c.txt"]);
    git_ok(&f, &other, &["commit", "-q", "-m", "merged elsewhere"]);
    git_ok(&f, &other, &["push", "-q", "origin", "main"]);
    assert_eq!(
        git_ok(&f, &f.project, &["rev-parse", "refs/remotes/origin/main"]),
        old
    );

    let mut c = with_env(&f, test_git::git_command());
    let out = c
        .env(
            "AIDA_MIRROR_SYNC_ATTEST",
            format!("mirror {old} refs/heads/main"),
        )
        .arg("-C")
        .arg(&f.project)
        .args(["push", "mirror", &format!("{old}:refs/heads/main")])
        .output()
        .unwrap();
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("not origin/main's tip"),
        "{}",
        text(&out)
    );
    assert_eq!(tip(&f, &f.mirror), None);
}
