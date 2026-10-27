//! Reviewer-worktree merge-hold preservation (STORY-1889, moved verbatim
//! from `dispatch.rs`). A dead reviewer worktree `aida-pr-<N>` may carry the
//! `.aida/merge-holds/PR-<N>` marker that holds its PR; removing the worktree
//! without first moving that marker to the main worktree would release the
//! hold. [`remove_reviewer_worktree_preserving_hold`] is the only path that
//! removes a reviewer worktree, and it preserves the marker first.
// mandate-gate
// trace:STORY-1889 trace:STORY-1917 | ai:claude

use anyhow::{bail, Context, Result};
use std::path::Path;
use uuid::Uuid;

use crate::gate::dispatch_io::{read_bounded_regular_file, MAX_BRIEF_BYTES};

/// Preserve `rev_wt`'s merge-hold marker for `pr_scope` into the main
/// worktree, then remove `rev_wt` with `git worktree remove --force`
/// (pruning when removal fails). Any preservation failure aborts before the
/// removal, so a hold is never lost.
// trace:STORY-1889 trace:STORY-1917 | ai:claude
pub(crate) fn remove_reviewer_worktree_preserving_hold(
    main_worktree: &Path,
    rev_wt: &Path,
    pr_scope: &str,
) -> Result<()> {
    preserve_reviewer_worktree_hold(main_worktree, rev_wt, pr_scope)?;
    let out = aida_core::git_ops::host_git_command(main_worktree)
        .args(["worktree", "remove", "--force"])
        .arg(rev_wt)
        .output()
        .with_context(|| format!("failed to run git worktree remove on {}", rev_wt.display()))?;
    if !out.status.success() && rev_wt.exists() {
        let _ = aida_core::git_ops::host_git_command(main_worktree)
            .args(["worktree", "prune"])
            .output();
    }
    Ok(())
}

/// Preserve any merge-hold marker `<rev_wt>/.aida/merge-holds/PR-<pr>` into
/// `<main_worktree>/.aida/merge-holds/PR-<pr>` using atomic no-clobber
/// publication and post-transfer verification (F4). Any read, write, symlink,
/// or verification error aborts worktree cleanup so a hold is never lost.
// trace:STORY-1917 | ai:agy
// trace:STORY-1889 | ai:claude
pub(in crate::gate) fn preserve_reviewer_worktree_hold(
    main_worktree: &Path,
    rev_wt: &Path,
    pr_scope: &str,
) -> Result<bool> {
    let wt_aida = rev_wt.join(".aida");
    match std::fs::symlink_metadata(&wt_aida) {
        Ok(m) if m.file_type().is_symlink() => {
            let main_aida_canon = main_worktree
                .join(".aida")
                .canonicalize()
                .unwrap_or_else(|_| main_worktree.join(".aida"));
            let wt_aida_canon = wt_aida.canonicalize().unwrap_or_else(|_| wt_aida.clone());
            if wt_aida_canon != main_aida_canon {
                bail!(
                    "refusing reviewer worktree cleanup: {} is a symlink outside main worktree .aida",
                    wt_aida.display()
                );
            }
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(e).with_context(|| format!("cannot stat {}", wt_aida.display()));
        }
    }

    let wt_hold_dir = wt_aida.join("merge-holds");
    match std::fs::symlink_metadata(&wt_hold_dir) {
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            bail!(
                "refusing reviewer worktree cleanup: {} is a symlink or not a directory",
                wt_hold_dir.display()
            );
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(e).with_context(|| format!("cannot stat {}", wt_hold_dir.display()));
        }
    }

    let wt_hold = wt_hold_dir.join(pr_scope);
    match std::fs::symlink_metadata(&wt_hold) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("cannot stat reviewer merge-hold {}", wt_hold.display()));
        }
    }

    // Source hold exists in `rev_wt`: read and validate it with `O_NOFOLLOW`.
    // If unreadable, empty, or a symlink, abort cleanup immediately!
    let wt_bytes = read_bounded_regular_file(&wt_hold, MAX_BRIEF_BYTES, false)
        .with_context(|| format!("cannot read reviewer merge-hold {}", wt_hold.display()))?;
    if wt_bytes.is_empty() {
        bail!(
            "refusing reviewer worktree cleanup: merge-hold {} is empty",
            wt_hold.display()
        );
    }

    let main_hold_dir = main_worktree.join(".aida").join("merge-holds");
    std::fs::create_dir_all(&main_hold_dir)
        .with_context(|| format!("cannot create {}", main_hold_dir.display()))?;
    let main_dir_meta = std::fs::symlink_metadata(&main_hold_dir)
        .with_context(|| format!("cannot stat {}", main_hold_dir.display()))?;
    if main_dir_meta.file_type().is_symlink() || !main_dir_meta.is_dir() {
        bail!(
            "refusing reviewer worktree cleanup: {} is a symlink or not a directory",
            main_hold_dir.display()
        );
    }

    let main_hold = main_hold_dir.join(pr_scope);
    let tmp_hold = main_hold_dir.join(format!(".{pr_scope}.tmp-{}", Uuid::new_v4()));
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut f = opts.open(&tmp_hold).with_context(|| {
            format!(
                "cannot stage merge-hold transfer in {}",
                main_hold_dir.display()
            )
        })?;
        if let Err(e) = f.write_all(&wt_bytes).and_then(|()| f.sync_all()) {
            drop(f);
            let _ = std::fs::remove_file(&tmp_hold);
            return Err(e).with_context(|| {
                format!("failed writing staged merge-hold {}", tmp_hold.display())
            });
        }
    }

    match crate::gate::review_verdict::publish_verdict_no_replace(&tmp_hold, &main_hold) {
        Ok(()) => {
            let landed = read_bounded_regular_file(&main_hold, MAX_BRIEF_BYTES, false)
                .with_context(|| {
                    format!(
                        "cannot verify transferred merge-hold at {}",
                        main_hold.display()
                    )
                })?;
            if landed != wt_bytes {
                bail!(
                    "transferred merge-hold {} content did not match source {}",
                    main_hold.display(),
                    wt_hold.display()
                );
            }
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(&tmp_hold);
            // Destination hold already exists (pre-existing or concurrently placed);
            // verify it is a valid non-empty regular file before allowing cleanup.
            let existing = read_bounded_regular_file(&main_hold, MAX_BRIEF_BYTES, false)
                .with_context(|| {
                    format!(
                        "existing destination merge-hold {} is invalid or unreadable",
                        main_hold.display()
                    )
                })?;
            if existing.is_empty() {
                bail!(
                    "existing destination merge-hold {} is empty; refusing reviewer worktree cleanup",
                    main_hold.display()
                );
            }
            Ok(true)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_hold);
            Err(e).with_context(|| {
                format!(
                    "failed atomic no-clobber transfer of merge-hold to {}",
                    main_hold.display()
                )
            })
        }
    }
}
