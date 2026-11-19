//! Declared completeness drives the implementer's Done signal, not the PR
//! title trailer (advisor sketch on BUG-2696, D1-D3).
//!
//! A publication either completes its spec for its scope (`completes_spec`)
//! or is one slice of it. A completing push sets the spec Done on every
//! round, a first push and every rework push alike; a partial push leaves
//! the status alone. The commit trailer is derived from the same decision:
//! `(SPEC)` when it completes, `(Refs SPEC)` otherwise.
// trace:BUG-2696 | ai:claude
use aida_core::{Comment, Requirement, RequirementStatus, RequirementsStore, Storage};
use anyhow::Result;
use std::path::Path;

/// History author for the Done write a completing implementer push makes.
pub(crate) const PUBLICATION_DONE_AUTHOR: &str = aida_core::conflict::PUBLICATION_DONE_AUTHOR;

/// True when `req` has a recorded slice plan with slices still outstanding:
/// a child spec (an outgoing `Parent` edge, the shape `aida decompose` and
/// `aida add --parent` record) that is neither terminal nor archived. A
/// publication for such a spec is one slice, never the whole spec.
pub(crate) fn slice_plan_outstanding(store: &RequirementsStore, req: &Requirement) -> bool {
    req.relationships
        .iter()
        .filter(|rel| rel.rel_type == aida_core::RelationshipType::Parent)
        .filter_map(|rel| store.requirements.iter().find(|r| r.id == rel.target_id))
        .any(|child| !child.archived && !aida_core::conflict::is_terminal_status(&child.status))
}

/// D1: whether a publication completes `spec`. False when the lane declared
/// a partial slice or the spec has outstanding slices; true otherwise.
pub(crate) fn completes_spec(store: Option<&RequirementsStore>, spec: &str, partial: bool) -> bool {
    if partial {
        return false;
    }
    let Some(store) = store else {
        return true;
    };
    match store.get_requirement_unambiguous(spec) {
        Ok(Some(req)) => !slice_plan_outstanding(store, req),
        _ => true,
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

/// D3: after a successful implementer push of `tip` whose subject is
/// `subject`, set `spec` Done when the publication completes it. Writes the
/// shared store and pushes it so lane hosts see the status. Returns whether
/// the status moved. Callers treat an error as a warning: the push itself
/// already succeeded.
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
    let store = storage.load()?;
    let Some(req) = store.get_requirement_unambiguous(spec)? else {
        return Ok(false);
    };
    if !subject_completes(Some(&store), subject, req) || slice_plan_outstanding(&store, req) {
        return Ok(false);
    }
    let mut moved = false;
    storage.update_spec_atomically(req, |r| moved = done_after_push_flip(r, tip))?;
    if moved && storage.path().is_dir() {
        crate::store_push_pull::auto_push_store_best_effort(
            storage.path(),
            "implementer push → Done",
        );
    }
    Ok(moved)
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
