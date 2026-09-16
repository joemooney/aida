//! BUG-1167: the substrate merge-hold marker (ADR-37 layer 1, client side).
//!
//! A supervised (drive/guided/operator/decide/unset) PR gets a
//! `.aida/merge-holds/PR-<n>` marker. Every AIDA merge path funnels through
//! `forge::merge_change`, which refuses fail-closed while the marker exists — so
//! a concurrent merger (an integrator sweep, a second `aida agent` session, a
//! stale-binary drain, `aida pr merge`) cannot bypass the supervised-merge hold
//! the way BUG-1167 reproduced. The marker is cleared only by an explicit
//! human/advisor review (`aida merge-hold clear <pr>`), which is what "merge
//! requires human/advisor review" means made enforceable rather than advisory.
//!
//! This is the client half. The paired server half (ADR-37 layer 2) is a GitHub
//! required-status-check that reads the equivalent PR label, catching even a raw
//! `gh pr merge` that never touches AIDA. A drain-mode PR is never marked, so its
//! auto-merge is unaffected (the granularity the branch-protection concern needs).
// trace:BUG-1167 | ai:claude

use std::path::{Path, PathBuf};

fn holds_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("merge-holds")
}

/// The marker path for one PR. Public so callers can log it.
pub(crate) fn hold_path(project_root: &Path, pr: u64) -> PathBuf {
    holds_dir(project_root).join(format!("PR-{pr}"))
}

/// Record a supervised merge-hold for `pr` with a human-readable reason.
/// Idempotent — re-recording refreshes the reason.
pub(crate) fn write_hold(project_root: &Path, pr: u64, reason: &str) -> std::io::Result<()> {
    let dir = holds_dir(project_root);
    std::fs::create_dir_all(&dir)?;
    let reason = reason.trim();
    let body = if reason.is_empty() {
        format!("PR-{pr} is under a supervised merge-hold\n")
    } else {
        format!("{reason}\n")
    };
    std::fs::write(hold_path(project_root, pr), body)
}

/// The hold reason if `pr` is under a supervised merge-hold, else `None`.
/// The merge chokepoint refuses whenever this is `Some`.
///
/// TASK-1238: fails CLOSED on a present-but-unreadable marker. `NotFound` is the
/// only "no hold" answer; ANY other read error (permissions, a directory in its
/// place, a transient IO fault) means a marker may be there but we can't confirm
/// it isn't — so we HOLD. The prior `.ok()?` collapsed every error to `None`,
/// which would let a merge through when the marker was present but unreadable —
/// the exact fail-open class this whole marker exists to prevent.
pub(crate) fn read_hold(project_root: &Path, pr: u64) -> Option<String> {
    match std::fs::read_to_string(hold_path(project_root, pr)) {
        Ok(body) => {
            let reason = body.trim();
            Some(if reason.is_empty() {
                format!("PR-{pr} is under a supervised merge-hold")
            } else {
                reason.to_string()
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => Some(format!(
            "PR-{pr} merge-hold marker present but unreadable — held for safety"
        )),
    }
}

/// Clear the hold — an explicit human/advisor review. Idempotent: clearing a
/// PR with no hold is a no-op success.
pub(crate) fn clear_hold(project_root: &Path, pr: u64) -> std::io::Result<()> {
    match std::fs::remove_file(hold_path(project_root, pr)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Every active hold as `(pr_number, reason)`, ascending by PR.
// Consumed by the follow-up `aida merge-hold list` surface; kept here so the
// primitive lands with the safety core.
#[allow(dead_code)]
pub(crate) fn list_holds(project_root: &Path) -> Vec<(u64, String)> {
    let dir = holds_dir(project_root);
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(n) = name.strip_prefix("PR-").and_then(|s| s.parse::<u64>().ok()) {
                if let Some(reason) = read_hold(project_root, n) {
                    out.push((n, reason));
                }
            }
        }
    }
    out.sort_by_key(|(n, _)| *n);
    out
}

/// The GitHub label mirroring the merge-hold marker for ADR-37 Layer 2 (a
/// required status check keyed to this label blocks a raw/stale/UI merge the
/// client chokepoint never sees). trace on the item below stays a plain comment.
// trace:BUG-1167 | ai:claude
pub(crate) const HOLD_LABEL: &str = "aida:merge-hold";

/// Best-effort mirror of the marker state to the GitHub `aida:merge-hold` label,
/// so Layer 2 can enforce server-side. Failures are swallowed: the file marker
/// is the source of truth; the label is a convenience mirror and its absence
/// only weakens Layer 2, never the client chokepoint.
// trace:BUG-1167 | ai:claude
pub(crate) fn sync_label(project_root: &Path, pr: u64, held: bool) {
    let flag = if held {
        "--add-label"
    } else {
        "--remove-label"
    };
    let _ = std::process::Command::new("gh")
        .current_dir(project_root)
        .args(["pr", "edit", &pr.to_string(), flag, HOLD_LABEL])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_clear_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // No hold initially.
        assert!(read_hold(root, 42).is_none());
        // Write a hold → read returns the reason.
        write_hold(
            root,
            42,
            "STORY-1155 is marked drive — merge requires review",
        )
        .unwrap();
        let reason = read_hold(root, 42).expect("hold must be present");
        assert!(reason.contains("drive"), "{reason}");
        // Clear → gone.
        clear_hold(root, 42).unwrap();
        assert!(read_hold(root, 42).is_none());
        // Clearing an absent hold is a no-op success.
        clear_hold(root, 42).unwrap();
    }

    #[test]
    fn empty_reason_still_holds_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_hold(root, 7, "   ").unwrap();
        // An empty/whitespace reason must STILL register as a hold — the marker's
        // presence is the signal, never its content. A blank marker that merged
        // would be the fail-open bug this fix exists to prevent.
        assert!(
            read_hold(root, 7).is_some(),
            "a marker with a blank reason must still hold"
        );
    }

    #[test]
    fn present_but_unreadable_marker_holds_fail_closed() {
        // TASK-1238: a marker that EXISTS but can't be read as a file must HOLD,
        // not merge. Simulate it by putting a directory where the marker file
        // would be — `read_to_string` then errors with something other than
        // NotFound. The old `.ok()?` returned None here (fail-open → merge); the
        // fix must return Some.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(hold_path(root, 99)).unwrap();
        assert!(
            read_hold(root, 99).is_some(),
            "a present-but-unreadable marker must HOLD (fail closed), never merge"
        );
    }

    #[test]
    fn list_holds_enumerates_ascending() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_hold(root, 30, "b").unwrap();
        write_hold(root, 5, "a").unwrap();
        let holds = list_holds(root);
        assert_eq!(
            holds.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            vec![5, 30]
        );
    }
}
