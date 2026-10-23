//! Self-heal for the `config.lock` placeholder a sandboxed shell leaves behind.
//!
//! Claude Code's bubblewrap sandbox protects `<git-common-dir>/config.lock` by
//! bind-mounting `/dev/null` over it. When the path is missing on the host,
//! bwrap first creates an empty, mode-0444 file there as the mount point and
//! never removes it. Host git then refuses every config write ("could not lock
//! config file .git/config: File exists"): `worktree add -b`, `branch -D`,
//! `push -u`, `git config`. A real git lock is created writable (0666 & umask)
//! and holds the config being written, so an EMPTY lock with mode EXACTLY 0444
//! is the placeholder and is always safe to remove. Anything else is left
//! alone, and git's own error surfaces.
// trace:BUG-2211 | ai:claude

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// `<git-common-dir>/config.lock` for the repository at `repo`, or `None`
/// when `repo` is not inside a git repository.
// trace:BUG-2211 | ai:claude
pub fn config_lock_path(repo: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let common = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!common.is_empty()).then(|| PathBuf::from(common).join("config.lock"))
}

/// Whether `lock` is the sandbox placeholder: a regular file (not a symlink),
/// zero bytes, permission bits exactly 0444.
// trace:BUG-2211 | ai:claude
pub fn is_sandbox_placeholder(lock: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(lock) else {
        return false;
    };
    if !meta.file_type().is_file() || meta.len() != 0 {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o7777 == 0o444
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// The placeholder lock in `repo`'s git common dir, when one is present.
// trace:BUG-2211 | ai:claude
pub fn sandbox_config_lock(repo: &Path) -> Option<PathBuf> {
    config_lock_path(repo).filter(|lock| is_sandbox_placeholder(lock))
}

/// Remove the placeholder lock in `repo`'s git common dir, if present.
/// Returns the removed path. Never touches a lock that fails the predicate.
// trace:BUG-2211 | ai:claude
pub fn clear_sandbox_config_lock(repo: &Path) -> Option<PathBuf> {
    let lock = sandbox_config_lock(repo)?;
    std::fs::remove_file(&lock).ok()?;
    Some(lock)
}

/// Whether git's stderr reports that it could not take the config lock.
pub(crate) fn is_config_lock_refusal(stderr: &[u8]) -> bool {
    let stderr = String::from_utf8_lossy(stderr);
    stderr.contains("could not lock config file") || stderr.contains("config.lock")
}

/// The arguments for the one retry after a config-lock refusal.
///
/// `worktree add -b <branch>` creates the branch before it writes the
/// tracking config, so the refused attempt leaves the branch behind and a
/// verbatim retry fails with "a branch named ... already exists". git checks
/// that `-b` names a new branch before doing anything, so after a lock
/// refusal the branch is the one git just made at the same start point, and
/// `-B` re-creates it there. Every other command retries unchanged.
// trace:BUG-2211 | ai:claude
pub(crate) fn retry_args(args: &[OsString]) -> Vec<OsString> {
    let mut retry = args.to_vec();
    if retry.len() >= 2 && retry[0] == "worktree" && retry[1] == "add" {
        if let Some(flag) = retry
            .iter_mut()
            .take_while(|a| a.as_os_str() != "--")
            .find(|a| a.as_os_str() == "-b")
        {
            *flag = OsString::from("-B");
        }
    }
    retry
}

/// Run a git command that takes the config lock of the repository at `repo`.
///
/// Clears the sandbox placeholder first; if git still reports the lock (a
/// sandboxed command elsewhere recreated it in between), clears it once more
/// and retries once with [`retry_args`]. `base` makes the `git` command for
/// each attempt (program, working directory, any `-C`/`-c` prefix); `args`
/// is the subcommand and its arguments.
// trace:BUG-2211 | ai:claude
pub fn output_healing_config_lock<S: AsRef<OsStr>>(
    repo: &Path,
    mut base: impl FnMut() -> Command,
    args: &[S],
) -> std::io::Result<Output> {
    let args: Vec<OsString> = args.iter().map(|a| a.as_ref().to_os_string()).collect();
    clear_sandbox_config_lock(repo);
    let out = base().args(&args).output()?;
    if out.status.success()
        || !is_config_lock_refusal(&out.stderr)
        || clear_sandbox_config_lock(repo).is_none()
    {
        return Ok(out);
    }
    base().args(retry_args(&args)).output()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::test_git::git_command;
    use std::os::unix::fs::PermissionsExt;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let out = git_command()
                .current_dir(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.invalid"]);
        run(&["config", "user.name", "t"]);
        run(&["commit", "-q", "--allow-empty", "-m", "init"]);
        dir
    }

    fn plant_lock(repo: &Path, body: &[u8], mode: u32) -> PathBuf {
        let lock = repo.join(".git/config.lock");
        std::fs::write(&lock, body).unwrap();
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(mode)).unwrap();
        lock
    }

    /// `worktree add -b` with upstream tracking, so git writes
    /// `branch.<name>.*` and takes the config lock (as it does from an
    /// `origin/main` base).
    fn worktree_add(repo: &Path, branch: &str) -> std::io::Result<Output> {
        let wt = repo.join(format!("wt-{branch}"));
        output_healing_config_lock(
            repo,
            || {
                let mut cmd = git_command();
                cmd.current_dir(repo);
                cmd
            },
            &[
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("-q"),
                OsStr::new("--track"),
                OsStr::new("-b"),
                OsStr::new(branch),
                wt.as_os_str(),
                OsStr::new("main"),
            ],
        )
    }

    #[test]
    fn placeholder_is_removed_and_worktree_add_succeeds() {
        let dir = repo();
        let lock = plant_lock(dir.path(), b"", 0o444);
        assert_eq!(sandbox_config_lock(dir.path()), Some(lock.clone()));
        let out = worktree_add(dir.path(), "feature").unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(!lock.exists());
    }

    #[test]
    fn placeholder_is_removed_and_branch_delete_succeeds() {
        let dir = repo();
        let made = git_command()
            .current_dir(dir.path())
            .args(["branch", "doomed"])
            .output()
            .unwrap();
        assert!(made.status.success());
        let lock = plant_lock(dir.path(), b"", 0o444);
        let out = output_healing_config_lock(
            dir.path(),
            || {
                let mut cmd = git_command();
                cmd.current_dir(dir.path());
                cmd
            },
            &["branch", "-D", "doomed"],
        )
        .unwrap();
        assert!(out.status.success(), "{out:?}");
        // Under the lock git deletes the ref but warns that the config
        // update failed; with the placeholder cleared first it does not.
        assert!(!is_config_lock_refusal(&out.stderr), "{out:?}");
        assert!(!lock.exists());
    }

    #[test]
    fn placeholder_recreated_after_the_pre_clear_is_cleared_and_retried_once() {
        let dir = repo();
        let wt = dir.path().join("wt-raced");
        let mut attempts = 0;
        let out = output_healing_config_lock(
            dir.path(),
            || {
                attempts += 1;
                if attempts == 1 {
                    // Another sandboxed command recreates it after the pre-clear.
                    plant_lock(dir.path(), b"", 0o444);
                }
                let mut cmd = git_command();
                cmd.current_dir(dir.path());
                cmd
            },
            &[
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("-q"),
                OsStr::new("--track"),
                OsStr::new("-b"),
                OsStr::new("raced"),
                wt.as_os_str(),
                OsStr::new("main"),
            ],
        )
        .unwrap();
        assert!(out.status.success(), "{out:?}");
        assert_eq!(attempts, 2);
        assert!(!dir.path().join(".git/config.lock").exists());
        assert!(wt.join(".git").exists());
        let upstream = git_command()
            .current_dir(&wt)
            .args(["rev-parse", "--abbrev-ref", "@{upstream}"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&upstream.stdout).trim(), "main");
    }

    #[test]
    fn retry_rewrites_only_a_worktree_add_branch_flag() {
        let os = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            retry_args(&os(&["worktree", "add", "-b", "x", "--", "p", "main"])),
            os(&["worktree", "add", "-B", "x", "--", "p", "main"])
        );
        // A `-b` after `--` is a path, not the flag.
        assert_eq!(
            retry_args(&os(&["worktree", "add", "--", "-b", "x"])),
            os(&["worktree", "add", "--", "-b", "x"])
        );
        assert_eq!(
            retry_args(&os(&["branch", "-D", "-b"])),
            os(&["branch", "-D", "-b"])
        );
    }

    #[test]
    fn non_empty_lock_is_left_alone_and_git_error_surfaces() {
        let dir = repo();
        let lock = plant_lock(dir.path(), b"[core]\n", 0o444);
        assert_eq!(sandbox_config_lock(dir.path()), None);
        let out = worktree_add(dir.path(), "feature").unwrap();
        assert!(!out.status.success());
        assert!(is_config_lock_refusal(&out.stderr), "{out:?}");
        assert_eq!(std::fs::read(&lock).unwrap(), b"[core]\n");
    }

    #[test]
    fn writable_lock_is_left_alone_and_git_error_surfaces() {
        let dir = repo();
        let lock = plant_lock(dir.path(), b"", 0o644);
        assert_eq!(clear_sandbox_config_lock(dir.path()), None);
        let out = worktree_add(dir.path(), "feature").unwrap();
        assert!(!out.status.success());
        assert!(is_config_lock_refusal(&out.stderr), "{out:?}");
        assert!(lock.exists());
    }

    #[test]
    fn linked_worktree_resolves_the_shared_lock() {
        let dir = repo();
        let out = worktree_add(dir.path(), "linked").unwrap();
        assert!(out.status.success(), "{out:?}");
        let lock = plant_lock(dir.path(), b"", 0o444);
        let linked = dir.path().join("wt-linked");
        assert_eq!(clear_sandbox_config_lock(&linked), Some(lock.clone()));
        assert!(!lock.exists());
    }
}
