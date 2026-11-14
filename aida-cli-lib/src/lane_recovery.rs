//! Preserve retry evidence without claiming completion or releasing leases.
// trace:BUG-2549 | ai:codex
use aida_core::{Comment, RequirementStatus, Storage};
use anyhow::{Context, Result};
use std::path::Path;

pub(crate) fn reconcile(root: &Path, spec: &str, branch: &str, head: Option<&str>) -> Result<()> {
    let storage = storage_for(root)?;
    reconcile_in(root, &storage, spec, branch, head)
}

// Queue callers already selected their store (including an explicit --file).
// trace:BUG-2549 | ai:codex
pub(crate) fn reconcile_in(
    root: &Path,
    storage: &Storage,
    spec: &str,
    branch: &str,
    head: Option<&str>,
) -> Result<()> {
    let leases = crate::leases::list_leases_strict_all(root).map_err(anyhow::Error::msg)?;
    let observed_head = match head {
        Some(head) => Some(head.to_owned()),
        None => exit_head(root, branch, &leases)?,
    };
    reconcile_observed_in(root, storage, spec, branch, observed_head.as_deref())
}

// An explicit unknown head must stay unknown after the source tree is gone.
// trace:BUG-2549 | ai:codex
pub(crate) fn reconcile_observed(
    root: &Path,
    spec: &str,
    branch: &str,
    head: Option<&str>,
) -> Result<()> {
    let storage = storage_for(root)?;
    reconcile_observed_in(root, &storage, spec, branch, head)
}

// trace:BUG-2549 | ai:codex
fn reconcile_observed_in(
    root: &Path,
    storage: &Storage,
    spec: &str,
    branch: &str,
    head: Option<&str>,
) -> Result<()> {
    // Exit facts must survive unavailable forge evidence and source teardown.
    storage
        .update_atomically(|store| {
            if let Ok(Some(req)) = store.get_requirement_unambiguous_mut(spec) {
                let note = format!(
                    "Lane exit evidence: retained branch `{branch}` at head `{}`.",
                    head.unwrap_or("unavailable")
                );
                if !req.comments.iter().any(|comment| comment.content == note) {
                    req.add_comment(Comment::new("lane-recovery".into(), note));
                }
            }
        })
        .context("failed to persist captured lane exit evidence before forge discovery")?;
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
        let held = shared_claim_holds(root, storage, store, spec).unwrap_or(true)
            || veto
            || leases.iter().any(|lease| {
                lease.branch == branch
                    || store
                        .get_requirement_by_spec_id(&lease.scope)
                        .zip(store.get_requirement_by_spec_id(spec))
                        .is_some_and(|(a, b)| a.id == b.id)
                    || lease.scope.eq_ignore_ascii_case(spec)
            });
        if let Ok(Some(req)) = store.get_requirement_unambiguous_mut(spec) {
            apply(req, branch, head, held);
        }
    })?;
    Ok(())
}

// Refs merges can arrive through an ordinary pull, with their head branch
// already deleted. Record the merge evidence without inventing a branch/head.
// trace:BUG-2549 | ai:codex
pub(crate) fn reconcile_reference(root: &Path, spec: &str, merge: &str) -> Result<()> {
    let storage = storage_for(root)?;
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
        let held = shared_claim_holds(root, storage, store, spec).unwrap_or(true) || veto || leases.iter().any(|lease| lease.scope.eq_ignore_ascii_case(spec)
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

// Use the root-scoped legacy resolver; a missing store must not create a new
// .aida-store beside an existing YAML/SQLite store.
// trace:BUG-2549 | ai:codex
fn storage_for(root: &Path) -> Result<Storage> {
    let path = match crate::detect_distributed_store_from(root) {
        Some(path) => path,
        None => aida_core::resolve_requirements_path_in(root, None, None, Path::new(""))?,
    };
    Ok(Storage::new(path))
}

// Recovery is deliberately conservative about cross-clone claims: even an old
// claim must be released/reconciled before we advertise its work as Approved.
// Read every claim so display/origin aliases and malformed records fail safe.
// trace:BUG-2549 | ai:codex
fn shared_claim_holds(
    root: &Path,
    storage: &Storage,
    store: &aida_core::RequirementsStore,
    spec: &str,
) -> Result<bool> {
    let store_root = crate::detect_distributed_store_from(root).unwrap_or_else(|| {
        if storage.path().is_dir() {
            storage.path().to_path_buf()
        } else {
            root.join(".aida-store")
        }
    });
    let dir = store_root.join("coordination/leases");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let wanted = store.get_requirement_unambiguous(spec)?.map(|r| r.id);
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let claim: crate::coordination::Claim = toml::from_str(&std::fs::read_to_string(path)?)?;
        if claim.scope.eq_ignore_ascii_case(spec)
            || wanted.is_some_and(|id| {
                store
                    .get_requirement_by_spec_id(&claim.scope)
                    .is_some_and(|r| r.id == id)
            })
        {
            return Ok(true);
        }
    }
    Ok(false)
}

// Never execute host Git against seat-controlled config. The existing probe
// contains Git and its descendants; an unreadable clone leaves head unknown.
// trace:BUG-2549 | ai:codex
pub(crate) fn exit_head(
    root: &Path,
    branch: &str,
    leases: &[crate::SessionLease],
) -> Result<Option<String>> {
    if let Some(lease) = leases.iter().find(|lease| lease.branch == branch) {
        if lease.tree_kind == crate::seat_clone::TreeKind::Clone {
            return Ok(match crate::seat_clone::probe(root, &lease.worktree_path) {
                crate::seat_clone::Probe::Known(facts) => facts.head,
                _ => None,
            });
        }
        if lease.worktree_path.try_exists()? {
            // The lease branch may no longer describe this tree (detached HEAD
            // or a branch switch). A failed live probe must stay unknown rather
            // than attributing the recorded branch's unrelated tip to the exit.
            return Ok(aida_core::git_ops::worktree_head_sha(&lease.worktree_path));
        }
    }
    let output = aida_core::git_ops::host_git_command(root)
        .args([
            "rev-parse",
            "--verify",
            &format!("refs/heads/{branch}^{{commit}}"),
        ])
        .output()?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
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
