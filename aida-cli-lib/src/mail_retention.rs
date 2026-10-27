//! CLI side of the mail-retention guard (STORY-1843 C2): retire a session's
//! tree before it is removed, reset or handed on, and the recovery-ownership
//! ending when retirement refuses.
//!
//! A refusal keeps the tree, the lease, the claim, the pin and the manifest.
//! Security still proceeds: the lease's child grants are revoked, and the
//! lease is stamped *ending-retained* (`ending_retained_at`), so it reads as
//! neither active work nor cleaned up. The next end, reap or heal retries.
//!
//! trace:STORY-1843 | ai:claude

use aida_core::mailbox_lifecycle::guard::{self, Gate, RetentionRefusal, Retirement};
use std::path::Path;

/// Retire the mail source of `worktree`, a seat clone when `is_clone`. A
/// clone's retention home is its main checkout's anchor, resolved only
/// through the verified host-recorded binding; without one, only a clone
/// with no mail bytes may go.
pub(crate) fn retire_tree(
    project_root: &Path,
    worktree: &Path,
    is_clone: bool,
    reason: &str,
) -> Gate {
    if is_clone {
        let main = verified_clone_main(project_root, worktree);
        guard::retire_clone_checkout(main.as_deref(), worktree, reason)
    } else {
        guard::retire_checkout(worktree, reason)
    }
}

/// `main` as the verified host binding of the seat clone at `clone`, or
/// `None` when the binding is missing, unverified, stale, or names another
/// main.
pub(crate) fn verified_clone_main(main: &Path, clone: &Path) -> Option<std::path::PathBuf> {
    aida_core::seat_clone_detect::note_host_main(main);
    let recorded = crate::seat_clone::host_recorded_clone(clone)?;
    let main = main.canonicalize().ok()?;
    (recorded.main == main).then_some(main)
}

/// Remove the linked worktree at `worktree` behind the barrier, for the CLI
/// entries that used to run a raw `git worktree remove`.
pub(crate) fn remove_worktree(
    project_root: &Path,
    worktree: &Path,
    force: bool,
    reason: &str,
) -> anyhow::Result<()> {
    guard::remove_worktree_retained(project_root, worktree, force, reason)
}

/// Settle a retirement after a step that did not destroy the tree (it was
/// kept for another reason): reopen it. Best-effort; a failure is reported
/// and the next admission reopens the abandoned source.
pub(crate) fn reopen_quietly(retirement: Option<Retirement>) {
    if let Some(retirement) = retirement {
        if let Err(e) = retirement.reopen() {
            eprintln!("warning: the retired mailbox could not be reopened: {e}");
        }
    }
}

/// The recovery-ownership ending for a refused retirement: revoke the lease's
/// child grants and stamp it ending-retained. The lease, claim and manifest
/// stay.
pub(crate) fn keep_ending_retained(lease_file: &Path, refusal: &RetentionRefusal) {
    crate::session::revoke_lease_child_grant(lease_file);
    if let Err(e) = stamp_ending_retained(lease_file, &refusal.reason) {
        eprintln!(
            "warning: could not mark lease {} ending-retained: {e:#}",
            lease_file.display()
        );
    }
}

/// Patch `ending_retained_at` / `ending_retained_reason` into the lease as
/// generic TOML keys, so keys this binary does not model survive.
fn stamp_ending_retained(lease_file: &Path, reason: &str) -> anyhow::Result<()> {
    let body = std::fs::read_to_string(lease_file)?;
    let mut table: toml::Table = body.parse()?;
    table.insert(
        "ending_retained_at".into(),
        toml::Value::String(chrono::Utc::now().to_rfc3339()),
    );
    table.insert(
        "ending_retained_reason".into(),
        toml::Value::String(reason.to_string()),
    );
    aida_core::write_atomic(lease_file, toml::to_string_pretty(&table)?.as_bytes())?;
    Ok(())
}

/// The operator-facing lines for a refusal, naming the activation command
/// when that is what is missing.
pub(crate) fn refusal_lines(refusal: &RetentionRefusal) -> Vec<String> {
    let mut lines = vec![format!("{refusal}")];
    if refusal.needs_activation {
        lines.push(format!(
            "check whether every mail writer on this machine takes a write permit: `{}`",
            aida_core::mailbox_lifecycle::ACTIVATION_COMMAND
        ));
    }
    if let Some(dir) = &refusal.retained_dir {
        lines.push(format!("a preserved copy is kept at {}", dir.display()));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(needs_activation: bool) -> RetentionRefusal {
        RetentionRefusal {
            checkout: "/w".into(),
            reason: "the mailbox holds mail".into(),
            needs_activation,
            retained_dir: Some("/anchor/retained/k/3".into()),
        }
    }

    #[test]
    fn a_refused_end_stamps_the_lease_ending_retained_and_keeps_its_keys() {
        let dir = tempfile::tempdir().unwrap();
        let lease = dir.path().join("s.toml");
        std::fs::write(&lease, "id = \"s\"\nunmodelled_key = \"kept\"\n").unwrap();
        keep_ending_retained(&lease, &refusal(true));
        let table: toml::Table = std::fs::read_to_string(&lease).unwrap().parse().unwrap();
        assert_eq!(table["id"].as_str(), Some("s"), "the lease is kept");
        assert_eq!(table["unmodelled_key"].as_str(), Some("kept"));
        assert!(table["ending_retained_at"].as_str().is_some());
        assert_eq!(
            table["ending_retained_reason"].as_str(),
            Some("the mailbox holds mail")
        );
    }

    #[test]
    fn the_refusal_names_the_activation_command_only_when_it_is_missing() {
        let command = aida_core::mailbox_lifecycle::ACTIVATION_COMMAND;
        let lines = refusal_lines(&refusal(true)).join("\n");
        assert!(lines.contains(command), "{lines}");
        assert!(lines.contains("/anchor/retained/k/3"), "{lines}");
        assert!(!refusal_lines(&refusal(false)).join("\n").contains(command));
    }
}
