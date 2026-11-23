//! BUG-HOME-048: a completion trailer completes only a spec the PR covers.
//!
//! A `(SPEC-ID)` trailer is written by whoever opened the PR or wrote the
//! branch commits. Before this check, ship granted completion to any trailer
//! id that resolved in the store, so a bare short form (`TASK-139`) that
//! resolved to an unrelated legacy spec (TASK-1-114) was completed by the
//! merge while the spec the branch actually claimed (TASK-HOME-138) stayed
//! open.
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
//!
//! This module is the decision; [`crate::trailer_coverage`] holds the
//! parsing (commit trailers, squash-body lines) and display it calls.
// mandate-gate
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude

use std::collections::BTreeSet;
use std::path::Path;

use aida_core::{RelationshipType, Requirement, RequirementsStore};

use crate::trailer_coverage as parse;

/// The specs one PR may complete through a trailer.
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
    /// every path shares: completion, the gate id lists, `review record`, the
    /// squash body and the rebase/merge branch-commit check.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
    pub(crate) fn uncovered<'s>(
        &self,
        store: &'s RequirementsStore,
        id: &str,
    ) -> Option<&'s Requirement> {
        let req = store.get_requirement_unambiguous(id).ok().flatten()?;
        (!self.covered.contains(&req.id)).then_some(req)
    }

    fn uncovered_phrase(&self, store: &RequirementsStore, id: &str) -> Option<String> {
        let req = self.uncovered(store, id)?;
        Some(parse::uncovered_phrase(id, req, &self.claimed))
    }

    /// Why title trailer `id` may not complete its spec, or `None` when the
    /// spec it resolves to is covered (or `id` does not resolve, which the
    /// caller's own resolution check already refuses).
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

    /// One refusal per completion trailer in `messages` (the full messages
    /// of the branch commits a rebase or merge lands verbatim) that this PR
    /// does not cover. The trailers are read the way the pull-side auto-bump
    /// reads a commit on the default branch.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
    pub(crate) fn commit_refusals(
        &self,
        store: &RequirementsStore,
        messages: &[String],
    ) -> Vec<String> {
        let mut out = Vec::new();
        for message in messages {
            for id in parse::commit_completion_ids(message) {
                if let Some(phrase) = self.uncovered_phrase(store, &id) {
                    let subject = message.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                    let line = format!(
                        "commit \"{}\": trailer {phrase}",
                        parse::short_title(subject)
                    );
                    if !out.contains(&line) {
                        out.push(line);
                    }
                }
            }
        }
        out
    }

    /// `body` with every completion trailer this PR does not cover rewritten
    /// as a `(Refs …)` reference the pull-side scan never completes, plus one
    /// note per rewritten id. Covered trailers are kept.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
    pub(crate) fn neutralize_body(
        &self,
        store: &RequirementsStore,
        body: &str,
    ) -> (String, Vec<String>) {
        let mut notes = Vec::new();
        let out = parse::rewrite_body_trailers(body, |id| {
            let phrase = self.uncovered_phrase(store, id)?;
            let note = format!("squash body trailer {phrase} — kept as a reference only");
            if !notes.contains(&note) {
                notes.push(note);
            }
            Some(())
        });
        (out, notes)
    }
}

/// The trailer coverage for PR `pr` on `branch` — its branch and lease
/// claim, supersede bundles, and review-story covers.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn pr_trailer_coverage(
    project_root: &Path,
    store: &RequirementsStore,
    branch: &str,
    pr: Option<u64>,
) -> Option<TrailerCoverage> {
    let scopes = parse::lease_scopes_for_branch(project_root, branch);
    TrailerCoverage::for_pr(store, branch, &scopes, pr)
}

/// The one filter every gate id list goes through: `ids` without those that
/// resolve to a spec PR `pr` on `branch` does not cover. Every id is kept
/// when the coverage is unknown (no readable store, or no claim). So a
/// verdict, supervised or draft-only policy is never looked up under a
/// foreign bare id (`TASK-139`): the ship approval gate, the mandate cover,
/// `pr ship-batch`, the merge train and the forge merge chokepoint all call
/// this.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn covered_gate_ids(
    project_root: &Path,
    branch: &str,
    pr: Option<u64>,
    ids: Vec<String>,
) -> Vec<String> {
    if ids.is_empty() {
        return ids;
    }
    let Some(store) = crate::load_store_for_lookup(project_root) else {
        return ids;
    };
    match pr_trailer_coverage(project_root, &store, branch, pr) {
        Some(coverage) => ids
            .into_iter()
            .filter(|id| coverage.uncovered(&store, id).is_none())
            .collect(),
        None => ids,
    }
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

/// `aida review record <spec> --pr <n>` against PR `n`'s head `branch`: the
/// refusal when the PR does not cover `spec`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn review_record_refusal(
    project_root: &Path,
    branch: &str,
    pr: u64,
    spec: &str,
) -> Option<String> {
    let store = crate::load_store_for_lookup(project_root)?;
    pr_trailer_coverage(project_root, &store, branch, Some(pr))?.record_refusal(&store, spec, pr)
}

/// The refusal for a rebase/merge ship of PR `pr` whose branch commits
/// (`messages`, landed verbatim on the default branch) carry a trailer the
/// PR does not cover; `None` when every trailer is covered or the coverage
/// is unknown.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn branch_commit_refusal(
    store: &RequirementsStore,
    coverage: Option<&TrailerCoverage>,
    pr: u64,
    messages: &[String],
) -> Option<String> {
    let refusals = coverage?.commit_refusals(store, messages);
    if refusals.is_empty() {
        return None;
    }
    Some(format!(
        "PR-{pr} would land branch commits whose trailers it does not cover, and the \
         default-branch auto-bump would complete those specs:\n  - {}\nReword the commits to \
         name the claimed spec by its full id, or ship as a squash merge (without `--rebase`).",
        refusals.join("\n  - ")
    ))
}

/// The pre-merge trailer check `aida pr ship` runs for every merge method,
/// failing closed: the store and the branch's commit range must both be
/// readable. A rebase or merge whose branch commits carry an uncovered
/// trailer is refused; a squash returns the explicit body ship hands the
/// forge — `squash_body` (the branch's commit messages, or the identity
/// gate's sanitized body) with every uncovered trailer neutralized — so the
/// forge never builds a body of its own from the branch commits.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn ship_trailer_check(
    store: anyhow::Result<RequirementsStore>,
    coverage_for: impl FnOnce(&RequirementsStore) -> Option<TrailerCoverage>,
    method: crate::forge::MergeMethod,
    pr: u64,
    branch_messages: impl FnOnce() -> anyhow::Result<Vec<String>>,
    squash_body: impl FnOnce() -> anyhow::Result<String>,
) -> anyhow::Result<Option<String>> {
    use anyhow::Context;
    let store = store.with_context(|| {
        format!(
            "refused: PR-{pr}: the requirements store could not be read, so the commit \
             trailers this merge would land cannot be checked against what the PR covers"
        )
    })?;
    let coverage = coverage_for(&store);
    if method != crate::forge::MergeMethod::Squash {
        let messages = branch_messages().with_context(|| {
            format!(
                "refused: PR-{pr}: its branch commits could not be read, so their trailers \
                 cannot be checked against what the PR covers — fetch the base branch and retry"
            )
        })?;
        if let Some(refusal) = branch_commit_refusal(&store, coverage.as_ref(), pr, &messages) {
            return Err(crate::gate::ship::ShipRefusal::Review.error(refusal));
        }
        return Ok(None);
    }
    let body = squash_body()?;
    let Some(coverage) = coverage else {
        return Ok(Some(body));
    };
    let (body, notes) = coverage.neutralize_body(&store, &body);
    for note in notes {
        eprintln!("  step 3: {note}");
    }
    Ok(Some(body))
}

#[cfg(test)]
#[path = "tests/trailer_coverage_tests.rs"]
mod tests;
