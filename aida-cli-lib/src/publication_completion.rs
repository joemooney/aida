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
///
/// A plan followup is not a slice: `aida queue done`, `aida edit` and the
/// landing scan file a plan's `## Followups` bullets as `--parent` children
/// once the spec is already Done, stamped with a `followup-src:` provenance
/// tag. They are follow-on work after the spec, so they never hold it open.
// trace:BUG-2696 | ai:claude
pub(crate) fn slice_plan_outstanding(store: &RequirementsStore, req: &Requirement) -> bool {
    req.relationships
        .iter()
        .filter(|rel| rel.rel_type == aida_core::RelationshipType::Parent)
        .any(
            |rel| match store.requirements.iter().find(|r| r.id == rel.target_id) {
                Some(child) => {
                    !child.archived
                        && !aida_core::conflict::is_terminal_status(&child.status)
                        && !is_plan_followup(child)
                }
                None => true,
            },
        )
}

/// True when `child` was filed from a plan's `## Followups` section (it
/// carries the `followup-src:` provenance tag the filer stamps).
fn is_plan_followup(child: &Requirement) -> bool {
    child
        .tags
        .iter()
        .any(|t| t.starts_with(crate::plan_followups::FOLLOWUP_SRC_TAG_PREFIX))
}

/// The slice-plan graph a completion decision rests on: every outgoing
/// `Parent` edge of the spec with its child's status and archive flag
/// (`None` when the child does not exist), sorted by child.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SliceEvidence(Vec<(uuid::Uuid, Option<(RequirementStatus, bool)>)>);

/// Capture [`SliceEvidence`] for `req` from `store`.
pub(crate) fn slice_evidence(store: &RequirementsStore, req: &Requirement) -> SliceEvidence {
    let mut edges: Vec<_> = req
        .relationships
        .iter()
        .filter(|rel| rel.rel_type == aida_core::RelationshipType::Parent)
        .map(|rel| {
            let child = store
                .requirements
                .iter()
                .find(|r| r.id == rel.target_id)
                .map(|c| (c.status.clone(), c.archived));
            (rel.target_id, child)
        })
        .collect();
    edges.sort_by_key(|(id, _)| *id);
    SliceEvidence(edges)
}

/// F4 (A2, round-3 ruling): the Done write a completion decision authorizes,
/// run on the store re-read UNDER the writer lock. The decision was made on an
/// unlocked snapshot whose slice graph is `evidence`; the locked store must
/// carry the same graph — same edges, and every child with the same status,
/// archive flag and existence — or the decision is stale and nothing is
/// written. A child reopened, unarchived, added, removed or deleted in
/// between all refuse. `flip` runs only on an unchanged graph with no
/// outstanding slice, on the locked copy of spec `id`.
pub(crate) fn done_write_under_lock(
    locked: &mut RequirementsStore,
    id: uuid::Uuid,
    evidence: &SliceEvidence,
    flip: impl FnOnce(&mut Requirement) -> bool,
) -> bool {
    let Some(parent) = locked.requirements.iter().find(|r| r.id == id) else {
        return false;
    };
    if slice_evidence(locked, parent) != *evidence || slice_plan_outstanding(locked, parent) {
        return false;
    }
    match locked.requirements.iter_mut().find(|r| r.id == id) {
        Some(parent) => flip(parent),
        None => false,
    }
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
    let verdict = push_completion_with(storage, spec, subject, tip, between)?;
    match verdict.unconfirmed {
        Some(why) => Err(why),
        None => Ok(verdict.moved),
    }
}

/// The host's completion decision for one confirmed implementer push.
/// `completes` is the veto every later publication step carries (round-7
/// finding 1): true only when the publication subject completes the spec AND
/// the slice graph re-read under the writer lock has no outstanding child.
/// A partial decision makes the PR title a `(Refs SPEC)`, whatever the seat's
/// commit subject says.
// trace:BUG-2696 | ai:claude
#[derive(Debug)]
pub(crate) struct PushCompletion {
    pub(crate) completes: bool,
    pub(crate) moved: bool,
    /// The shared-store push was not confirmed; the decision above stands.
    pub(crate) unconfirmed: Option<anyhow::Error>,
}

/// [`mark_done_after_push_with`], returning the full [`PushCompletion`]. An
/// `Err` means the decision could not be made at all (fail closed: partial).
pub(crate) fn push_completion_with(
    storage: &Storage,
    spec: &str,
    subject: &str,
    tip: &str,
    between: impl FnOnce(),
) -> Result<PushCompletion> {
    let store = storage.load()?;
    let (id, evidence) = match store.get_requirement_unambiguous(spec)? {
        Some(req)
            if subject_completes(Some(&store), subject, req)
                && !slice_plan_outstanding(&store, req) =>
        {
            (req.id, slice_evidence(&store, req))
        }
        _ => {
            // Nothing to write, but a status write the shared store does
            // not have yet is still owed. trace:BUG-2696 | ai:claude
            return Ok(PushCompletion {
                completes: false,
                moved: false,
                unconfirmed: propagate_store_writes(storage.path(), "implementer push → Done")
                    .err(),
            });
        }
    };
    between();
    let mut moved = false;
    let mut graph_unchanged = false;
    storage.update_atomically(|locked| {
        moved = done_write_under_lock(locked, id, &evidence, |r| {
            graph_unchanged = true;
            done_after_push_flip(r, tip)
        });
    })?;
    Ok(PushCompletion {
        completes: graph_unchanged,
        moved,
        unconfirmed: propagate_store_writes(storage.path(), "implementer push → Done").err(),
    })
}

/// Whether `store` positively has a shared remote: `Ok(false)` only when git
/// answered that it is not a repository, or that it has no `origin` (a
/// local-only store). A probe that cannot answer — a spawn failure, a host
/// git refusal, an unreadable config — is `Err`, never "local-only" (A2,
/// round-4 finding 2).
// trace:BUG-2696 | ai:claude
pub(crate) fn store_has_shared_remote(store: &Path) -> Result<bool> {
    use aida_core::git_ops;
    Ok(git_ops::probe_git_repo(store)? && git_ops::probe_remote(store, "origin")?)
}

/// The remote a store push publishes to first: `origin`, or the first
/// declared push hub when this clone redirects origin to its hubs (the same
/// route [`crate::store_push_pull::push_store_confirmed`] takes). `None`
/// when a hub's remote lookup could not answer: the route is unknown.
fn shared_remote(store: &Path) -> Option<String> {
    let unknown = std::cell::Cell::new(false);
    let hubs =
        crate::store_local::redirected_store_hubs(&crate::store_local::push_hubs(store), |r| {
            aida_core::git_ops::probe_remote(store, r).unwrap_or_else(|_| {
                unknown.set(true);
                false
            })
        });
    if unknown.get() {
        return None;
    }
    Some(
        hubs.and_then(|hubs| hubs.into_iter().next())
            .unwrap_or_else(|| "origin".to_string()),
    )
}

/// Round-3 ruling (BUG-2696): "push pending" is DERIVED, never recorded. A
/// git store owes a push while it holds anything its shared remote's
/// tracking ref does not: uncommitted changes, or local commits that ref
/// does not reach. Anything that cannot be established (a failing
/// repository or remote probe, a failing `git status`, a detached HEAD, a
/// missing tracking ref, a failing `rev-list`) counts as owed (A2: fail
/// closed). Only a store git positively reports as not a repository, or as
/// having no `origin`, owes nothing.
pub(crate) fn store_owes_shared_push(store: &Path) -> bool {
    use aida_core::git_ops;
    match store_has_shared_remote(store) {
        Ok(false) => return false,
        Ok(true) => {}
        Err(_) => return true,
    }
    if git_ops::has_changes_checked(store).unwrap_or(true) {
        return true;
    }
    let Ok(branch) = git_ops::current_branch(store) else {
        return true;
    };
    if branch.trim() == "HEAD" || branch.trim().is_empty() {
        return true;
    }
    let Some(remote) = shared_remote(store) else {
        return true;
    };
    let tracking = format!("refs/remotes/{remote}/{}", branch.trim());
    let out = git_ops::host_git_command(store)
        .args([
            "rev-list",
            "--count",
            crate::git_arg_guard::END_OF_OPTIONS,
            &format!("{tracking}..HEAD"),
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse::<u64>()
            .map_or(true, |ahead| ahead > 0),
        _ => true,
    }
}

/// D5 / F6 (BUG-2696): propagate lifecycle status writes to the shared
/// store, and keep owing them until a push is CONFIRMED. Every completion
/// write and every landing scan calls this; whenever the store is ahead of
/// its shared remote ([`store_owes_shared_push`]) — this caller's write,
/// another writer's, or an earlier unconfirmed push, a crashed process's
/// included — it pushes. There is no marker: the debt is the commit graph
/// itself, so an older push succeeding after a newer one failed, or a
/// process exiting between its status commit and its push, cannot hide it.
/// `Err` means the store still holds writes the remote lacks: the caller
/// must report it and must not checkpoint, and the next call retries. A
/// YAML / SQLite store has no shared remote: always `Ok`.
pub(crate) fn propagate_store_writes(store: &Path, reason: &str) -> Result<()> {
    if !store.is_dir() || !store_owes_shared_push(store) {
        return Ok(());
    }
    // The push captures HEAD under the store lock after this caller's write
    // (and after every earlier unconfirmed one), so a confirmed push covers
    // all of them; a newer write past it is its writer's to push, and the
    // next scan's if that fails. A rejection can mean an overlapping writer
    // already published past this commit: settle on the graph, not on the
    // outcome of this particular push.
    match crate::store_push_pull::push_store_confirmed(store, reason) {
        Ok(()) => Ok(()),
        Err(_) if !store_owes_shared_push(store) => Ok(()),
        Err(why) => anyhow::bail!(
            "status recorded locally, but the shared-store push was not confirmed ({why}); \
             it is retried on the next landing scan or implementer push"
        ),
    }
}

/// [`mark_done_after_push_in`] on the project's store, reported on stderr;
/// never fails the caller.
/// Returns whether the host decided the publication completes the spec
/// ([`PushCompletion::completes`]); any failure to decide is `false`.
pub(crate) fn mark_done_after_push_reported(
    project_root: &Path,
    spec: &str,
    subject: &str,
    tip: &str,
) -> bool {
    let shown = aida_core::node_alias::display_spec_id(spec);
    let verdict = crate::lane_recovery::storage_for(project_root)
        .and_then(|storage| push_completion_with(&storage, spec, subject, tip, || {}));
    match verdict {
        Ok(verdict) => {
            if verdict.moved {
                eprintln!("  {shown} → Done (the push completes it for its scope)");
            }
            if let Some(e) = verdict.unconfirmed {
                eprintln!("  warning: could not mark {shown} Done after the push: {e:#}");
            }
            verdict.completes
        }
        Err(e) => {
            eprintln!("  warning: could not mark {shown} Done after the push: {e:#}");
            false
        }
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

    // A plan followup filed at Done (`followup-src:` tagged `--parent` child)
    // is follow-on work, not a slice: it never holds its parent open. An
    // untagged open child beside it still does.
    #[test]
    fn bug_2696_plan_followup_child_is_not_an_outstanding_slice() {
        let mut parent = spec("BUG-1", RequirementStatus::Done);
        let mut followup = spec("TASK-2", RequirementStatus::Approved);
        followup.tags.insert(format!(
            "{}plan-fixture.md",
            crate::plan_followups::FOLLOWUP_SRC_TAG_PREFIX
        ));
        parent.relationships.push(parent_edge(followup.id));
        let store = RequirementsStore {
            requirements: vec![parent.clone(), followup.clone()],
            ..Default::default()
        };
        assert!(!slice_plan_outstanding(&store, &parent));

        let slice = spec("TASK-3", RequirementStatus::Approved);
        parent.relationships.push(parent_edge(slice.id));
        let store = RequirementsStore {
            requirements: vec![parent.clone(), followup, slice],
            ..Default::default()
        };
        assert!(slice_plan_outstanding(&store, &parent));
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

    // F4 round 3: the parent's edges do not change, but an already-linked
    // Completed child is reopened (or an archived open child unarchived)
    // before the writer takes the lock. The locked re-read of the child's
    // status refuses Done.
    #[test]
    fn bug_2696_child_reopened_before_the_lock_refuses_done() {
        type Race = fn(&mut Requirement);
        let reopen: Race = |c| c.status = RequirementStatus::Approved;
        let unarchive: Race = |c| c.archived = false;
        for (label, settled, race) in [
            ("reopened", (RequirementStatus::Completed, false), reopen),
            ("unarchived", (RequirementStatus::Approved, true), unarchive),
        ] {
            let mut parent = spec("STORY-1", RequirementStatus::InProgress);
            let mut child = spec("TASK-2", settled.0.clone());
            child.archived = settled.1;
            parent.relationships.push(parent_edge(child.id));
            let (_dir, storage) = store_with(vec![parent, child]);
            let raced = std::cell::Cell::new(false);
            let moved =
                mark_done_after_push_with(&storage, "STORY-1", "feat: x (STORY-1)", "a", || {
                    raced.set(true);
                    storage
                        .update_atomically(|s| {
                            race(s.get_requirement_by_spec_id_mut("TASK-2").unwrap())
                        })
                        .unwrap();
                })
                .unwrap();
            assert!(raced.get(), "{label}: the seam ran");
            assert!(!moved, "{label}: stale child evidence must refuse Done");
            assert_eq!(
                status(&storage, "STORY-1"),
                RequirementStatus::InProgress,
                "{label}"
            );
        }
        // Control: a settled child with no concurrent change completes.
        let mut parent = spec("STORY-1", RequirementStatus::InProgress);
        let child = spec("TASK-2", RequirementStatus::Completed);
        parent.relationships.push(parent_edge(child.id));
        let (_dir, storage) = store_with(vec![parent, child]);
        assert!(mark_done_after_push_in(&storage, "STORY-1", "feat: x (STORY-1)", "a").unwrap());
        assert_eq!(status(&storage, "STORY-1"), RequirementStatus::Done);
    }

    // The locked check compares the whole child graph, not just the edges.
    #[test]
    fn bug_2696_done_write_under_lock_refuses_any_child_difference() {
        let mut parent = spec("STORY-1", RequirementStatus::InProgress);
        let child = spec("TASK-2", RequirementStatus::Completed);
        parent.relationships.push(parent_edge(child.id));
        let mut store = RequirementsStore::new();
        store.requirements = vec![parent.clone(), child];
        let evidence = slice_evidence(&store, &parent);
        let run = |mut locked: RequirementsStore| {
            done_write_under_lock(&mut locked, parent.id, &evidence, |r| {
                done_after_push_flip(r, "a")
            })
        };
        // A terminal-to-terminal change is still a difference.
        let mut rejected = store.clone();
        rejected.requirements[1].status = RequirementStatus::Rejected;
        assert!(!run(rejected));
        let mut deleted = store.clone();
        deleted.requirements.truncate(1);
        assert!(!run(deleted));
        let mut gone = store.clone();
        gone.requirements.remove(0);
        assert!(!run(gone));
        assert!(run(store));
    }

    // D2: a rework push re-derives the existing PR title's trailer.
    #[test]
    fn bug_2696_rework_title_follows_the_tip_trailer() {
        let mut store = RequirementsStore::new();
        store.requirements = vec![spec("BUG-1", RequirementStatus::Approved)];
        let align = |title: &str, tip: &str| {
            crate::ship_from_seat::aligned_title(Some(&store), title, "BUG-1", tip, true)
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

    // Round 7 (finding 1): the host's partial decision travels into the PR
    // title. A seat commit `feat: slice one (STORY-1)` for a spec with an
    // outstanding child is vetoed: the push reports NOT completing, the spec
    // is not Done, and the title the publication carries is `(Refs STORY-1)`
    // whether the PR title was completing or already a reference; it is never
    // upgraded.
    #[test]
    fn bug_2696_host_veto_carries_into_the_pr_title() {
        let mut parent = spec("STORY-1", RequirementStatus::InProgress);
        let child = spec("TASK-2", RequirementStatus::Approved);
        parent.relationships.push(parent_edge(child.id));
        let (_dir, storage) = store_with(vec![parent, child]);
        let subject = "feat: slice one (STORY-1)";
        let verdict = push_completion_with(&storage, "STORY-1", subject, "a", || {}).unwrap();
        assert!(!verdict.completes && !verdict.moved, "{verdict:?}");
        assert_eq!(status(&storage, "STORY-1"), RequirementStatus::InProgress);
        let store = storage.load().unwrap();
        let align = |title: &str| {
            crate::ship_from_seat::aligned_title(
                Some(&store),
                title,
                "STORY-1",
                subject,
                verdict.completes,
            )
        };
        assert_eq!(
            align("feat: slice one (STORY-1)").as_deref(),
            Some("feat: slice one (Refs STORY-1)")
        );
        assert_eq!(
            align("feat: slice one (Refs STORY-1)"),
            None,
            "never upgraded"
        );
    }

    // Round 7 (finding 1): a declaration that passes on the unlocked snapshot
    // but whose graph changes before the locked write is partial too: the
    // verdict, not just the status write, reports it.
    #[test]
    fn bug_2696_lock_time_graph_change_makes_the_verdict_partial() {
        let parent = spec("STORY-1", RequirementStatus::InProgress);
        let child = spec("TASK-2", RequirementStatus::Approved);
        let child_id = child.id;
        let (_dir, storage) = store_with(vec![parent, child]);
        let verdict = push_completion_with(&storage, "STORY-1", "feat: x (STORY-1)", "a", || {
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
        assert!(!verdict.completes && !verdict.moved, "{verdict:?}");
        // Control: an already-Done spec on a rework round still completes
        // (nothing moves, but the host's decision is completing).
        let (_dir, storage) = store_with(vec![spec("STORY-1", RequirementStatus::Done)]);
        let verdict =
            push_completion_with(&storage, "STORY-1", "feat: x (STORY-1)", "a", || {}).unwrap();
        assert!(verdict.completes && !verdict.moved, "{verdict:?}");
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
