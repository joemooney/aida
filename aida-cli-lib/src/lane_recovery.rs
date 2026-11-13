//! Preserve retry evidence without claiming completion or releasing leases.
// trace:BUG-2549 | ai:codex
use aida_core::{Comment, RequirementStatus, Storage};
use anyhow::Result;
use std::path::Path;

pub(crate) fn reconcile(root: &Path, spec: &str, branch: &str, head: Option<&str>) -> Result<()> {
    let forge = crate::forge::forge_for_open_change(root);
    // Failed/unknown forge observations must never be treated as absence.
    if !forge.open_changes_for_spec(spec)?.is_empty()
        || !matches!(
            forge.change_for_branch(branch)?,
            crate::forge::ChangeLookup::NoChange
        )
    {
        return Ok(());
    }
    let path =
        crate::detect_distributed_store_from(root).unwrap_or_else(|| root.join(".aida-store"));
    let storage = Storage::new(path);
    let leases = crate::leases::list_leases_strict_all(root).map_err(anyhow::Error::msg)?;
    let veto = crate::punt::signal_path(root, spec).try_exists()?
        || crate::punt::hold_signal_path(root, spec).try_exists()?;
    let tree = leases
        .iter()
        .find(|lease| lease.branch == branch)
        .map(|lease| lease.worktree_path.as_path())
        .unwrap_or(root);
    let observed_head = head.map(str::to_owned).or_else(|| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(tree)
            .args([
                "rev-parse",
                "--verify",
                &format!("refs/heads/{branch}^{{commit}}"),
            ])
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    });
    storage.update_atomically(|store| {
        // Re-read under the store writer lock: a new launch persists its lease
        // before claiming In Progress, so a completed claim cannot be reset
        // using the earlier head-discovery snapshot.
        let leases = match crate::leases::list_leases_strict_all(root) {
            Ok(leases) => leases,
            Err(error) => {
                eprintln!("warning: lane recovery retained {spec}: {error}");
                return;
            }
        };
        let held = veto
            || leases.iter().any(|lease| {
                lease.branch == branch
                    || store
                        .get_requirement_by_spec_id(&lease.scope)
                        .zip(store.get_requirement_by_spec_id(spec))
                        .is_some_and(|(a, b)| a.id == b.id)
                    || lease.scope.eq_ignore_ascii_case(spec)
            });
        if let Ok(Some(req)) = store.get_requirement_unambiguous_mut(spec) {
            apply(req, branch, observed_head.as_deref(), held);
        }
    })?;
    Ok(())
}

// Refs merges can arrive through an ordinary pull, with their head branch
// already deleted. Record the merge evidence without inventing a branch/head.
// trace:BUG-2549 | ai:codex
pub(crate) fn reconcile_reference(root: &Path, spec: &str, merge: &str) -> Result<()> {
    let storage = Storage::new(
        crate::detect_distributed_store_from(root).unwrap_or_else(|| root.join(".aida-store")),
    );
    reconcile_reference_in(root, &storage, spec, merge)
}

// Pull already pins the selected store, including legacy YAML stores.
// trace:BUG-2549 | ai:codex
pub(crate) fn reconcile_reference_in(
    root: &Path,
    storage: &Storage,
    spec: &str,
    merge: &str,
) -> Result<()> {
    let snapshot = storage.load()?;
    let Some(req) = snapshot.get_requirement_unambiguous(spec)? else {
        return Ok(());
    };
    if req.status != RequirementStatus::InProgress {
        return Ok(());
    }
    let id = req.id;
    let forge = crate::forge::forge_for_open_change(root);
    if !forge.open_changes_for_spec(spec)?.is_empty() {
        return Ok(());
    }
    let veto = crate::punt::signal_path(root, spec).try_exists()?
        || crate::punt::hold_signal_path(root, spec).try_exists()?;
    storage.update_atomically(|store| {
        // Re-read under the store writer lock: a new launch persists its lease
        // before claiming In Progress, so a completed claim cannot be reset
        // using the earlier head-discovery snapshot.
        let leases = match crate::leases::list_leases_strict_all(root) {
            Ok(leases) => leases,
            Err(error) => {
                eprintln!("warning: lane recovery retained {spec}: {error}");
                return;
            }
        };
        let held = veto || leases.iter().any(|lease| lease.scope.eq_ignore_ascii_case(spec)
            || store.get_requirement_by_spec_id(&lease.scope).is_some_and(|r| r.id == id));
        if let Ok(Some(req)) = store.get_requirement_unambiguous_mut(spec) {
            if req.status == RequirementStatus::InProgress && !held {
                let note = format!("Reference merge `{merge}` left unfinished work with no lease or open PR; returned to Approved.");
                req.add_comment(Comment::new("lane-recovery".into(), note));
                aida_core::conflict::set_status_recorded(req, RequirementStatus::Approved, "lane-recovery");
            }
        }
    })?;
    Ok(())
}

fn apply(req: &mut aida_core::Requirement, branch: &str, head: Option<&str>, held: bool) {
    let note = format!(
        "Lane recovery: no open PR; retained branch `{branch}` at head `{}`.",
        head.unwrap_or("unavailable")
    );
    if !req.comments.iter().any(|comment| comment.content == note) {
        req.add_comment(Comment::new("lane-recovery".into(), note));
    }
    if !held && req.status == RequirementStatus::InProgress {
        aida_core::conflict::set_status_recorded(req, RequirementStatus::Approved, "lane-recovery");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bug_2549_prless_exit_preserves_head_and_returns_unheld_work() {
        let mut req = aida_core::Requirement::new("retry".into(), String::new());
        req.status = RequirementStatus::InProgress;
        apply(&mut req, "bug-2549", Some("abc123"), false);
        assert_eq!(req.status, RequirementStatus::Approved);
        assert!(req.comments[0]
            .content
            .contains("bug-2549` at head `abc123"));
        apply(&mut req, "bug-2549", Some("abc123"), false);
        assert_eq!(req.comments.len(), 1);
    }

    #[test]
    fn bug_2549_retained_lease_blocks_reset_but_keeps_evidence() {
        let mut req = aida_core::Requirement::new("retry".into(), String::new());
        req.status = RequirementStatus::InProgress;
        apply(&mut req, "bug-2549", Some("abc123"), true);
        assert_eq!(req.status, RequirementStatus::InProgress);
        assert_eq!(req.comments.len(), 1);
        apply(&mut req, "bug-2549", Some("abc123"), false);
        assert_eq!(req.status, RequirementStatus::Approved);
    }

    #[test]
    fn bug_2549_recovery_preserves_manual_and_terminal_states() {
        for status in [
            RequirementStatus::NeedsAttention,
            RequirementStatus::Completed,
            RequirementStatus::Rejected,
            RequirementStatus::Draft,
        ] {
            let mut req = aida_core::Requirement::new("retry".into(), String::new());
            req.status = status.clone();
            apply(&mut req, "bug-2549", None, false);
            assert_eq!(req.status, status);
        }
    }
}
