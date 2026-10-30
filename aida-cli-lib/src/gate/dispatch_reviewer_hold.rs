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
    // The reviewer's mail is retired first; a refused retirement keeps the
    // tree and fails the cleanup. trace:STORY-1843 | ai:claude
    let removed = aida_core::mailbox_lifecycle::guard::remove_worktree_retained(
        main_worktree,
        rev_wt,
        true,
        "reviewer cleanup",
    );
    if let Err(e) = &removed {
        if e.downcast_ref::<aida_core::mailbox_lifecycle::guard::RetentionRefusal>()
            .is_some()
        {
            bail!("refusing to clean up reviewer worktree: {e:#}");
        }
    }
    if removed.is_err() && rev_wt.exists() {
        let _ = aida_core::git_ops::host_git_command(main_worktree)
            .args(["worktree", "prune"])
            .output();
    }
    Ok(())
}

/// Preserve any merge-hold marker `<rev_wt>/.aida/merge-holds/PR-<pr>` onto
/// the PR's gate ledger in `main_worktree`'s shared state (M1 slice 2), with
/// post-collection verification (F4). Any read, symlink, parse, write or
/// verification error aborts worktree cleanup so a hold is never lost.
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

    let wt_hold_dir = crate::gate::hold_collect::marker_dir(rev_wt);
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

    // M1 slice 2: the marker is collected onto the PR's gate ledger in the
    // main worktree's shared state, not copied as a file. A marker that does
    // not name a PR, or a ledger that does not answer with an open hold
    // afterwards, aborts the cleanup so a hold is never lost.
    // trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
    let pr = pr_scope
        .trim()
        .to_ascii_uppercase()
        .strip_prefix("PR-")
        .and_then(|n| n.parse::<u64>().ok())
        .with_context(|| format!("reviewer merge-hold scope {pr_scope} names no PR"))?;
    let collected = crate::gate::hold_collect::collect_marker(
        main_worktree,
        pr,
        &String::from_utf8_lossy(&wt_bytes),
    )
    .with_context(|| {
        format!(
            "cannot collect reviewer merge-hold {} onto the PR-{pr} gate ledger",
            wt_hold.display()
        )
    })?;
    if !collected {
        bail!("reviewer merge-hold {} did not parse", wt_hold.display());
    }
    if !crate::gate::hold_collect::hold_is_open(main_worktree, pr) {
        bail!(
            "the PR-{pr} gate ledger does not hold the collected merge-hold {}; refusing reviewer worktree cleanup",
            wt_hold.display()
        );
    }
    Ok(true)
}
