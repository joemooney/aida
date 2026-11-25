// How a registration-clear attempt for an absent lease path ended.
// trace:BUG-1694 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationClearOutcome {
    /// The stale admin registration was removed.
    Cleared,
    /// The path could not be confirmed unoccupied (or the removal itself
    /// failed), so the registration was kept. Fail closed: a stale
    /// registration is strictly better than orphaning a live worktree.
    Declined,
    /// No admin registration names this path (or the repository state was
    /// unreadable); there was nothing to clear.
    NoRegistration,
}

pub fn inspect_missing_registration(
    project_root: &std::path::Path,
    worktree_path: &std::path::Path,
    pinned_tip: Option<&str>,
    dry_run: bool,
    include_locked: bool,
    before_clear: impl FnOnce(),
    after_final_check: impl FnOnce(),
) -> RegistrationClearOutcome {
    if !worktree_path.is_absolute() {
        return RegistrationClearOutcome::Declined;
    }
    if worktree_path.exists() {
        return RegistrationClearOutcome::Declined;
    }
    before_clear();
    if worktree_path.exists() {
        return RegistrationClearOutcome::Declined;
    }

    // Unreadable repository state cannot establish that no registration names
    // this path — decline loudly rather than fall through to branch deletion.
    // trace:BUG-1694 | ai:claude
    let output = crate::git_ops::host_git_command(project_root)
        .args(["rev-parse", "--git-common-dir"])
        .output();
    let Ok(output) = output else {
        return RegistrationClearOutcome::Declined;
    };
    if !output.status.success() {
        return RegistrationClearOutcome::Declined;
    }
    let common_dir = std::path::PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let common_dir = if common_dir.is_absolute() {
        common_dir
    } else {
        project_root.join(common_dir)
    };
    let worktrees_dir = common_dir.join("worktrees");
    let expected_gitdir = worktree_path.join(".git");
    // A missing worktrees dir means no registrations exist at all (the common
    // already-pruned case); any OTHER read failure is unreadable state and
    // declines like the above. trace:BUG-1694 | ai:claude
    let entries = match std::fs::read_dir(worktrees_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return RegistrationClearOutcome::NoRegistration;
        }
        Err(_) => return RegistrationClearOutcome::Declined,
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return RegistrationClearOutcome::Declined;
        };
        let admin_dir = entry.path();
        let Ok(gitdir) = std::fs::read_to_string(admin_dir.join("gitdir")) else {
            return RegistrationClearOutcome::Declined;
        };
        if worktree_gitdir_matches(gitdir.trim(), &expected_gitdir) {
            if worktree_path.exists() {
                return RegistrationClearOutcome::Declined;
            }
            let absent = |name| matches!(std::fs::symlink_metadata(admin_dir.join(name)), Err(e) if e.kind() == std::io::ErrorKind::NotFound);
            if !include_locked && !absent("locked") {
                return RegistrationClearOutcome::Declined;
            }
            if dry_run {
                return if absent("index.lock")
                    && registered_index_matches_tip(&admin_dir, pinned_tip)
                {
                    RegistrationClearOutcome::Cleared
                } else {
                    RegistrationClearOutcome::Declined
                };
            }
            after_final_check();
            if std::fs::create_dir(worktree_path).is_err() {
                return RegistrationClearOutcome::Declined;
            }
            let lock = admin_dir.join("index.lock");
            let removed = std::fs::File::create_new(&lock).is_ok_and(|_writer_guard| {
                // Rename the whole locked directory first. Recursive removal alone
                // could unlink index.lock while the old registration still exists.
                let retired = registered_index_matches_tip(&admin_dir, pinned_tip)
                    && tempfile::tempdir_in(&common_dir).is_ok_and(|quarantine| {
                        std::fs::rename(&admin_dir, quarantine.path().join("registration")).is_ok()
                    });
                if !retired {
                    let _ = std::fs::remove_file(lock);
                }
                retired
            });
            // Release the claim on both outcomes; non-recursive, so a creator
            // that already populated the empty directory keeps it (ENOTEMPTY).
            let _ = std::fs::remove_dir(worktree_path);
            return if removed {
                RegistrationClearOutcome::Cleared
            } else {
                RegistrationClearOutcome::Declined
            };
        }
    }
    RegistrationClearOutcome::NoRegistration
}

// The linked checkout can disappear while its index still holds staged work.
// Callers without a previously pinned branch tip use the registration's own HEAD.
// trace:BUG-2749 | ai:codex
fn registered_index_matches_tip(admin: &std::path::Path, pinned_tip: Option<&str>) -> bool {
    if !std::fs::symlink_metadata(admin.join("index")).is_ok_and(|m| m.is_file()) {
        return false;
    }
    // Preserve split indexes (including shared snapshots) for manual recovery.
    if !std::fs::read_dir(admin).is_ok_and(|mut entries| {
        entries.all(|entry| {
            entry.is_ok_and(|e| {
                !e.file_name()
                    .as_encoded_bytes()
                    .starts_with(b"sharedindex.")
            })
        })
    }) {
        return false;
    }
    let git = || {
        let mut cmd = crate::git_ops::host_git_command(admin);
        // Keep the builder's explicit safeguards while excluding inherited Git
        // overrides that could redirect the index, refs, or object database.
        let env: Vec<_> = cmd
            .get_envs()
            .filter_map(|(key, value)| value.map(|v| (key.to_owned(), v.to_owned())))
            .collect();
        cmd.env_clear()
            .envs(env)
            .args(["--no-replace-objects", "--git-dir=.", "--work-tree=."]);
        cmd
    };
    let Ok(head) = git()
        .args(["rev-parse", "--verify", "HEAD^{commit}"])
        .output()
    else {
        return false;
    };
    let Ok(tip) = std::str::from_utf8(&head.stdout) else {
        return false;
    };
    let tip = tip.trim();
    let algorithm = match tip.len() {
        40 => &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
        64 => &ring::digest::SHA256,
        _ => return false,
    };
    let Ok(index) = std::fs::read(admin.join("index")) else {
        return false;
    };
    let Some(body_len) = index.len().checked_sub(algorithm.output_len()) else {
        return false;
    };
    let (body, checksum) = index.split_at(body_len);
    head.status.success()
        && ring::digest::digest(algorithm, body).as_ref() == checksum
        && pinned_tip.is_none_or(|pinned| pinned == tip)
        && git()
            .args([
                "diff-index",
                "--cached",
                "--quiet",
                "--ita-visible-in-index",
                "--ignore-submodules=none",
                tip,
                "--",
            ])
            .output()
            .is_ok_and(|out| out.status.success())
}

// Windows' Git for Windows and `std::fs::canonicalize` can spell the same
// worktree with different separator, case, or `\\?\` prefix forms. The path
// is already constrained to an absolute path from this lease; normalize those
// Windows spelling differences before matching the stale administrative link.
// trace:BUG-1777 | ai:codex
fn worktree_gitdir_matches(recorded: &str, expected: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        let key = |path: &str| {
            let path = path.replace('\\', "/");
            let path = path
                .strip_prefix("//?/UNC/")
                .map(|unc| format!("//{unc}"))
                .or_else(|| path.strip_prefix("//?/").map(str::to_string))
                .unwrap_or(path);
            path.trim_end_matches('/').to_ascii_lowercase()
        };
        return key(recorded) == key(&expected.to_string_lossy());
    }
    #[cfg(not(windows))]
    {
        std::path::Path::new(recorded) == expected
    }
}
