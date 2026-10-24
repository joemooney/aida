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
    if !is_config_lock_refusal(&out.stderr) || clear_sandbox_config_lock(repo).is_none() {
        return Ok(out);
    }
    if out.status.success() {
        // `branch -d/-D` deletes the ref and exits 0 even when it could not
        // drop `branch.<name>.*`; finish that half instead of retrying.
        // trace:TASK-2270 | ai:claude
        for name in deleted_branches(&args).unwrap_or_default() {
            let mut section = OsString::from("branch.");
            section.push(&name);
            let _ = base()
                .args(["config", "--remove-section"])
                .arg(section)
                .output();
        }
        return Ok(out);
    }
    base().args(retry_args(&args)).output()
}

/// The local branches a `git branch -d/-D/--delete` invocation names, or
/// `None` for any other command (and for `-r`, whose remote-tracking refs
/// carry no `branch.<name>` config).
// trace:TASK-2270 | ai:claude
pub(crate) fn deleted_branches(args: &[OsString]) -> Option<Vec<OsString>> {
    let (first, rest) = args.split_first()?;
    if first != "branch" {
        return None;
    }
    let mut deleting = false;
    let mut names = Vec::new();
    let mut options_ended = false;
    for arg in rest {
        let text = arg.to_string_lossy();
        if options_ended || !text.starts_with('-') {
            names.push(arg.clone());
        } else if text == "--" {
            options_ended = true;
        } else if matches!(text.as_ref(), "-d" | "-D" | "--delete") {
            deleting = true;
        } else if matches!(text.as_ref(), "-r" | "--remotes" | "-a" | "--all") {
            return None;
        }
    }
    (deleting && !names.is_empty()).then_some(names)
}

/// After a successful `git push -u <remote> <src>:<remote_branch>` (or
/// `<branch>`), set the upstream the push could not record because the
/// sandbox placeholder held the config lock: git pushes, warns "unable to
/// write upstream branch configuration", and exits 0. `local` is the branch
/// that should track, or `None` for the checked-out branch (a `HEAD:` push).
/// Repairs whenever the upstream is not `<remote>/<remote_branch>`, including
/// a branch that already tracked `origin/main` from `worktree add -b` (the
/// failed write leaves that stale upstream in place). Does nothing when the
/// upstream already matches or the remote-tracking ref is missing. Returns
/// whether it set the upstream.
// trace:TASK-2270 | ai:claude
pub fn repair_push_upstream(
    repo: &Path,
    remote: &str,
    local: Option<&str>,
    remote_branch: &str,
) -> bool {
    let git = || {
        let mut cmd = Command::new("git");
        cmd.current_dir(repo);
        cmd
    };
    let upstream_of = format!("{}@{{upstream}}", local.unwrap_or(""));
    let wanted = format!("{remote}/{remote_branch}");
    // trace:TASK-2270 | ai:claude
    let upstream_matches = git()
        .args([
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            &upstream_of,
        ])
        .output()
        .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == wanted);
    let tracking = format!("refs/remotes/{wanted}");
    let has_tracking = git()
        .args(["rev-parse", "--verify", "--quiet", &tracking])
        .output()
        .is_ok_and(|o| o.status.success());
    if upstream_matches || !has_tracking {
        return false;
    }
    let mut args = vec![format!("--set-upstream-to={wanted}")];
    args.extend(local.map(str::to_string));
    args.insert(0, "branch".to_string());
    output_healing_config_lock(repo, git, &args).is_ok_and(|o| o.status.success())
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

    fn git_ok(dir: &Path, args: &[&str]) -> String {
        let out = crate::test_git::git_command()
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn has_branch_section(dir: &Path, name: &str) -> bool {
        git_command()
            .current_dir(dir)
            .args(["config", "--get-regexp", &format!("^branch\\.{name}\\.")])
            .output()
            .unwrap()
            .status
            .success()
    }

    #[test]
    fn branch_delete_under_a_recreated_placeholder_drops_the_config_section() {
        let dir = repo();
        git_ok(dir.path(), &["branch", "--track", "tracked", "main"]);
        assert!(has_branch_section(dir.path(), "tracked"));
        let mut attempts = 0;
        let out = output_healing_config_lock(
            dir.path(),
            || {
                attempts += 1;
                if attempts == 1 {
                    plant_lock(dir.path(), b"", 0o444);
                }
                let mut cmd = git_command();
                cmd.current_dir(dir.path());
                cmd
            },
            &["branch", "-D", "--", "tracked"],
        )
        .unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(is_config_lock_refusal(&out.stderr), "{out:?}");
        // The ref went on the first run; one follow-up drops the section.
        assert_eq!(attempts, 2);
        assert!(!has_branch_section(dir.path(), "tracked"));
        assert!(!dir.path().join(".git/config.lock").exists());
    }

    #[test]
    fn branch_delete_under_a_real_lock_is_not_followed_up() {
        let dir = repo();
        git_ok(dir.path(), &["branch", "--track", "tracked", "main"]);
        let mut attempts = 0;
        let out = output_healing_config_lock(
            dir.path(),
            || {
                attempts += 1;
                if attempts == 1 {
                    plant_lock(dir.path(), b"", 0o644);
                }
                let mut cmd = git_command();
                cmd.current_dir(dir.path());
                cmd
            },
            &["branch", "-D", "tracked"],
        )
        .unwrap();
        assert!(is_config_lock_refusal(&out.stderr), "{out:?}");
        assert_eq!(attempts, 1);
        assert!(dir.path().join(".git/config.lock").exists());
    }

    #[test]
    fn deleted_branches_reads_only_local_deletes() {
        let os = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            deleted_branches(&os(&["branch", "-D", "--", "a", "-b"])),
            Some(os(&["a", "-b"]))
        );
        assert_eq!(
            deleted_branches(&os(&["branch", "--delete", "-f", "a"])),
            Some(os(&["a"]))
        );
        assert_eq!(
            deleted_branches(&os(&["branch", "-r", "-d", "origin/a"])),
            None
        );
        assert_eq!(deleted_branches(&os(&["branch", "a"])), None);
        assert_eq!(deleted_branches(&os(&["worktree", "add", "-d", "a"])), None);
    }

    /// A clone of a bare remote, on branch `feat`, with the placeholder
    /// planted: `push -u` pushes and exits 0 but records no upstream.
    fn pushed_without_upstream(refspec: &str) -> (tempfile::TempDir, PathBuf) {
        pushed_with_failed_upstream_write(refspec, false)
    }

    /// As [`pushed_without_upstream`], but with `tracks_main` the branch is
    /// forked from `origin/main` with tracking first, the way AIDA's
    /// `worktree add -b <branch> -- <path> origin/main` creates it, so the
    /// failed write leaves `origin/main` as the upstream.
    fn pushed_with_failed_upstream_write(
        refspec: &str,
        tracks_main: bool,
    ) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        git_ok(root.path(), &["init", "-q", "--bare", "remote.git"]);
        let work = root.path().join("work");
        std::fs::create_dir(&work).unwrap();
        git_ok(&work, &["init", "-q", "-b", "main"]);
        git_ok(&work, &["config", "user.email", "t@example.invalid"]);
        git_ok(&work, &["config", "user.name", "t"]);
        git_ok(&work, &["commit", "-q", "--allow-empty", "-m", "init"]);
        git_ok(&work, &["remote", "add", "origin", "../remote.git"]);
        if tracks_main {
            git_ok(&work, &["push", "-q", "origin", "main"]);
            git_ok(
                &work,
                &["checkout", "-q", "--track", "-b", "feat", "origin/main"],
            );
            assert_eq!(
                git_ok(&work, &["rev-parse", "--abbrev-ref", "feat@{upstream}"]),
                "origin/main"
            );
        } else {
            git_ok(&work, &["checkout", "-q", "-b", "feat"]);
        }
        plant_lock(&work, b"", 0o444);
        let push = crate::test_git::git_command()
            .current_dir(&work)
            .args(["push", "-q", "-u", "origin", refspec])
            .output()
            .unwrap();
        assert!(push.status.success(), "{push:?}");
        assert!(is_config_lock_refusal(&push.stderr), "{push:?}");
        (root, work)
    }

    #[test]
    fn push_upstream_is_repaired_for_a_named_branch() {
        let (_root, work) = pushed_without_upstream("feat");
        assert!(repair_push_upstream(&work, "origin", Some("feat"), "feat"));
        assert_eq!(
            git_ok(&work, &["rev-parse", "--abbrev-ref", "feat@{upstream}"]),
            "origin/feat"
        );
        assert!(!work.join(".git/config.lock").exists());
        // Idempotent: the upstream already matches.
        assert!(!repair_push_upstream(&work, "origin", Some("feat"), "feat"));
    }

    #[test]
    fn push_upstream_is_repaired_for_a_head_refspec() {
        let (_root, work) = pushed_without_upstream("HEAD:refs/heads/remote-name");
        assert!(repair_push_upstream(&work, "origin", None, "remote-name"));
        assert_eq!(
            git_ok(&work, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
            "origin/remote-name"
        );
    }

    #[test]
    fn push_upstream_replaces_a_stale_origin_main_upstream() {
        let (_root, work) = pushed_with_failed_upstream_write("feat", true);
        // The failed write left the creation-time upstream in place.
        assert_eq!(
            git_ok(&work, &["rev-parse", "--abbrev-ref", "feat@{upstream}"]),
            "origin/main"
        );
        assert!(repair_push_upstream(&work, "origin", Some("feat"), "feat"));
        assert_eq!(
            git_ok(&work, &["rev-parse", "--abbrev-ref", "feat@{upstream}"]),
            "origin/feat"
        );
        assert!(!repair_push_upstream(&work, "origin", Some("feat"), "feat"));
    }

    #[test]
    fn head_push_upstream_replaces_a_stale_origin_main_upstream() {
        let (_root, work) = pushed_with_failed_upstream_write("HEAD:refs/heads/remote-name", true);
        assert!(repair_push_upstream(&work, "origin", None, "remote-name"));
        assert_eq!(
            git_ok(&work, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
            "origin/remote-name"
        );
    }

    #[test]
    fn push_upstream_repair_skips_a_missing_tracking_ref() {
        let (_root, work) = pushed_without_upstream("feat");
        assert!(!repair_push_upstream(
            &work,
            "origin",
            Some("feat"),
            "never-pushed"
        ));
        // The placeholder is untouched when nothing needed the lock.
        assert!(work.join(".git/config.lock").exists());
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
