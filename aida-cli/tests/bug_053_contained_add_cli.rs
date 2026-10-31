#![cfg(target_os = "linux")]
//! Black-box coverage for `aida add` in a contained session (AC2): with the
//! main checkout's `.git` read-only, as a contained seat's sandbox leaves it,
//! `aida add` prints the final ID and the queued-for-the-host notice, and
//! `aida show <id>` resolves it in the same session before any host commit.
// trace:BUG-B6B1278890774EF993D2EBC327862470-053 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = test_git::git_command()
        .current_dir(dir)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn text(out: &Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn aida(root: &Path, repo: &Path, args: &[&str]) -> Output {
    let home = root.join("home");
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(repo)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &home)
        .env("USER", "fixture-human")
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_RUNTIME_DIR", home.join("run"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_OUTPUT_FORMAT", "human")
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .args(args)
        .output()
        .expect("run aida")
}

fn ok(root: &Path, repo: &Path, args: &[&str]) -> String {
    let out = aida(root, repo, args);
    assert!(out.status.success(), "aida {args:?}\n{}", text(&out));
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The contained sandbox's `denyWrite` on `<main>/.git`: every directory
/// under it read-only, restored on drop.
struct DeniedGit(Vec<(PathBuf, std::fs::Permissions)>);

impl DeniedGit {
    fn new(git: &Path) -> Self {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            out.push(dir.to_path_buf());
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    walk(&entry.path(), out);
                }
            }
        }
        let mut dirs = Vec::new();
        walk(git, &mut dirs);
        DeniedGit(
            dirs.into_iter()
                .map(|d| {
                    let original = std::fs::metadata(&d).unwrap().permissions();
                    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o555)).unwrap();
                    (d, original)
                })
                .collect(),
        )
    }
}

impl Drop for DeniedGit {
    fn drop(&mut self) {
        // Children first is unnecessary: restoring a parent's bits never
        // depends on its children's.
        for (d, original) in &self.0 {
            let _ = std::fs::set_permissions(d, original.clone());
        }
    }
}

#[test]
fn contained_add_prints_the_final_id_and_queued_notice_and_show_resolves_it() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipping: root ignores directory write bits");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(root.join("home")).unwrap();
    git_in(&repo, &["init", "-q", "-b", "main"]);
    git_in(
        &repo,
        &["commit", "-q", "--allow-empty", "-m", "chore: base"],
    );
    test_git::enroll_synthetic_home(&repo);
    ok(
        &root,
        &repo,
        &[
            "init",
            "--force",
            "--no-skills",
            "--no-hooks",
            "--no-post-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
    );
    let store = repo.join(".aida-store");
    // The host files one spec first: its counter and store hooks exist.
    let host = ok(
        &root,
        &repo,
        &["add", "--title", "host spec", "--type", "bug"],
    );
    assert!(!host.contains("Queued for the host"), "{host}");
    let head_before = git_in(&store, &["rev-parse", "HEAD"]);

    let (added, id, shown) = {
        let _denied = DeniedGit::new(&repo.join(".git"));
        let added = ok(
            &root,
            &repo,
            &["add", "--title", "contained spec", "--type", "bug"],
        );
        let id = added
            .lines()
            .find_map(|l| l.strip_prefix("Added: "))
            .and_then(|l| l.split(" - ").next())
            .unwrap_or_else(|| panic!("no Added line:\n{added}"))
            .trim()
            .to_string();
        let shown = ok(&root, &repo, &["show", &id, "--no-git"]);
        (added, id, shown)
    };

    assert!(added.contains("contained spec"), "{added}");
    assert!(added.contains(&format!("`aida show {id}`")), "{added}");
    assert!(
        added.contains("Queued for the host to commit")
            && added.contains("The ID is final")
            && added.contains("works here now"),
        "queued notice missing:\n{added}"
    );
    assert!(shown.contains("contained spec"), "show:\n{shown}");
    assert_eq!(
        git_in(&store, &["rev-parse", "HEAD"]),
        head_before,
        "a contained add must not commit"
    );
}
