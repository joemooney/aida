//! The reviewer-worktree cleanup's one door to merge-hold state (M1 slice
//! 2): a legacy marker a reviewer worktree still carries is collected onto
//! the PR's gate ledger in the main worktree's shared state. Kept apart from
//! `gate/dispatch_*`, which reaches outside `gate/` only through its facade,
//! so the dispatch code names this module rather than the hold store.
// mandate-gate
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude

use std::path::{Path, PathBuf};

/// `<root>/.aida/merge-holds`: where a pre-slice-2 reviewer left markers.
pub(crate) fn marker_dir(root: &Path) -> PathBuf {
    crate::gate::ledger::legacy::marker_dir(root)
}

/// Collect the marker `body` a reviewer worktree held for PR `pr` onto the
/// PR's gate ledger from `main_worktree`, composed with any open hold.
/// `Ok(false)` when the body does not parse as a marker.
pub(crate) fn collect_marker(main_worktree: &Path, pr: u64, body: &str) -> std::io::Result<bool> {
    let Some(record) = crate::gate::merge_hold_core::parse_hold_marker(Ok(body.to_string()), pr)
    else {
        return Ok(false);
    };
    crate::gate::merge_hold_core::collect_legacy_hold(main_worktree, &record)?;
    Ok(true)
}

/// Whether PR `pr` is under an open hold, read from `main_worktree`.
pub(crate) fn hold_is_open(main_worktree: &Path, pr: u64) -> bool {
    crate::gate::merge_hold_core::hold_is_open(main_worktree, pr)
}
