#![cfg(target_os = "linux")]
//! BUG-1929 E1: a contained session can write the main checkout's store, so it
//! can plant an embedded repository there (`objects/zz/.git` naming a git
//! directory it made, whose config sets `core.fsmonitor=<cmd>`). Before the
//! fix, the host's next store sync ran `git status` in the store, git
//! recursed into the planted repository, and `<cmd>` ran on the host, outside
//! the sandbox. This drives the real `aida db sync --push` over such a store
//! and asserts the command never runs the planted hook, refuses, and
//! publishes nothing. Hermetic: fake HOME, git config isolated from the user,
//! a local bare origin, no network.
// trace:BUG-1929 | ai:claude

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Fx {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    store: PathBuf,
    origin: PathBuf,
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

fn text(out: &Output) -> String {
    format!(
        "status={:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

impl Fx {
    fn run(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(bin());
        let path = format!(
            "{}:{}",
            bin().parent().unwrap().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        cmd.env_clear();
        git_env(&mut cmd);
        cmd.current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("TMPDIR", std::env::temp_dir())
            .env("LANG", "C.UTF-8")
            .env("PATH", path)
            .env("AIDA_TELEMETRY", "0")
            .env("AIDA_USER", "t1929")
            .env("AIDA_OUTPUT_FORMAT", "human")
            .stdin(Stdio::null())
            .args(args);
        if let Some(grant) = &self.grant {
            cmd.env("AIDA_SESSION_GRANT", grant);
        }
        cmd.output().expect("run aida")
    }

    fn run_ok(&self, args: &[&str]) {
        let out = self.run(args);
        assert!(out.status.success(), "aida {args:?} failed: {}", text(&out));
    }

    fn origin_head(&self) -> String {
        git(&self.origin, &["rev-parse", "refs/heads/aida-store"])
    }

    /// Plant what a sandboxed session can write: a hook script and an
    /// embedded repository inside the store, the repository's own config
    /// naming the script as its `core.fsmonitor`. The script appends to the
    /// returned marker, which lives outside the store. `stage` also stages
    /// the gitlink (the store's index is writable from the sandbox too).
    fn plant(&self, stage: bool) -> PathBuf {
        let marker = self.base.join("fsmonitor-ran");
        let hook = self.store.join(".evil-fsm.sh");
        std::fs::write(
            &hook,
            format!("#!/bin/sh\necho ran >> '{}'\nexit 1\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let sub = self.store.join("objects").join("zz");
        std::fs::create_dir_all(&sub).unwrap();
        let gitdir = self.store.join(".evil.git");
        git(
            &self.store,
            &[
                "init",
                "-q",
                &format!("--separate-git-dir={}", gitdir.display()),
                sub.to_str().unwrap(),
            ],
        );
        std::fs::write(sub.join("x.txt"), "x").unwrap();
        git(&sub, &["add", "x.txt"]);
        git(&sub, &["commit", "-qm", "planted"]);
        git(&sub, &["config", "core.fsmonitor", hook.to_str().unwrap()]);
        if stage {
            git(&self.store, &["add", "objects/zz"]);
        }
        marker
    }
}

fn fixture() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("home");
    let repo = base.join("repo");
    let origin = base.join("origin.git");
    for d in [&home, &repo] {
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
        grant: None,
    };
    fx.run_ok(&[
        "init",
        "--force",
        "--no-skills",
        "--no-agent-config",
        "--no-roles",
        "--no-hooks",
    ]);
    fx.run_ok(&["add", "seed spec", "--type", "task"]);
    git(
        &fx.repo,
        &["remote", "add", "origin", fx.origin.to_str().unwrap()],
    );
    git(&fx.repo, &["push", "-q", "origin", "main"]);
    let _ = git_try(&fx.store, &["add", "-A"]);
    let _ = git_try(&fx.store, &["commit", "-qm", "chore: fixture settle"]);
    git(&fx.store, &["push", "-q", "origin", "aida-store"]);
    fx.grant = support::ensure_seat_for(&fx.home, &fx.repo, "t1929", "advisor", &[]);
    let _ = git_try(&fx.store, &["add", "-A"]);
    let _ = git_try(&fx.store, &["commit", "-qm", "chore: fixture roster"]);
    git(&fx.store, &["push", "-q", "origin", "aida-store"]);
    assert_eq!(git(&fx.store, &["status", "--porcelain"]), "");
    fx
}

/// A staged planted gitlink: a plain `git status` in the store runs the hook
/// (the plant is live), but `aida db sync --push` never does, refuses, and
/// neither commits nor publishes.
#[test]
fn store_sync_never_runs_a_planted_fsmonitor_and_refuses() {
    let fx = fixture();
    let marker = fx.plant(true);
    let before = (git(&fx.store, &["rev-parse", "HEAD"]), fx.origin_head());

    let _ = git_try(&fx.store, &["status", "--porcelain"]);
    assert!(
        marker.exists(),
        "precondition: an unhardened git status must run the planted fsmonitor"
    );
    std::fs::remove_file(&marker).unwrap();

    std::fs::write(fx.store.join("objects").join("pending.yaml"), "id: P\n").unwrap();
    let out = fx.run(&["db", "sync", "--push"]);
    assert!(
        !marker.exists(),
        "aida db sync --push ran the planted fsmonitor on the host: {}",
        text(&out)
    );
    assert!(!out.status.success(), "sync must refuse: {}", text(&out));
    let all = text(&out);
    assert!(all.contains("embedded git repository"), "{all}");
    assert!(all.contains("objects/zz"), "{all}");
    assert_eq!(
        (git(&fx.store, &["rev-parse", "HEAD"]), fx.origin_head()),
        before,
        "nothing may be committed or pushed"
    );
}

/// An unstaged plant: the host's own `git add -A` used to stage it as a
/// gitlink. The sync refuses before staging anything.
#[test]
fn store_sync_refuses_an_unstaged_nested_git() {
    let fx = fixture();
    let marker = fx.plant(false);
    let before = fx.origin_head();
    let out = fx.run(&["db", "sync", "--push"]);
    assert!(!out.status.success(), "sync must refuse: {}", text(&out));
    assert!(text(&out).contains("objects/zz/.git"), "{}", text(&out));
    assert!(!git(&fx.store, &["ls-files", "--stage"]).contains("160000 "));
    assert!(!marker.exists());
    assert_eq!(fx.origin_head(), before);
}
