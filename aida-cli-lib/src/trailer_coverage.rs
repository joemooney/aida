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
                    claimed.push(req.friendly_id());
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

    /// The spec trailer `id` resolves to when this PR does not cover it;
    /// `None` when it is covered or does not resolve. The one coverage test
    /// every path shares: completion, the approval gate, `review record`, and
    /// the rebase/merge branch-commit check.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
    fn uncovered<'s>(&self, store: &'s RequirementsStore, id: &str) -> Option<&'s Requirement> {
        let req = store.get_requirement_unambiguous(id).ok().flatten()?;
        (!self.covered.contains(&req.id)).then_some(req)
    }

    /// `` `id` resolves to X ("title"), which this PR does not cover (it
    /// claims Y) `` for an uncovered `id`.
    fn uncovered_phrase(&self, store: &RequirementsStore, id: &str) -> Option<String> {
        let req = self.uncovered(store, id)?;
        // spec_id first: a legacy spec's agreed id IS the bare trailer, so
        // only its spec id tells the reader it is a different spec.
        let resolved = req.spec_id.as_deref().unwrap_or(id);
        Some(format!(
            "`{id}` resolves to {resolved} (\"{}\"), which this PR does not cover (it claims {})",
            short_title(&req.title),
            self.claimed.join(", "),
        ))
    }

    /// Why trailer `id` may not complete its spec, or `None` when the spec it
    /// resolves to is covered (or `id` does not resolve, which the caller's
    /// own resolution check already refuses).
    // trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
    pub(crate) fn refusal(&self, store: &RequirementsStore, id: &str) -> Option<String> {
        let phrase = self.uncovered_phrase(store, id)?;
        Some(format!(
            "title trailer {phrase} — not completed by this merge; name the claimed spec by \
             its full id"
        ))
    }

    /// Why `aida review record <spec> --pr <n>` may not record a verdict for
    /// `spec`, or `None` when PR `n` covers it.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
    pub(crate) fn record_refusal(
        &self,
        store: &RequirementsStore,
        spec: &str,
        pr: u64,
    ) -> Option<String> {
        let phrase = self.uncovered_phrase(store, spec)?;
        Some(format!(
            "refused: review record for PR-{pr}: {phrase} — nothing was recorded; record the \
             approval under the claimed spec's full id"
        ))
    }

    /// One refusal per trailer id in `subjects` (the subjects of the branch
    /// commits a rebase or merge lands verbatim) that this PR does not cover.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
    pub(crate) fn commit_refusals(
        &self,
        store: &RequirementsStore,
        subjects: &[String],
    ) -> Vec<String> {
        let mut out = Vec::new();
        for subject in subjects {
            for id in crate::pr_ship::extract_trailing_spec_ids_from_subject(subject) {
                if let Some(phrase) = self.uncovered_phrase(store, &id) {
                    let line = format!("commit \"{}\": trailer {phrase}", short_title(subject));
                    if !out.contains(&line) {
                        out.push(line);
                    }
                }
            }
        }
        out
    }
}

/// The ids in `title` the ship approval gate looks verdicts up under: every id
/// when the PR's coverage is unknown (no store, or no claim), otherwise only
/// the covered ones, so an approval recorded under a foreign bare id
/// (`TASK-139`) is neither demanded nor accepted.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn covered_title_ids(
    project_root: &std::path::Path,
    branch: &str,
    pr: Option<u64>,
    title: &str,
) -> Vec<String> {
    let ids = crate::pr_ship::extract_spec_ids_from_text(title);
    if ids.is_empty() {
        return ids;
    }
    let Some(store) = crate::load_store_for_lookup(project_root) else {
        return ids;
    };
    match crate::pr_cmd::pr_trailer_coverage(project_root, &store, branch, pr) {
        Some(coverage) => ids
            .into_iter()
            .filter(|id| coverage.uncovered(&store, id).is_none())
            .collect(),
        None => ids,
    }
}

/// `aida review record <spec> --pr <n>` against PR `n`'s head `branch`: the
/// refusal when the PR does not cover `spec`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn review_record_refusal(
    project_root: &std::path::Path,
    branch: &str,
    pr: u64,
    spec: &str,
) -> Option<String> {
    let store = crate::load_store_for_lookup(project_root)?;
    crate::pr_cmd::pr_trailer_coverage(project_root, &store, branch, Some(pr))?
        .record_refusal(&store, spec, pr)
}

/// The refusals for the branch commits a rebase or merge of PR `pr` on
/// `branch` would land verbatim on the default branch: each one names a
/// trailer the PR does not cover, which the pull-side auto-bump would
/// otherwise complete. Empty when coverage is unknown or every trailer is
/// covered.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn branch_commit_refusals(
    project_root: &std::path::Path,
    store: &RequirementsStore,
    branch: &str,
    pr: u64,
    subjects: &[String],
) -> Vec<String> {
    crate::pr_cmd::pr_trailer_coverage(project_root, store, branch, Some(pr))
        .map(|c| c.commit_refusals(store, subjects))
        .unwrap_or_default()
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
