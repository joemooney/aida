//! Declared completeness drives the implementer's Done signal, not the PR
//! title trailer (advisor sketch on BUG-2696, D1-D3).
//!
//! A publication either completes its spec for its scope (`completes_spec`)
//! or is one slice of it. A completing push sets the spec Done on every
//! round, a first push and every rework push alike; a partial push leaves
//! the status alone. The commit trailer is derived from the same decision:
//! `(SPEC)` when it completes, `(Refs SPEC)` otherwise.
//!
//! Round-2 ruling (A1, A2): a slice plan is ONLY a set of outgoing `Parent`
//! edges to child specs. Advisors decompose sliced work into child specs; a
//! plan written in comment prose or a plan document is not a slice plan and
//! is never consulted. The host computes outstanding children itself, and
//! every decision here fails closed: a missing store, an unresolvable spec, a
//! failed read or a graph that changed under the writer lock all mean NOT
//! completing.
// trace:BUG-2696 | ai:claude
use aida_core::{Comment, Requirement, RequirementStatus, RequirementsStore, Storage};
use anyhow::Result;
use std::path::Path;

/// History author for the Done write a completing implementer push makes.
pub(crate) const PUBLICATION_DONE_AUTHOR: &str = aida_core::conflict::PUBLICATION_DONE_AUTHOR;

/// True when `req` has a recorded slice plan with slices still outstanding:
/// a child spec (an outgoing `Parent` edge, the shape `aida decompose` and
/// `aida add --parent` record) that is neither terminal nor archived. A
/// publication for such a spec is one slice, never the whole spec. An edge
/// whose child cannot be found counts as outstanding (fail closed, A2).
pub(crate) fn slice_plan_outstanding(store: &RequirementsStore, req: &Requirement) -> bool {
    req.relationships
        .iter()
        .filter(|rel| rel.rel_type == aida_core::RelationshipType::Parent)
        .any(
            |rel| match store.requirements.iter().find(|r| r.id == rel.target_id) {
                Some(child) => {
                    !child.archived && !aida_core::conflict::is_terminal_status(&child.status)
                }
                None => true,
            },
        )
}

/// The slice-plan edges of `req`: its outgoing `Parent` targets, sorted.
fn slice_edges(req: &Requirement) -> Vec<String> {
    let mut edges: Vec<String> = req
        .relationships
        .iter()
        .filter(|rel| rel.rel_type == aida_core::RelationshipType::Parent)
        .map(|rel| rel.target_id.to_string())
        .collect();
    edges.sort();
    edges
}

/// F4 (A2): the check a Done writer runs on the copy it reads under the
/// store write lock. The completion decision was made on an unlocked
/// snapshot; when the spec's slice plan changed since (an advisor added or
/// removed a child), that decision is stale and the writer refuses.
pub(crate) fn slice_plan_unchanged(snapshot: &Requirement, locked: &Requirement) -> bool {
    slice_edges(snapshot) == slice_edges(locked)
}

/// D1: whether a publication completes `spec`. False when the lane declared
/// a partial slice or the spec has outstanding slices, and (A2) whenever the
/// host cannot establish that it has none: no store, or a spec it cannot
/// resolve.
pub(crate) fn completes_spec(store: Option<&RequirementsStore>, spec: &str, partial: bool) -> bool {
    if partial {
        return false;
    }
    let Some(store) = store else {
        return false;
    };
    match store.get_requirement_unambiguous(spec) {
        Ok(Some(req)) => !slice_plan_outstanding(store, req),
        _ => false,
    }
}

/// D2: the subject with its trailing `(ID)` / `(Refs ID)` group replaced by
/// the trailer `completes` derives.
pub(crate) fn derive_trailer(subject: &str, spec: &str, completes: bool) -> Result<String> {
    let prefix = subject
        .trim_end()
        .rsplit_once('(')
        .map(|(head, _)| head)
        .ok_or_else(|| anyhow::anyhow!("publication subject has no trailer"))?;
    Ok(format!(
        "{prefix}({}{spec})",
        if completes { "" } else { "Refs " }
    ))
}

/// Whether a commit subject's trailing group completes the spec `target`
/// (a bare `(ID)` that resolves to it, never a `(Refs ID)`).
pub(crate) fn subject_completes(
    store: Option<&RequirementsStore>,
    subject: &str,
    target: &Requirement,
) -> bool {
    if crate::pr_ship::refs_spec_ids_in_text(subject)
        .iter()
        .any(|id| names(store, id, target))
    {
        return false;
    }
    crate::pr_ship::extract_trailing_spec_ids_from_subject(subject)
        .iter()
        .any(|id| names(store, id, target))
}

fn names(store: Option<&RequirementsStore>, id: &str, target: &Requirement) -> bool {
    match store.and_then(|s| s.get_requirement_unambiguous(id).ok().flatten()) {
        Some(found) => found.id == target.id,
        None => [target.spec_id.as_deref(), target.agreed_id.as_deref()]
            .into_iter()
            .flatten()
            .any(|own| aida_core::id_identity::same_id(own, id)),
    }
}

/// The status write itself: Approved / Planned / In Progress move to Done.
/// Any other status (Draft, a shelved spec, Done already, terminal) stays.
pub(crate) fn done_after_push_flip(req: &mut Requirement, tip: &str) -> bool {
    if !matches!(
        req.status,
        RequirementStatus::Approved | RequirementStatus::Planned | RequirementStatus::InProgress
    ) {
        return false;
    }
    aida_core::conflict::set_status_recorded(req, RequirementStatus::Done, PUBLICATION_DONE_AUTHOR);
    req.modified_at = chrono::Utc::now();
    let short = tip.get(..12).unwrap_or(tip);
    req.add_comment(Comment::new(
        PUBLICATION_DONE_AUTHOR.to_string(),
        format!(
            "Implementer push {short} declared this spec complete for its scope; Done until \
             it lands."
        ),
    ));
    true
}

/// D3: after a successful implementer push of `tip` whose publication
/// subject is `subject`, set `spec` Done when the publication completes it.
/// Writes the shared store and confirms its push so lane hosts see the
/// status. Returns whether the status moved; an unconfirmed push is an error
/// (the Done write stays recorded locally and the push is retried, see
/// [`propagate_store_writes`]). Callers treat an error as a warning: the code
/// push itself already succeeded.
pub(crate) fn mark_done_after_push(
    project_root: &Path,
    spec: &str,
    subject: &str,
    tip: &str,
) -> Result<bool> {
    let storage = crate::lane_recovery::storage_for(project_root)?;
    mark_done_after_push_in(&storage, spec, subject, tip)
}

pub(crate) fn mark_done_after_push_in(
    storage: &Storage,
    spec: &str,
    subject: &str,
    tip: &str,
) -> Result<bool> {
    mark_done_after_push_with(storage, spec, subject, tip, || {})
}

/// [`mark_done_after_push_in`] with a seam between the unlocked completion
/// decision and the locked write, so a test can change the graph there.
pub(crate) fn mark_done_after_push_with(
    storage: &Storage,
    spec: &str,
    subject: &str,
    tip: &str,
    between: impl FnOnce(),
) -> Result<bool> {
    let store = storage.load()?;
    let snapshot = match store.get_requirement_unambiguous(spec)? {
        Some(req)
            if subject_completes(Some(&store), subject, req)
                && !slice_plan_outstanding(&store, req) =>
        {
            req.clone()
        }
        _ => {
            // Nothing to write, but a landing or Done write whose push was
            // never confirmed is still owed. trace:BUG-2696 | ai:claude
            propagate_store_writes(storage.path(), false, "implementer push → Done")?;
            return Ok(false);
        }
    };
    between();
    let mut moved = false;
    storage.update_spec_atomically(&snapshot, |r| {
        if slice_plan_unchanged(&snapshot, r) {
            moved = done_after_push_flip(r, tip);
        }
    })?;
    propagate_store_writes(storage.path(), moved, "implementer push → Done")?;
    Ok(moved)
}

/// Name of the marker a git-canonical store keeps (in its git directory,
/// never in the tracked tree) while a status write it holds has not reached
/// the shared remote.
const PENDING_PUSH_MARKER: &str = "aida-pending-status-push";

fn pending_push_marker(store: &Path) -> Option<std::path::PathBuf> {
    let out = aida_core::git_ops::host_git_command(store)
        .args(["rev-parse", "--git-path", PENDING_PUSH_MARKER])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let rel = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!rel.is_empty()).then(|| store.join(rel))
}

/// D5 / F6 (BUG-2696): propagate lifecycle status writes to the shared
/// store, and keep owing them until a push is CONFIRMED. When `wrote`, a
/// pending marker is recorded first; then, whenever a marker is present
/// (this write or an earlier unconfirmed one), the store is pushed and the
/// marker cleared only on a confirmed push. `Err` means the writes are
/// recorded locally but not shared yet: the caller must report it and must
/// not checkpoint, and the next call retries the push even when it writes
/// nothing. A YAML / SQLite store has no shared remote: always `Ok`.
pub(crate) fn propagate_store_writes(store: &Path, wrote: bool, reason: &str) -> Result<()> {
    if !store.is_dir() {
        return Ok(());
    }
    let marker = pending_push_marker(store);
    if wrote {
        if let Some(marker) = &marker {
            if let Err(e) = std::fs::write(marker, format!("{reason}\n")) {
                eprintln!(
                    "  warning: could not record the pending store push ({}): {e}",
                    marker.display()
                );
            }
        }
    }
    let owed = wrote || marker.as_deref().is_some_and(Path::exists);
    if !owed {
        return Ok(());
    }
    match crate::store_push_pull::push_store_confirmed(store, reason) {
        Ok(()) => {
            if let Some(marker) = &marker {
                let _ = std::fs::remove_file(marker);
            }
            Ok(())
        }
        Err(why) => anyhow::bail!(
            "status recorded locally, but the shared-store push was not confirmed ({why}); \
             it is retried on the next landing scan or implementer push"
        ),
    }
}

/// [`mark_done_after_push`] reported on stderr; never fails the caller.
pub(crate) fn mark_done_after_push_reported(
    project_root: &Path,
    spec: &str,
    subject: &str,
    tip: &str,
) {
    let shown = aida_core::node_alias::display_spec_id(spec);
    match mark_done_after_push(project_root, spec, subject, tip) {
        Ok(true) => eprintln!("  {shown} → Done (the push completes it for its scope)"),
        Ok(false) => {}
        Err(e) => eprintln!("  warning: could not mark {shown} Done after the push: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, status: RequirementStatus) -> Requirement {
        let mut req = Requirement::new(format!("{id} title"), String::new());
        req.spec_id = Some(id.into());
        req.req_type = aida_core::RequirementType::Bug;
        req.status = status;
        req
    }

    fn store_with(reqs: Vec<Requirement>) -> (tempfile::TempDir, Storage) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().join("requirements.yaml"));
        let mut store = RequirementsStore::new();
        store.requirements = reqs;
        storage.save(&store).unwrap();
        (dir, storage)
    }

    fn status(storage: &Storage, id: &str) -> RequirementStatus {
        storage
            .load()
            .unwrap()
            .get_requirement_by_spec_id(id)
            .unwrap()
            .status
            .clone()
    }

    // AC1: every completing push sets Done, a rework round included.
    #[test]
    fn bug_2696_completing_push_sets_done_on_first_and_rework_rounds() {
        let (_dir, storage) = store_with(vec![spec("BUG-1", RequirementStatus::InProgress)]);
        assert!(mark_done_after_push_in(&storage, "BUG-1", "fix: a (BUG-1)", "aaa").unwrap());
        assert_eq!(status(&storage, "BUG-1"), RequirementStatus::Done);
        // `aida queue rework` returns the spec to Approved; the rework push
        // must set it Done again.
        storage
            .update_atomically(|s| {
                s.get_requirement_by_spec_id_mut("BUG-1").unwrap().status =
                    RequirementStatus::Approved;
            })
            .unwrap();
        assert!(mark_done_after_push_in(&storage, "BUG-1", "fix: b (BUG-1)", "bbb").unwrap());
        assert_eq!(status(&storage, "BUG-1"), RequirementStatus::Done);
        let after = storage.load().unwrap();
        let req = after.get_requirement_by_spec_id("BUG-1").unwrap();
        assert!(req
            .history
            .iter()
            .any(|h| h.author == PUBLICATION_DONE_AUTHOR));
    }

    // D3: a partial push leaves the status alone.
    #[test]
    fn bug_2696_partial_push_leaves_status_alone() {
        let (_dir, storage) = store_with(vec![spec("BUG-1", RequirementStatus::Approved)]);
        assert!(!mark_done_after_push_in(&storage, "BUG-1", "fix: a (Refs BUG-1)", "a").unwrap());
        assert!(
            !mark_done_after_push_in(&storage, "BUG-1", "fix: (Refs BUG-1) (BUG-1)", "a").unwrap()
        );
        assert!(!mark_done_after_push_in(&storage, "BUG-1", "fix: other (BUG-2)", "a").unwrap());
        assert_eq!(status(&storage, "BUG-1"), RequirementStatus::Approved);
    }

    // Done never reopens or overrides a non-pipeline status.
    #[test]
    fn bug_2696_done_flip_only_from_pipeline_states() {
        for (from, moves) in [
            (RequirementStatus::Approved, true),
            (RequirementStatus::Planned, true),
            (RequirementStatus::InProgress, true),
            (RequirementStatus::Draft, false),
            (RequirementStatus::Done, false),
            (RequirementStatus::Completed, false),
            (RequirementStatus::Rejected, false),
            (RequirementStatus::NeedsAttention, false),
        ] {
            let mut req = spec("BUG-1", from.clone());
            assert_eq!(done_after_push_flip(&mut req, "abc"), moves, "{from:?}");
        }
    }

    // AC6 / D1: an outstanding child slice makes the publication partial.
    #[test]
    fn bug_2696_outstanding_slice_plan_withholds_completion() {
        let mut parent = spec("STORY-1", RequirementStatus::InProgress);
        parent.req_type = aida_core::RequirementType::Story;
        let child = spec("TASK-2", RequirementStatus::Approved);
        parent.relationships.push(aida_core::Relationship {
            rel_type: aida_core::RelationshipType::Parent,
            target_id: child.id,
            created_at: Some(chrono::Utc::now()),
            created_by: None,
        });
        let (_dir, storage) = store_with(vec![parent.clone(), child.clone()]);
        let store = storage.load().unwrap();
        assert!(!completes_spec(Some(&store), "STORY-1", false));
        assert!(!completes_spec(Some(&store), "TASK-2", true));
        assert!(completes_spec(Some(&store), "TASK-2", false));
        assert!(!mark_done_after_push_in(&storage, "STORY-1", "feat: s1 (STORY-1)", "a").unwrap());
        assert_eq!(status(&storage, "STORY-1"), RequirementStatus::InProgress);
        // Once every slice is terminal the publication completes again.
        storage
            .update_atomically(|s| {
                s.get_requirement_by_spec_id_mut("TASK-2").unwrap().status =
                    RequirementStatus::Completed;
            })
            .unwrap();
        let store = storage.load().unwrap();
        assert!(completes_spec(Some(&store), "STORY-1", false));
    }

    fn parent_edge(target: uuid::Uuid) -> aida_core::Relationship {
        aida_core::Relationship {
            rel_type: aida_core::RelationshipType::Parent,
            target_id: target,
            created_at: Some(chrono::Utc::now()),
            created_by: None,
        }
    }

    // A2 / F3: no store, an unknown spec and a dangling child edge all mean
    // NOT completing.
    #[test]
    fn bug_2696_unknowable_completeness_fails_closed() {
        assert!(!completes_spec(None, "BUG-1", false));
        let mut parent = spec("STORY-1", RequirementStatus::InProgress);
        parent.relationships.push(parent_edge(uuid::Uuid::new_v4()));
        let mut store = RequirementsStore::new();
        store.requirements = vec![parent, spec("BUG-1", RequirementStatus::InProgress)];
        assert!(!completes_spec(Some(&store), "BUG-404", false));
        assert!(!completes_spec(Some(&store), "STORY-1", false));
        assert!(completes_spec(Some(&store), "BUG-1", false));
    }

    // F4: the completion decision is made on an unlocked snapshot. A child
    // slice an advisor adds before the writer takes the lock must refuse the
    // Done write, which re-checks the slice plan on the locked copy.
    #[test]
    fn bug_2696_slice_added_before_the_lock_refuses_done() {
        let parent = spec("STORY-1", RequirementStatus::InProgress);
        let child = spec("TASK-2", RequirementStatus::Approved);
        let child_id = child.id;
        let (_dir, storage) = store_with(vec![parent, child]);
        let raced = std::cell::Cell::new(false);
        let moved =
            mark_done_after_push_with(&storage, "STORY-1", "feat: x (STORY-1)", "a", || {
                raced.set(true);
                storage
                    .update_atomically(|s| {
                        s.get_requirement_by_spec_id_mut("STORY-1")
                            .unwrap()
                            .relationships
                            .push(parent_edge(child_id));
                    })
                    .unwrap();
            })
            .unwrap();
        assert!(
            raced.get(),
            "the seam ran between the decision and the write"
        );
        assert!(!moved);
        assert_eq!(status(&storage, "STORY-1"), RequirementStatus::InProgress);
        // Control: with no concurrent change the same push sets Done.
        let (_dir, storage) = store_with(vec![spec("STORY-1", RequirementStatus::InProgress)]);
        assert!(
            mark_done_after_push_with(&storage, "STORY-1", "feat: x (STORY-1)", "a", || {})
                .unwrap()
        );
        assert_eq!(status(&storage, "STORY-1"), RequirementStatus::Done);
    }

    // D2: a rework push re-derives the existing PR title's trailer.
    #[test]
    fn bug_2696_rework_title_follows_the_tip_trailer() {
        let mut store = RequirementsStore::new();
        store.requirements = vec![spec("BUG-1", RequirementStatus::Approved)];
        let align = |title: &str, tip: &str| {
            crate::ship_from_seat::aligned_title(Some(&store), title, "BUG-1", tip)
        };
        assert_eq!(
            align("fix: x (Refs BUG-1)", "fix: rework (BUG-1)").as_deref(),
            Some("fix: x (BUG-1)")
        );
        assert_eq!(
            align("fix: x (BUG-1)", "fix: slice (Refs BUG-1)").as_deref(),
            Some("fix: x (Refs BUG-1)")
        );
        assert_eq!(align("fix: x (BUG-1)", "fix: rework (BUG-1)"), None);
        assert_eq!(align("fix: x (BUG-1)", "chore: no trailer"), None);
        assert_eq!(align("fix: x (BUG-1 BUG-2)", "fix: (Refs BUG-1)"), None);
        assert_eq!(align("fix: x (BUG-2)", "fix: (Refs BUG-1)"), None);
    }

    #[test]
    fn bug_2696_trailer_is_derived_from_completeness() {
        assert_eq!(
            derive_trailer("fix: x (Refs BUG-1)", "BUG-1", true).unwrap(),
            "fix: x (BUG-1)"
        );
        assert_eq!(
            derive_trailer("fix: x (BUG-1)", "BUG-1", false).unwrap(),
            "fix: x (Refs BUG-1)"
        );
        assert!(derive_trailer("no trailer", "BUG-1", true).is_err());
    }
}
