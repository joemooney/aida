//! BUG-HOME-048: a PR-title trailer completes only a spec the PR covers.
//!
//! A `(SPEC-ID)` title trailer is written by whoever opened the PR. Before
//! this check, ship granted completion to any trailer id that resolved in the
//! store, so a bare short form (`TASK-139`) that resolved to an unrelated
//! legacy spec (TASK-1-114) was completed by the merge while the spec the
//! branch actually claimed (TASK-HOME-138) stayed open.
//!
//! The covered set is what the PR's branch or lease actually claims, plus the
//! specs bundled into those claims:
//!
//! - the spec ids in the branch name and in the scope of any lease on that
//!   branch (the claim);
//! - specs linked to a claimed spec by a recorded supersede
//!   (`superseded-by` / `supersedes`, either direction);
//! - the specs the PR's queued review story lists under `## Covers` (its
//!   `implements` relationships).
//!
//! When nothing is claimed (a hand-named branch with no lease), the set is
//! unknown and no coverage check applies: refusing every trailer there would
//! stop ordinary human PRs from completing anything.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude

use std::collections::BTreeSet;

use aida_core::{RelationshipType, Requirement, RequirementsStore};

/// The specs one PR may complete through its title trailer.
#[derive(Debug, Clone)]
pub(crate) struct TrailerCoverage {
    /// Store ids of every covered spec.
    covered: BTreeSet<uuid::Uuid>,
    /// Display ids of the claimed specs, for the refusal message.
    claimed: Vec<String>,
}

impl TrailerCoverage {
    /// The coverage for a PR on `branch`. `lease_scopes` are the `--owns`
    /// scopes of leases on that branch; `pr` selects the review story whose
    /// covers count. `None` when neither the branch nor a lease claims a spec.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
    pub(crate) fn for_pr(
        store: &RequirementsStore,
        branch: &str,
        lease_scopes: &[String],
        pr: Option<u64>,
    ) -> Option<Self> {
        let mut claim_ids = crate::pr_ship::extract_spec_ids_from_text(branch);
        for scope in lease_scopes {
            claim_ids.extend(crate::pr_ship::extract_spec_ids_from_text(scope));
        }
        let mut covered = BTreeSet::new();
        let mut claimed = Vec::new();
        for id in &claim_ids {
            if let Ok(Some(req)) = store.get_requirement_unambiguous(id) {
                if covered.insert(req.id) {
                    claimed.push(display(req));
                }
            }
        }
        if covered.is_empty() {
            return None;
        }
        // Bundled by a recorded supersede, in either direction, one hop.
        let claimed_ids: Vec<uuid::Uuid> = covered.iter().copied().collect();
        for req in &store.requirements {
            let claimed_req = claimed_ids.contains(&req.id);
            for rel in &req.relationships {
                if !matches!(
                    rel.rel_type,
                    RelationshipType::Supersedes | RelationshipType::SupersededBy
                ) {
                    continue;
                }
                if claimed_req {
                    covered.insert(rel.target_id);
                } else if claimed_ids.contains(&rel.target_id) {
                    covered.insert(req.id);
                }
            }
        }
        // The PR's queued review story's `## Covers` list.
        if let Some(n) = pr {
            for review in store
                .requirements
                .iter()
                .filter(|r| crate::pr_lookup::parse_review_story_pr_number(&r.title) == Some(n))
            {
                for rel in &review.relationships {
                    if matches!(
                        &rel.rel_type,
                        RelationshipType::Custom(name) if name.eq_ignore_ascii_case("implements")
                    ) {
                        covered.insert(rel.target_id);
                    }
                }
            }
        }
        Some(TrailerCoverage { covered, claimed })
    }

    /// Why trailer `id` may not complete its spec, or `None` when the spec it
    /// resolves to is covered (or `id` does not resolve, which the caller's
    /// own resolution check already refuses).
    // trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
    pub(crate) fn refusal(&self, store: &RequirementsStore, id: &str) -> Option<String> {
        let req = store.get_requirement_unambiguous(id).ok().flatten()?;
        if self.covered.contains(&req.id) {
            return None;
        }
        let resolved = req.spec_id.as_deref().unwrap_or(id);
        Some(format!(
            "title trailer `{id}` resolves to {resolved} (\"{}\"), which this PR does not \
             cover (it claims {}) — not completed by this merge; name the claimed spec by \
             its full id",
            short_title(&req.title),
            self.claimed.join(", "),
        ))
    }
}

/// The `--owns` scopes of leases on `branch` in `project_root`'s main
/// worktree.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn lease_scopes_for_branch(project_root: &std::path::Path, branch: &str) -> Vec<String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Vec::new();
    }
    let root = crate::worktree_gc::main_worktree_root_from(project_root);
    crate::leases::list_leases(&root)
        .into_iter()
        .filter(|l| l.branch == branch)
        .map(|l| l.scope)
        .collect()
}

/// Store-backed trailer resolution with the coverage check: `id` carries
/// completion authority only when it resolves unambiguously and, when the
/// coverage is known, to a covered spec. Each refusal is pushed to `notes`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn admits(
    store: &RequirementsStore,
    coverage: Option<&TrailerCoverage>,
    id: &str,
    notes: &std::cell::RefCell<Vec<String>>,
) -> bool {
    if !matches!(store.get_requirement_unambiguous(id), Ok(Some(_))) {
        return false;
    }
    match coverage.and_then(|c| c.refusal(store, id)) {
        Some(why) => {
            let mut notes = notes.borrow_mut();
            if !notes.contains(&why) {
                notes.push(why);
            }
            false
        }
        None => true,
    }
}

fn display(req: &Requirement) -> String {
    req.spec_id
        .as_deref()
        .map(aida_core::node_alias::display_spec_id)
        .unwrap_or_else(|| req.display_id())
}

fn short_title(title: &str) -> String {
    const MAX: usize = 60;
    if title.chars().count() <= MAX {
        title.to_string()
    } else {
        let cut: String = title.chars().take(MAX).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
#[path = "tests/trailer_coverage_tests.rs"]
mod tests;
