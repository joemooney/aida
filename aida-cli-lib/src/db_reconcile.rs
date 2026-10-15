//! Done-to-completed auto-bump, closure holds, stale-review flips and `aida db reconcile-status` (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// TASK-78: project-wide opt-out for the post-pull merge-gate. Defaults to
/// on; set `AIDA_AUTO_MERGE_GATE=false` (or `0`, `no`, `off`) to disable.
/// Mirrors the env-var convention used for `AIDA_BG_FETCH`.
/// trace:TASK-78 | ai:claude
pub(crate) fn auto_merge_gate_enabled() -> bool {
    match std::env::var("AIDA_AUTO_MERGE_GATE") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// STORY-86: project-wide opt-out for the post-pull `Done` → `Completed`
/// auto-bump. Defaults to on; set `AIDA_AUTO_BUMP=false` (or `0`, `no`,
/// `off`) to disable. Mirrors `auto_merge_gate_enabled` directly above.
/// trace:STORY-86 | ai:claude
pub(crate) fn auto_bump_enabled() -> bool {
    match std::env::var("AIDA_AUTO_BUMP") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// BUG-328: a commit on the default branch is authoritative evidence that
/// approved/planned/in-flight/done work shipped. Draft preserves the approval
/// signal; terminal statuses stay terminal. BUG-405 extends eligibility to a
/// shelved `NeedsAttention` whose PR a later session merged.
///
/// TASK-740: single-sourced in the lifecycle model as the merge-evidence
/// GitEvent guard — the `aida pull` scanner and the `aida db reconcile-status`
/// replay both ask the model so the eligibility set can't drift from the
/// declared `Done → Completed` transition (the BUG-328 / BUG-405 rationale now
/// lives on `lifecycle::git_merge_completes`).
/// trace:TASK-740 | ai:claude trace:BUG-328 trace:BUG-405
pub(crate) fn auto_bump_eligible_status(status: &RequirementStatus) -> bool {
    aida_core::lifecycle::git_merge_completes(aida_core::lifecycle::State::from_status(status))
}

/// BUG-1506: the work-item types eligible for the Draft→Done landing bump —
/// deliverable units of implementation work that a trailered commit can
/// plausibly "land". Excludes `Epic` (a read-only rollup of its children,
/// never hand-set — `BUG-626`), the ADR/knowledge-graph family (`Decision`,
/// `Principle`, `Vision`, `Constraint`, `Term`, `Doc` — narrative/governance
/// artifacts with their own stateful lifecycles, not code-shipped work), and
/// the organizational/meta/agile-container types (`Folder`, `Meta`,
/// `Sprint`). Without this filter a Draft `ADR-N` merely referenced by a
/// trailered commit (e.g. the commit that implements the decision, not the
/// decision itself) would be falsely bumped to Done alongside the real work.
// trace:BUG-1506 | ai:claude
pub(crate) fn is_auto_bump_work_type(req_type: &RequirementType) -> bool {
    matches!(
        req_type,
        RequirementType::Functional
            | RequirementType::NonFunctional
            | RequirementType::System
            | RequirementType::User
            | RequirementType::ChangeRequest
            | RequirementType::Bug
            | RequirementType::Story
            | RequirementType::Task
            | RequirementType::Spike
    )
}

/// TASK-1446: true when `candidate` is the reopen-marker sha itself or an
/// ancestor of it on the code repo — i.e. the commit was already on the
/// default branch at (or before) the moment the spec was deliberately
/// reopened to `Draft`, so it is stale evidence that must NOT re-land the
/// spec at `Done`. A DIFFERENT commit that lands *after* the reopen (not an
/// ancestor of `reopen_sha`) is fresh evidence and still lands it. Best-effort:
/// an unreadable/missing repo defaults to "not stale" (the pre-existing
/// draft-landing behavior) rather than silently swallowing a legitimate flip.
// trace:TASK-1446 | ai:claude
pub(crate) fn sha_at_or_before_reopen(
    project_root: &std::path::Path,
    candidate: &str,
    reopen_sha: &str,
) -> bool {
    if candidate.eq_ignore_ascii_case(reopen_sha) {
        return true;
    }
    std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "merge-base",
            "--is-ancestor",
            git_arg_guard::END_OF_OPTIONS,
            candidate,
            reopen_sha,
        ]) // trace:BUG-1622 | ai:claude
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Reopening fences off all already-landed evidence, including a different
/// older SHA and paths that never stamped completion_sha (closure holds).
// trace:TASK-1600 | ai:codex
pub(crate) fn auto_bump_evidence_is_stale(
    project_root: &std::path::Path,
    req: &Requirement,
    sha: &str,
) -> bool {
    // trace:TASK-1338 | ai:codex
    // Older human reopens predate reopened_at_sha. Ignore automated status
    // rewrites when finding the latest deliberate decision: a replayed bump
    // must not erase the decision it incorrectly overwrote.
    if human_reopen_after_evidence(project_root, req, sha) {
        return true;
    }
    let Some(info) = req.implementation_info.as_ref() else {
        return false;
    };
    if !sha.is_empty() && info.completion_sha.as_deref() == Some(sha) {
        return true;
    }
    info.reopened_at_sha.as_deref().is_some_and(|reopen_sha| {
        sha.is_empty() || sha_at_or_before_reopen(project_root, sha, reopen_sha)
    })
}

/// Legacy reopen history fences old evidence even without a SHA marker.
/// A subsequent deliberate status decision or genuinely later commit permits
/// progress; automated replay never supersedes a human reopen.
// trace:TASK-1338 | ai:codex
fn human_reopen_after_evidence(
    project_root: &std::path::Path,
    req: &Requirement,
    sha: &str,
) -> bool {
    let latest = req
        .history
        .iter()
        .filter(|entry| !aida_core::conflict::is_automated_status_author(&entry.author))
        .filter_map(|entry| {
            entry
                .changes
                .iter()
                .find(|change| change.field_name == "status")
                .map(|change| (entry.timestamp, change))
        })
        .max_by_key(|(timestamp, _)| *timestamp);
    let Some((reopened_at, change)) = latest else {
        return false;
    };
    if !matches!(
        change.old_value.to_ascii_lowercase().as_str(),
        "done" | "completed"
    ) || !change.new_value.eq_ignore_ascii_case("approved")
    {
        return false;
    }
    if !crate::git_arg_guard::is_hex_sha(sha) {
        return true;
    }
    let committed_at = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "show",
            "-s",
            "--format=%ct",
            crate::git_arg_guard::END_OF_OPTIONS,
            sha,
        ])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| {
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse::<i64>()
                .ok()
        });
    // Unknown evidence cannot override a recorded deliberate reopen.
    committed_at.is_none_or(|timestamp| timestamp <= reopened_at.timestamp())
}

// Filter before closure projection so stale evidence cannot move a reopened
// spec back to Done or release its dependents during this pass.
// trace:TASK-1600 | ai:codex
fn retain_fresh_auto_bump_flips(
    project_root: &std::path::Path,
    store: &aida_core::RequirementsStore,
    flips: &mut Vec<AutoBumpFlip>,
) {
    flips.retain(|flip| {
        trailer_spec_or_skip(store, &flip.spec_id, &flip.sha)
            .is_some_and(|r| !auto_bump_evidence_is_stale(project_root, r, &flip.sha))
    });
}

// trace:TASK-1600 | ai:codex
fn retain_fresh_stale_review_flips(
    project_root: &std::path::Path,
    store: &aida_core::RequirementsStore,
    flips: &mut Vec<(String, String, u64, RequirementStatus)>,
) {
    flips.retain(|(id, sha, ..)| {
        trailer_spec_or_skip(store, id, sha)
            .is_some_and(|r| !auto_bump_evidence_is_stale(project_root, r, sha))
    });
}

/// Resolve a `(SPEC-ID)` commit-trailer id for the pull auto-bump. An id
/// that names more than one requirement is SKIPPED with a printed warning
/// (the rest of the pull proceeds): bumping a guessed spec is how a trailer
/// for the real spec completed a rejected fixture instead.
// trace:TASK-1468 | ai:claude
pub(crate) fn trailer_spec_or_skip<'s>(
    store: &'s aida_core::RequirementsStore,
    spec_id: &str,
    sha: &str,
) -> Option<&'s Requirement> {
    match store.get_requirement_unambiguous(spec_id) {
        Ok(found) => found,
        Err(e) => {
            let short = sha.get(..7).unwrap_or(sha);
            eprintln!(
                "{} skipping commit trailer `{}` (commit {}): {}",
                "Warning:".yellow().bold(),
                spec_id,
                short,
                e.to_string().lines().next().unwrap_or_default()
            );
            None
        }
    }
}

/// BUG-1506: find Draft specs among `candidates` (spec_id → first-seen commit
/// sha, the same map both the live pull-time scan and `reconcile-status`
/// already build from `(SPEC-ID)` trailers) whose commit is already on the
/// default branch. Shared by both call sites so the eligibility check can't
/// drift between them — mirrors `auto_bump_eligible_status`'s role for the
/// existing Completed-bump path. Honors an optional `spec` filter the same
/// way the reconcile-status replay narrows its own candidate scan. Restricted
/// to `is_auto_bump_work_type` — see that function's doc comment for why
/// non-work types (epics, ADRs, docs, …) must never be silently landed here.
///
/// TASK-1446: also honors the reopen guard — a spec whose
/// `implementation_info.reopened_at_sha` is set was deliberately taken back
/// to Draft after landing once already; a candidate commit at or before that
/// sha is the SAME old evidence that already fired, and must not re-land it.
// trace:BUG-1506 | ai:claude trace:TASK-1446 | ai:claude
pub(crate) fn collect_draft_landed_candidates(
    project_root: &std::path::Path,
    store: &aida_core::RequirementsStore,
    candidates: &std::collections::BTreeMap<String, String>,
    spec: Option<&str>,
) -> Vec<(String, String)> {
    candidates
        .iter()
        .filter(|(spec_id, _)| match spec {
            Some(target) => spec_id.eq_ignore_ascii_case(target),
            None => true,
        })
        .filter_map(|(spec_id, sha)| {
            let req = trailer_spec_or_skip(store, spec_id, sha)?;
            if !is_auto_bump_work_type(&req.req_type) {
                return None;
            }
            if !aida_core::lifecycle::git_merge_lands_draft_at_done(
                aida_core::lifecycle::State::from_status(&req.status),
            ) {
                return None;
            }
            if auto_bump_evidence_is_stale(project_root, req, sha) {
                return None;
            }
            Some((spec_id.clone(), sha.clone()))
        })
        .collect()
}

/// BUG-1506: mutate one already-fetched requirement in place for the
/// `Draft → Done` landing flip — the shared body for both write paths below,
/// mirroring how `apply_auto_bump_flip` is shared between the git-canonical
/// and legacy writers for the Completed bump. Records the flip in the spec's
/// history plus an audit comment naming the landed commit, so `aida show`
/// explains why a spec skipped straight from Draft to Done instead of
/// silently rewriting it. Caller is responsible for re-checking `r.status`
/// is still `Draft` immediately before calling this (a concurrent edit may
/// have already moved it on).
// trace:BUG-1506 | ai:claude
pub(crate) fn apply_draft_landed_flip(
    r: &mut aida_core::Requirement,
    sha: &str,
    now: chrono::DateTime<chrono::Utc>,
) {
    let prior_status = r.status.clone();
    r.set_status_from_str("Done");
    // BUG-1637: through the one shared history helper. trace:BUG-1637 | ai:claude
    aida_core::conflict::record_status_transition(
        r,
        aida_core::conflict::AUTO_BUMP_AUTHOR,
        &prior_status,
    );
    r.modified_at = now;
    let short = if sha.len() >= 7 { &sha[..7] } else { sha };
    r.add_comment(aida_core::Comment::new(
        "aida-auto-bump".to_string(),
        format!(
            "Flipped Draft → Done: a trailered commit ({}) referencing this spec \
             is already on the default branch, though it skipped the intermediate \
             approval states. Confirm it, then move to Completed.",
            short
        ),
    ));
}

/// BUG-1506: write the `Draft → Done` flip for each `(spec_id, sha)` pair
/// `collect_draft_landed_candidates` found. Returns the spec_ids actually
/// confirmed Done after the write.
///
/// On the git-canonical store this MUST be a targeted per-spec write — the
/// same `get_requirement_by_spec_id` + `update_requirement` path the
/// Done→Completed bump uses (TASK-1161 / BUG-634) — never the full-store
/// `Storage::update_atomically`. On a `GitBackend`, `update_atomically`
/// loads the ENTIRE store, applies the closure, and SAVES the entire
/// snapshot back — and that save deletes any spec on disk that is missing
/// from the in-memory snapshot. A spec added concurrently (a drain
/// follow-up, or `aida add` from another session) between the load and the
/// save is therefore silently DELETED by this function's own write, not
/// merely left unbumped. Reading and writing one spec at a time closes that
/// window entirely — there is no snapshot for a concurrent spec to be
/// missing from.
// trace:BUG-1506 | ai:claude
pub(crate) fn apply_draft_to_done_bumps(
    project_root: &std::path::Path,
    storage: &Storage,
    draft_candidates: &[(String, String)],
) -> Result<Vec<(String, String)>> {
    if draft_candidates.is_empty() {
        return Ok(Vec::new());
    }
    let now = chrono::Utc::now();
    let store_path = storage.path();
    if store_path.is_dir() {
        // Git-canonical store: targeted per-spec writes only. trace:BUG-1506 | ai:claude
        use aida_core::db::DatabaseBackend;
        let backend = aida_core::db::GitBackend::new(store_path)?;
        let mut confirmed = Vec::new();
        for (spec_id, sha) in draft_candidates {
            // Plain lookup: a checked one on a bare GitBackend reloads the whole
            // store per spec. Trailer ids were vetted by trailer_spec_or_skip.
            // trace:TASK-1468 | ai:claude
            let Some(mut r) = backend.get_requirement_by_spec_id(spec_id)? else {
                continue;
            };
            if !matches!(r.status, RequirementStatus::Draft)
                || auto_bump_evidence_is_stale(project_root, &r, sha)
            {
                continue;
            }
            apply_draft_landed_flip(&mut r, sha, now);
            backend.update_requirement(&r)?;
            confirmed.push((spec_id.clone(), sha.clone()));
        }
        return Ok(confirmed);
    }

    // Legacy YAML/SQLite store: this store type has no targeted per-spec
    // write path, so the atomic full-store update is its normal write —
    // not a shortcut around one, the way it would be on `GitBackend`.
    let for_write = draft_candidates.to_vec();
    storage.update_atomically(|s| {
        for (spec_id, sha) in &for_write {
            if let Some(r) = s.requirements.iter_mut().find(|r| {
                r.spec_id.as_deref() == Some(spec_id.as_str())
                    || r.agreed_id.as_deref() == Some(spec_id.as_str())
            }) {
                if !matches!(r.status, RequirementStatus::Draft)
                    || auto_bump_evidence_is_stale(project_root, &r, sha)
                {
                    continue;
                }
                apply_draft_landed_flip(r, sha, now);
            }
        }
    })?;
    let after = storage.load()?;
    Ok(draft_candidates
        .iter()
        .filter(|(spec_id, _)| {
            after
                .get_requirement_by_spec_id(spec_id)
                .map(|r| matches!(r.status, RequirementStatus::Done))
                .unwrap_or(false)
        })
        .cloned()
        .collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AutoBumpFlip {
    pub(crate) spec_id: String,
    pub(crate) sha: String,
    pub(crate) prior_status: RequirementStatus,
}

/// BUG-1454: return the candidate specs that still have an open GitHub PR.
///
/// A completion trailer proves that *some* work landed, but an open PR naming
/// the same spec is stronger evidence that the spec has another deliverable in
/// flight. Keep those specs at Done until the final PR closes. One search is
/// made per candidate because GitHub's PR search covers title, body, and
/// comments without downloading every open PR in a large repository.
///
/// `None` means the GitHub lookup was unavailable or failed. Callers
/// default-to-preserve in that ambiguous case; hiding unfinished work is more
/// damaging than leaving shipped work visible for another pass.
///
/// Compatibility wrapper over the checked lookup in [`reconcile_evidence`],
/// which owns the forge query and response validation (a non-array or
/// malformed response is unavailable, never empty). The pure-git positive
/// and GitLab-is-unknown rules are documented there.
// trace:BUG-1454 | ai:codex
// trace:TASK-1335 | ai:claude
pub(crate) fn specs_with_open_prs(
    project_root: &std::path::Path,
    spec_ids: impl IntoIterator<Item = String>,
) -> Option<std::collections::BTreeMap<String, u64>> {
    let forge = Ok(forge::resolve_forge_kind(project_root));
    reconcile_evidence::open_changes_or_none(reconcile_evidence::check_open_changes(
        project_root,
        &forge,
        spec_ids,
        true,
    ))
}

/// BUG-2134: the line `aida pull` prints when the BUG-1454 open-PR guard
/// withholds a merge-driven completion. It names the status the spec actually
/// keeps (it used to say "stays Done" for every status). A Needs Attention
/// spec gets a triage prompt as well: this merge commit is now behind the scan
/// window, so no later pull retries it, and the spec would otherwise sit
/// shelved with its work merged. The scan window makes the prompt print once.
// trace:BUG-2134 | ai:claude
pub(crate) fn open_pr_withheld_message(
    spec_id: &str,
    prior_status: &RequirementStatus,
    open_pr: u64,
) -> String {
    let glyph = "↷".yellow();
    if matches!(prior_status, RequirementStatus::NeedsAttention) {
        format!(
            "{glyph} merged work references needs-attention spec {spec_id}: triage it \
             (not completed — open PR #{open_pr} still references it)"
        )
    } else {
        format!("{glyph} {spec_id} stays {prior_status} — open PR #{open_pr} still references it")
    }
}

/// BUG-2134: does a commit message's BODY (everything after the subject line)
/// carry a [`crate::pr_ship::NO_COMPLETE_BODY_MARKERS`] line? A squash merge
/// copies the PR body into the commit body, so this is where a non-completing
/// slice declaration survives a merge done outside `aida pr ship`.
// trace:BUG-2134 | ai:claude
pub(crate) fn commit_declares_non_completing(message: &str) -> bool {
    let mut lines = message.lines().skip_while(|l| l.trim().is_empty());
    lines.next();
    let body: Vec<&str> = lines.collect();
    crate::pr_ship::pr_body_declares_non_completing(&body.join("\n"))
}

impl AutoBumpFlip {
    pub(crate) fn new(spec_id: String, sha: String, prior_status: RequirementStatus) -> Self {
        Self {
            spec_id,
            sha,
            prior_status,
        }
    }
}

// STORY-1418: the emission lives in the into-Completed seam.
pub(crate) use completion::emit_spec_completed;

pub(crate) fn completing_ref_label(project_root: &std::path::Path, sha: &str) -> String {
    if sha.is_empty() {
        return "completion evidence".to_string();
    }
    let short = if sha.len() >= 7 { &sha[..7] } else { sha };
    let subject = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "show",
            "-s",
            "--format=%s",
            git_arg_guard::END_OF_OPTIONS,
            sha,
            "--",
        ]) // trace:BUG-1622 | ai:claude
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    match extract_pr_number_from_commit_subject(&subject) {
        Some(pr_n) => format!("commit {short} / PR #{pr_n}"),
        None => format!("commit {short}"),
    }
}

pub(crate) fn is_auto_complete_failure_bug_about(
    req: &aida_core::Requirement,
    completed_spec: &str,
) -> bool {
    if !matches!(req.req_type, aida_core::RequirementType::Bug) {
        return false;
    }
    if !matches!(req.status, RequirementStatus::Draft) {
        return false;
    }
    let has_auto_marker = req.tags.iter().any(|t| t == "auto-complete")
        && req.tags.iter().any(|t| t == "auto-drafted")
        && req.tags.iter().any(|t| t.starts_with("failure-"));
    if !has_auto_marker {
        return false;
    }
    req.title.starts_with("auto-complete failure:")
        && req
            .title
            .trim_end()
            .ends_with(&format!(" on {completed_spec}"))
}

/// BUG-1768: the parent spec an auto-drafted phase-failure finding is about,
/// read back out of the finding's own title. The flip-driven sweep below learns
/// the parent from the set of specs it just flipped; a finding whose parent
/// completed by some other route has no such source, and the title is the only
/// place the parent is recorded.
///
/// `None` unless the finding passes `is_auto_complete_failure_bug_about` against
/// the id this reads out, so that predicate stays the sole admission test and a
/// hand-filed BUG that merely mentions a spec is never admitted here.
// trace:BUG-1768 | ai:claude
pub(crate) fn auto_complete_failure_bug_parent_spec(
    req: &aida_core::Requirement,
) -> Option<String> {
    let parent = req
        .title
        .trim_end()
        .rsplit(" on ")
        .next()?
        .trim()
        .to_string();
    if parent.is_empty() || !is_auto_complete_failure_bug_about(req, &parent) {
        return None;
    }
    Some(parent)
}

/// BUG-1768: is there a Draft auto-drafted phase-failure finding whose parent is
/// already `Completed`? Deliberately NOT
/// `auto_resolve_failure_bugs_for_completed_specs(store, &[], ..)`: the guards
/// below only need to know *whether* such work exists, and that function also
/// builds a completion label per hit, which shells out to `git show`. Asking the
/// question this way short-circuits on the first hit and never runs git, so the
/// overwhelmingly common answer (none) costs one cheap scan.
// trace:BUG-1768 | ai:claude
pub(crate) fn has_orphaned_failure_finding(store: &aida_core::RequirementsStore) -> bool {
    store.requirements.iter().any(|req| {
        auto_complete_failure_bug_parent_spec(req).is_some_and(|parent| {
            store.requirements.iter().any(|p| {
                (p.spec_id.as_deref() == Some(parent.as_str())
                    || p.agreed_id.as_deref() == Some(parent.as_str()))
                    && matches!(p.status, RequirementStatus::Completed)
            })
        })
    })
}

pub(crate) fn reject_resolved_auto_complete_failure_bug(
    req: &mut aida_core::Requirement,
    completed_spec: &str,
    completion_ref: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let Some(finding_id) = req.spec_id.as_deref().or(req.agreed_id.as_deref()) else {
        return false;
    };
    let finding_id = finding_id.to_string();
    if !is_auto_complete_failure_bug_about(req, completed_spec) {
        return false;
    }
    let prior = req.status.clone();
    req.set_status_from_str("Rejected");
    // BUG-1637: through the one shared history helper. trace:BUG-1637 | ai:claude
    aida_core::conflict::record_status_transition(
        req,
        aida_core::conflict::AUTO_BUMP_AUTHOR,
        &prior,
    );
    req.modified_at = now;
    req.add_comment(aida_core::Comment::new(
        "aida-auto-bump".to_string(),
        format!(
            "Auto-resolved: {completed_spec} reached Completed via {completion_ref}; \
             rejecting auto-drafted phase-failure finding {finding_id}."
        ),
    ));
    true
}

// trace:TASK-1192 | ai:codex
pub(crate) fn auto_resolve_failure_bugs_for_completed_specs(
    store: &aida_core::RequirementsStore,
    completed: &[(String, String)],
    project_root: &std::path::Path,
) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (completed_spec, sha) in completed {
        let completion_ref = completing_ref_label(project_root, sha);
        for req in &store.requirements {
            if !is_auto_complete_failure_bug_about(req, completed_spec) {
                continue;
            }
            let Some(finding_id) = req.spec_id.as_deref().or(req.agreed_id.as_deref()) else {
                continue;
            };
            out.push((
                finding_id.to_string(),
                completed_spec.clone(),
                completion_ref.clone(),
            ));
        }
    }

    // BUG-1768: the loop above only sees the specs THIS pass flipped. A parent
    // that reached Completed by any other route -- a hand
    // `aida edit --status completed`, or any path outside this pass's confirmed
    // set -- left its auto-drafted finding in Draft forever, where it surfaced
    // as human work in the advisor groom bucket (BUG-1821 on BUG-1817 sat there
    // 20 hours after its parent completed). Sweep the findings themselves: each
    // names its parent, so ask the store whether that parent is already done.
    // trace:BUG-1768 | ai:claude
    for req in &store.requirements {
        let Some(parent) = auto_complete_failure_bug_parent_spec(req) else {
            continue;
        };
        // Already emitted above, against the real completing commit.
        if completed.iter().any(|(spec_id, _)| *spec_id == parent) {
            continue;
        }
        let Some(parent_req) = store.requirements.iter().find(|p| {
            p.spec_id.as_deref() == Some(parent.as_str())
                || p.agreed_id.as_deref() == Some(parent.as_str())
        }) else {
            continue;
        };
        if !matches!(parent_req.status, RequirementStatus::Completed) {
            continue;
        }
        let Some(finding_id) = req.spec_id.as_deref().or(req.agreed_id.as_deref()) else {
            continue;
        };
        // The parent's own auto-bump stamp when it has one; `completing_ref_label`
        // degrades to "completion evidence" for a hand flip, which leaves none.
        let parent_sha = parent_req
            .implementation_info
            .as_ref()
            .and_then(|i| i.completion_sha.clone())
            .unwrap_or_default();
        out.push((
            finding_id.to_string(),
            parent,
            completing_ref_label(project_root, &parent_sha),
        ));
    }

    out.sort();
    out.dedup();
    out
}

/// BUG-219 / TASK-246: collect review stories stranded short of
/// `Completed` because their PR merged before the review lifecycle ever
/// finished. `/aida-pr` auto-queues a `Review PR-N` story at `Approved`
/// (ready-to-work); three ways it never reaches `Completed` on its own:
///
/// - **Approved** — a reviewer session was never spawned at all: the user
///   self-merged the PR, or the `--auto-complete` orchestrator skipped
///   phase 3. The story sits at `Approved` forever, a stale entry in the
///   reviewer queue (BUG-219's observed case).
/// - **InProgress** — a reviewer asked for fixups, then the PR self-merged
///   instead of a fresh `/aida-review` pass (the TASK-246 case).
/// - **Draft** — the story's own tracking record never even reached
///   Approved before the PR it tracks landed (a failed queueing step,
///   BUG-1230's shape). Decided in BUG-1560 rather than left excluded by
///   omission: every candidate reaching this function is against a PR
///   whose merge commit is already confirmed present in `pr_to_sha`, so
///   there is no "is it still open?" ambiguity for Draft to inherit here —
///   that question belongs to the forge-lookup stranded-review-PR sweep
///   (`collect_stranded_review_pr_resolutions`, BUG-1543), which is a
///   different sweep touching the same population. Same underlying fact as
///   Approved/InProgress (review lifecycle never finished), caught one
///   stage earlier.
///
/// Either way the `(#N)` merge commit landing on the default branch is the
/// authoritative "review is over" signal. For each merged PR in
/// `pr_to_sha` this returns the `(review_spec_id, merge_sha, pr_number,
/// prior_status)` of any review story still at `Draft`/`Approved`/`InProgress`,
/// skipping specs already claimed by the caller's `flips` list (Done specs
/// / Done review stories the commit-subject + BUG-102 scan handles).
/// trace:BUG-219 | ai:claude
// trace:BUG-1560 | ai:claude
pub(crate) fn collect_stale_review_story_flips(
    store: &aida_core::RequirementsStore,
    pr_to_sha: &std::collections::BTreeMap<u64, String>,
    flips: &[AutoBumpFlip],
) -> Vec<(String, String, u64, RequirementStatus)> {
    let mut out: Vec<(String, String, u64, RequirementStatus)> = Vec::new();
    for (pr_n, sha) in pr_to_sha {
        let Some(review_story) = store
            .requirements
            .iter()
            .find(|r| parse_review_story_pr_number(&r.title) == Some(*pr_n))
        else {
            continue;
        };
        if !matches!(
            review_story.status,
            RequirementStatus::Draft | RequirementStatus::Approved | RequirementStatus::InProgress
        ) {
            continue;
        }
        let Some(spec_id) = review_story.spec_id.as_deref() else {
            continue;
        };
        if flips.iter().any(|f| f.spec_id == spec_id) {
            continue;
        }
        if out.iter().any(|(id, ..)| id == spec_id) {
            continue;
        }
        out.push((
            spec_id.to_string(),
            sha.clone(),
            *pr_n,
            review_story.status.clone(),
        ));
    }
    out
}

/// BUG-219 / TASK-246: audit-comment text recorded on a review story the
/// auto-bump completed because its PR merged before review finished. The
/// wording names *why* review was skipped so the user can tell the work
/// did not go through a reviewer session. trace:BUG-219 | ai:claude
pub(crate) fn stale_review_audit_comment(prior: &RequirementStatus, pr_n: u64) -> String {
    match prior {
        RequirementStatus::Approved => format!(
            "Auto-completed: PR #{} merged without a reviewer session \
             (self-merge or orchestrator skipped phase 3).",
            pr_n
        ),
        // BUG-1560: a dedicated arm rather than falling into the InProgress
        // wording below, which would misdescribe a story that never reached
        // Approved at all ("left at In Progress" is simply false for one
        // that was left at Draft).
        RequirementStatus::Draft => format!(
            "Auto-completed: PR #{} merged without a reviewer session \
             (the review story was never queued past Draft).",
            pr_n
        ),
        _ => format!(
            "Auto-completed: PR #{} merged without a re-review iteration \
             — the review story was left at In Progress.",
            pr_n
        ),
    }
}

/// BUG-113: a review story can be stranded at `Done` after its PR merges.
/// The `(#N)`-in-`pr_to_sha` linkage (BUG-102 bumps the Done review story,
/// BUG-106 bumps the specs it covers) only fires while the PR's `(#N)`
/// merge commit is inside the current scan window. The common miss: the
/// reviewer flips the story In-Progress → Done *after* the merge-carrying
/// pull already ran, so no later pull re-scans that commit and the review
/// story sits at `Done` forever — `reconcile-status` couldn't recover it
/// either, since it shares the same window-bound scan.
///
/// The covers chain is a window-independent signal. A review story's
/// `implements` relationships are the specs /aida-pr's auto-queue recorded
/// from the PR's `(REQ-ID)` trailers — the `## Covers` list. A covered
/// spec only reaches `Completed` once its commit lands on the default
/// branch (auto-bump / reconcile graduate `Done → Completed` solely on a
/// default-branch merge), so a `Completed` covered spec *is* the proof its
/// PR merged. A covered spec sitting in the caller's `flips` list is being
/// completed by a default-branch commit in this very pass — same proof.
/// Either way the PR is merged, so a `Done` review story with ANY covered
/// spec already (or about-to-be) `Completed` should be `Completed` too.
/// Checking `flips` as well as the store keeps the single-pass guarantee:
/// when the merge commit IS in the window the covered spec and its review
/// story graduate together rather than needing a second pull.
///
/// Returns `(review_spec_id, completion_sha)` for each such review story,
/// skipping ones already in `flips`. The sha is the covered spec's merge
/// commit (its recorded `completion_sha`, else the sha it is being
/// completed with this pass); empty only when a covered spec was completed
/// manually and carries no sha — the caller leaves `completion_sha` unset
/// in that case. trace:BUG-113 | ai:claude
pub(crate) fn collect_covers_completed_review_flips(
    store: &aida_core::RequirementsStore,
    flips: &[AutoBumpFlip],
) -> Vec<AutoBumpFlip> {
    let mut out: Vec<AutoBumpFlip> = Vec::new();
    for review in &store.requirements {
        if !matches!(review.status, RequirementStatus::Done) {
            continue;
        }
        // Review stories only — title shape `Review PR-<n>: ...`. The
        // guard also stops a plain Done story that happens to `implements`
        // a Completed spec from being mistaken for a merged review story.
        if parse_review_story_pr_number(&review.title).is_none() {
            continue;
        }
        let Some(spec_id) = review.spec_id.as_deref() else {
            continue;
        };
        if flips.iter().any(|f| f.spec_id == spec_id) {
            continue;
        }
        if out.iter().any(|f| f.spec_id == spec_id) {
            continue;
        }
        // First covered spec that has merged (Completed in the store, or
        // being completed in this pass) graduates the review story.
        let mut flip: Option<AutoBumpFlip> = None;
        for rel in &review.relationships {
            if !matches!(
                &rel.rel_type,
                aida_core::RelationshipType::Custom(n) if n.eq_ignore_ascii_case("implements")
            ) {
                continue;
            }
            let Some(covered) = store.requirements.iter().find(|r| r.id == rel.target_id) else {
                continue;
            };
            let pending_sha = covered.spec_id.as_deref().and_then(|cid| {
                flips
                    .iter()
                    .find(|f| f.spec_id == cid)
                    .map(|f| f.sha.clone())
            });
            if !matches!(covered.status, RequirementStatus::Completed) && pending_sha.is_none() {
                continue;
            }
            let sha = covered
                .implementation_info
                .as_ref()
                .and_then(|i| i.completion_sha.clone())
                .or(pending_sha)
                .unwrap_or_default();
            flip = Some(AutoBumpFlip::new(
                spec_id.to_string(),
                sha,
                review.status.clone(),
            ));
            break;
        }
        if let Some(f) = flip {
            out.push(f);
        }
    }
    out
}

/// STORY-86 / BUG-328: scan the **code repo's** default branch for newly-landed
/// commits whose subject references a spec, and flip any eligible spec
/// (Approved, Planned, InProgress, Done) to `Completed`. Stamps
/// `implementation_info.completed_at`
/// and `implementation_info.completion_sha` so post-merge `aida show`
/// shows when (and from which commit) the spec actually shipped.
///
/// Inputs:
/// - `project_root`: the code repo (where merges to default branch
///   happen). The store_path's parent in the common case.
/// - `store_path`: the orphan-store worktree (where YAML is written).
/// - `pre_sha`: HEAD of the code repo BEFORE the pull. `None` means
///   "no snapshot was taken" — fall back to scanning HEAD~50..HEAD so
///   first-pull / shallow-clone cases still pick something up. The
///   eligibility guard below prevents us from over-flipping.
/// - `storage`: the orphan-store backend, used for the atomic write.
///
/// Semantics:
/// - Silently no-op when the current branch ≠ the default branch
///   (`origin/HEAD` if set, else `main` or `master`). On a feature
///   branch a `pull` shouldn't graduate work — that happens at PR
///   merge time, when the merge commit lands on default.
/// - Idempotent: only flips specs at an approved-to-ship status. Draft
///   preserves the approval signal; Completed/Rejected stay terminal.
/// - Honors `AIDA_AUTO_BUMP=false` via the caller.
/// - Returns a `Vec<AutoBumpFlip>` of flips for the caller to summarize.
///   Empty vec = nothing to print.
///
/// STORY-582: privacy floor for the durable processing record — redact
/// anything that looks like a secret before it lands in the committed YAML.
/// Conservative, prefix/keyword-driven (NOT a long-run base64 sweep, which
/// would eat legitimate SHAs): known token prefixes (`ghp_`, `sk-`, `AKIA…`,
/// `xoxb-`, …), `Bearer <token>`, and `secret/token/password/api_key = VALUE`
/// shapes collapse to `[REDACTED]`. trace:STORY-582 | ai:claude
pub(crate) fn redact_secrets(text: &str) -> String {
    let mut out: Vec<String> = Vec::with_capacity(text.split_whitespace().count());
    // Token-ish if it carries a known secret prefix OR is a long opaque run
    // gated behind a secret prefix — we only redact whole whitespace tokens.
    let looks_secret = |tok: &str| -> bool {
        let t = tok.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-');
        const PREFIXES: [&str; 9] = [
            "ghp_",
            "gho_",
            "ghu_",
            "ghs_",
            "ghr_",
            "github_pat_",
            "sk-",
            "xoxb-",
            "xoxp-",
        ];
        if PREFIXES.iter().any(|p| t.starts_with(p)) {
            return true;
        }
        // AWS access key id: AKIA + 16 base32 chars.
        if t.len() == 20 && t.starts_with("AKIA") {
            return true;
        }
        false
    };
    let mut prev_lower = String::new();
    for tok in text.split_whitespace() {
        // `Bearer <token>` → redact the token after a `Bearer`/`token:` lead.
        let after_bearer = prev_lower == "bearer";
        // `key = VALUE` / `key: VALUE` where key is secret-ish.
        let kv_secret = {
            let key = prev_lower
                .trim_end_matches([':', '='])
                .rsplit(['=', ':'])
                .next()
                .unwrap_or("");
            matches!(
                key,
                "secret" | "token" | "password" | "passwd" | "api_key" | "apikey"
            ) || prev_lower.ends_with("secret=")
                || prev_lower.ends_with("token=")
                || prev_lower.ends_with("password=")
                || prev_lower.ends_with("api_key=")
        };
        // `secret=VALUE` inline (no space).
        let inline_kv = {
            let lo = tok.to_ascii_lowercase();
            ["secret=", "token=", "password=", "api_key=", "apikey="]
                .iter()
                .any(|k| lo.starts_with(k))
        };
        if looks_secret(tok) || after_bearer || kv_secret {
            out.push("[REDACTED]".to_string());
        } else if inline_kv {
            let key = tok.split_once('=').map(|(k, _)| k).unwrap_or(tok);
            out.push(format!("{key}=[REDACTED]"));
        } else {
            out.push(tok.to_string());
        }
        prev_lower = tok.to_ascii_lowercase();
    }
    out.join(" ")
}

/// STORY-582: read the reviewer verdict promoted into the durable record from
/// the gitignored `.aida/review-verdicts/<SPEC>.json`. Returns the normalized
/// verdict word plus the one-line summary, when the file exists + parses.
/// trace:STORY-582 | ai:claude
pub(crate) fn read_review_verdict_for_record(
    project_root: &std::path::Path,
    spec_id: &str,
) -> Option<reviewer_summary::VerdictFile> {
    let path = project_root
        .join(".aida")
        .join("review-verdicts")
        .join(format!("{spec_id}.json"));
    let body = std::fs::read_to_string(path).ok()?;
    reviewer_summary::parse_verdict_file(&body)
}

/// STORY-582: find the most recent brief (pending `.md` or acked
/// `.md.acked`) routed for `spec_id`, across every agent mailbox dir.
/// Returns `(project_relative_path, agent, generated_by)`. The brief is the
/// otherwise-gitignored routing artifact we promote into the durable record.
/// trace:STORY-582 | ai:claude
pub(crate) fn latest_brief_for_spec(
    project_root: &std::path::Path,
    spec_id: &str,
) -> Option<(String, String, Option<String>)> {
    let root = project_root.join(".aida").join("agent-briefs");
    let prefix = format!("{}-", spec_id.to_ascii_uppercase());
    let mut best: Option<(String, String, std::path::PathBuf)> = None; // (fname, agent, path)
    let agent_dirs = std::fs::read_dir(&root).ok()?;
    for agent_entry in agent_dirs.flatten() {
        if !agent_entry.path().is_dir() {
            continue;
        }
        let agent = agent_entry.file_name().to_string_lossy().to_string();
        let Ok(briefs) = std::fs::read_dir(agent_entry.path()) else {
            continue;
        };
        for b in briefs.flatten() {
            let fname = b.file_name().to_string_lossy().to_string();
            let is_brief = fname.ends_with(".md") || fname.ends_with(".md.acked");
            if !is_brief || !fname.to_ascii_uppercase().starts_with(&prefix) {
                continue;
            }
            // Filenames embed a sortable `…-<UTC timestamp>.md`, so the
            // lexically-greatest name is the most recent brief.
            if best.as_ref().map(|(f, _, _)| &fname > f).unwrap_or(true) {
                best = Some((fname.clone(), agent.clone(), b.path()));
            }
        }
    }
    let (_, agent, path) = best?;
    let generated_by = std::fs::read_to_string(&path).ok().and_then(|body| {
        body.lines()
            .find_map(|l| {
                l.strip_prefix("generated_by:")
                    .map(|v| v.trim().to_string())
            })
            .filter(|s| !s.is_empty())
    });
    let rel = path
        .strip_prefix(project_root)
        .unwrap_or(&path)
        .display()
        .to_string();
    Some((rel, agent, generated_by))
}

/// STORY-582: assemble the durable [`ProcessingRecord`] for a spec being
/// completed — promoting the gitignored review verdict + brief artifacts and
/// the punt ledger into a committed, queryable, secret-scrubbed audit row.
/// Reuses existing capture points (AC-2): no new author burden. The PR number
/// is best-effort from the verdict's review-comment URL. trace:STORY-582
pub(crate) fn build_processing_record(
    project_root: &std::path::Path,
    spec_id: &str,
    commit_sha: &str,
) -> aida_core::ProcessingRecord {
    let brief = latest_brief_for_spec(project_root, spec_id);
    let verdict = read_review_verdict_for_record(project_root, spec_id);

    // Agent: brief `generated_by` → live lease role on the spec → "aida".
    let agent = brief
        .as_ref()
        .and_then(|(_, _, gb)| gb.clone())
        .or_else(|| {
            list_leases(project_root)
                .into_iter()
                .find(|l| l.scope.eq_ignore_ascii_case(spec_id))
                .and_then(|l| l.role)
        })
        .unwrap_or_else(|| "aida".to_string());

    let summary = verdict
        .as_ref()
        .and_then(|v| v.summary.clone())
        .filter(|s| !s.trim().is_empty())
        .map(|s| redact_secrets(&s))
        .unwrap_or_else(|| {
            let short = &commit_sha[..commit_sha.len().min(8)];
            format!("Completed via merge {short}")
        });

    let mut decisions: Vec<String> = Vec::new();
    if let Some(v) = verdict.as_ref() {
        if let Some(c) = v.complexity_agreement.as_deref().filter(|s| !s.is_empty()) {
            decisions.push(redact_secrets(&format!("complexity: {c}")));
        }
    }

    // Punted: this spec's punt-ledger details + any follow-up TASKs the
    // reviewer filed.
    let mut punted: Vec<String> = punt::read_ledger(project_root)
        .into_iter()
        .filter(|r| r.spec.eq_ignore_ascii_case(spec_id))
        .map(|r| {
            let detail = redact_secrets(r.detail.trim());
            match r.resolution_path.as_str() {
                "escalated-to-human" => format!("escalated: {detail}"),
                "advisor-resolved" => format!("resolved: {detail}"),
                _ => format!("punted: {detail}"),
            }
        })
        .collect();
    if let Some(filed) = verdict.as_ref().and_then(|v| v.findings_filed.clone()) {
        for f in filed.into_iter().filter(|f| !f.trim().is_empty()) {
            punted.push(format!("follow-up: {}", f.trim()));
        }
    }

    let pr = verdict
        .as_ref()
        .and_then(|v| v.comment_url.as_deref())
        .and_then(parse_pr_number_from_url);

    let mut record = aida_core::ProcessingRecord::new(agent, summary);
    record.brief_ref = brief.map(|(rel, _, _)| rel);
    record.commit_sha = (!commit_sha.is_empty()).then(|| commit_sha.to_string());
    record.pr = pr;
    record.decisions = decisions;
    record.punted = punted;
    // trace:BUG-1505 | ai:claude — persist the canonical spelling.
    record.review_verdict = verdict
        .map(|v| review_verdict::canonical_verdict_word(&v.verdict))
        .filter(|s| !s.is_empty());
    record
}

/// STORY-582: pull a PR/MR number out of a forge review-comment URL like
/// `https://github.com/o/r/pull/123#issuecomment-…`. trace:STORY-582
pub(crate) fn parse_pr_number_from_url(url: &str) -> Option<u64> {
    let marker = url.find("/pull/").map(|i| i + "/pull/".len()).or_else(|| {
        url.find("/merge_requests/")
            .map(|i| i + "/merge_requests/".len())
    })?;
    url[marker..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

// Shared per-requirement mutation for the Done→Completed auto-bump. One
// body used by BOTH write paths: the targeted per-spec path (git-canonical
// stores — one `update SPEC-ID` commit per bumped spec, no bulk chore
// commits) and the legacy `update_atomically` full-store path (YAML/SQLite).
// Returns true when the requirement actually flipped (caller then persists).
// Re-checks eligibility against the just-re-read copy so a concurrent edit
// that moved the spec off an eligible status wins.
// trace:TASK-1161 | ai:claude
pub(crate) fn apply_auto_bump_flip(
    r: &mut aida_core::Requirement,
    flip: &AutoBumpFlip,
    now: chrono::DateTime<chrono::Utc>,
    project_root: &std::path::Path,
) -> bool {
    // Re-check against the freshest copy — concurrent edits may have
    // moved it off an eligible status.
    if !auto_bump_eligible_status(&r.status) {
        return false;
    }
    // trace:TASK-1600 | ai:codex
    if auto_bump_evidence_is_stale(project_root, r, &flip.sha) {
        return false;
    }
    // BUG-477: record the merge-driven bump in the per-spec history the same
    // way the manual `aida edit --status` path does.
    // trace:STORY-1418 | ai:claude
    let prior_status = completion::mark_completed(r);
    // BUG-1637: through the one shared history helper. trace:BUG-1637 | ai:claude
    aida_core::conflict::record_status_transition(
        r,
        aida_core::conflict::AUTO_BUMP_AUTHOR,
        &prior_status,
    );
    r.modified_at = now;
    // BUG-405: a Completed spec must not carry a stale FailureReason.
    r.failure_reason = None;
    let info = r
        .implementation_info
        .get_or_insert_with(aida_core::ImplementationInfo::default);
    info.completed_at.get_or_insert(now);
    // BUG-113: a covers-chain flip can carry an empty sha when the covered
    // spec was completed manually — don't stamp `Some("")`.
    if info.completion_sha.is_none() && !flip.sha.is_empty() {
        info.completion_sha = Some(flip.sha.clone());
        // STORY-634 (ADR-12 D2/D5): qualify the record, not the SHA — stamp
        // the workspace repo slug the completing commit landed in as a sibling
        // to completion_sha. None (single-repo, no manifest) stays absent.
        info.completion_repo = workspace_repo_slug(project_root);
    }
    // STORY-582: capture the durable processing record. Idempotent on the
    // completing SHA (re-runs of the bump don't stack duplicates).
    let record = build_processing_record(project_root, &flip.spec_id, &flip.sha);
    r.add_processing_record(record);
    // STORY-1385: stretch criteria never hold completion, but any still
    // unchecked when the spec completes are recorded as debt, automatically.
    // trace:STORY-1385 | ai:claude
    if let Some(note) = stretch_debt_comment(r, &flip.sha) {
        r.add_comment(aida_core::Comment::new("aida-auto-bump".to_string(), note));
    }
    true
}

/// STORY-1385: marker on the debt note written when a spec completes with
/// unchecked STRETCH closure criteria. Greppable so the debt population (and
/// the stretch-met rate) is countable from spec comments.
pub(crate) const STRETCH_DEBT_MARKER: &str = "[aida:stretch-debt]";

/// STORY-1385: the debt note for `r`'s unmet stretch criteria at completion,
/// or `None` when every stretch criterion was met (or none were declared) or
/// the note for this completing commit already exists. Records which
/// criterion, what was wanted, what shipped instead, and what closes it.
// trace:STORY-1385 | ai:claude
pub(crate) fn stretch_debt_comment(r: &aida_core::Requirement, sha: &str) -> Option<String> {
    let unmet = aida_core::pickability::unmet_stretch_closure_criteria(r);
    if unmet.is_empty() {
        return None;
    }
    let already = r.comments.iter().any(|c| {
        c.content.contains(STRETCH_DEBT_MARKER) && (sha.is_empty() || c.content.contains(sha))
    });
    if already {
        return None;
    }
    let shipped = if sha.is_empty() {
        "the merged change".to_string()
    } else {
        format!("commit {sha}")
    };
    let items = unmet
        .iter()
        .enumerate()
        .map(|(i, c)| format!("\n  {}. wanted: \"{c}\"", i + 1))
        .collect::<String>();
    Some(format!(
        "{STRETCH_DEBT_MARKER} Completed with {} unmet stretch closure criteri{} (marked \
         stretch ahead of time, so they did not hold completion):{items}\n\
         Shipped instead: {shipped}, which met every required criterion but not these. \
         To close the gap: implement the criterion in a follow-up, then check its box in \
         the spec's Closure section.",
        unmet.len(),
        if unmet.len() == 1 { "on" } else { "a" },
    ))
}

/// BUG-1551: marker on the audit comment a closure hold writes. Also the
/// dedupe key (with the merge SHA) so a re-run of pull/reconcile over the same
/// commit does not stack notes.
pub(crate) const CLOSURE_HOLD_MARKER: &str = "[aida:closure-held]";

/// BUG-1551: a merge-evidence flip that the auto-bump will NOT complete,
/// because the spec still has an unresolved `BlockedBy` predecessor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClosureHold {
    pub(crate) flip: AutoBumpFlip,
    pub(crate) blockers: Vec<aida_core::pickability::ClosureBlocker>,
    /// BUG-1721: the subset of `blockers` that are themselves held candidates
    /// from this same reconcile pass — a dependency cycle among specs one merge
    /// credited, reported as such instead of as an ordinary blocker wait.
    // trace:BUG-1721 | ai:claude
    pub(crate) cycle_members: Vec<String>,
    /// STORY-1430: the spec's own declared closure criteria still unmet (a
    /// `closure:pending` tag, or unchecked items in its Closure section).
    pub(crate) criteria: Vec<String>,
}

/// STORY-1430: does anything hold `req`'s closure — an unresolved BlockedBy
/// predecessor (BUG-1551) or one of its own declared, unmet closure criteria?
/// Returns both sets; both empty = the auto-bump completes exactly as before.
// trace:STORY-1430 | ai:claude
pub(crate) fn closure_holders(
    req: &aida_core::Requirement,
    store: &aida_core::RequirementsStore,
) -> (Vec<aida_core::pickability::ClosureBlocker>, Vec<String>) {
    closure_holders_treating_resolved(req, store, &std::collections::HashSet::new())
}

/// BUG-1721: [`closure_holders`], treating `resolved` requirement ids as
/// already closed — the specs the current reconcile pass is itself completing.
/// A spec's OWN declared closure criteria are never affected by another spec
/// completing, so they pass straight through.
// trace:BUG-1721 | ai:claude
pub(crate) fn closure_holders_treating_resolved(
    req: &aida_core::Requirement,
    store: &aida_core::RequirementsStore,
    resolved: &std::collections::HashSet<uuid::Uuid>,
) -> (Vec<aida_core::pickability::ClosureBlocker>, Vec<String>) {
    (
        aida_core::pickability::unresolved_closure_blockers_treating_resolved(req, store, resolved),
        aida_core::pickability::unmet_declared_closure_criteria(req),
    )
}

/// BUG-1551: `BlockedBy` gates closure, not just pickup (ADR recorded on the
/// spec). Split `flips` into the ones free to complete (kept in place) and the
/// ones held at Done by an unresolved blocker (returned). Shared by the live
/// `aida pull` / `db sync --pull` auto-bump and the `db reconcile-status`
/// replay so the two can't drift.
// trace:BUG-1551 | ai:claude
pub(crate) fn split_closure_held_flips(
    store: &aida_core::RequirementsStore,
    flips: &mut Vec<AutoBumpFlip>,
) -> Vec<ClosureHold> {
    // BUG-1721: ONE merge can credit both a blocker and the spec it blocks. A
    // single pass evaluated the blocked spec while its blocker was still
    // InProgress in the store, held it at Done, and then completed the blocker
    // later in the same pass — leaving the dependent un-credited until some
    // later `aida pull`. Iterate to a LEAST fixed point instead: a flip is
    // released once every closure holder is resolved in the store OR is itself
    // being released by this pass. Growing a resolved-id set (rather than
    // stamping Completed into a store copy) keeps the STORY-1418 completion
    // seam the only path into Completed, and costs no store clone.
    //
    // Least, not greatest: the set starts EMPTY and only grows, so a BlockedBy
    // cycle among credited specs never becomes resolvable and every member
    // stays held. Each iteration must release at least one flip or the loop
    // breaks, so it terminates in at most `flips.len()` iterations.
    // trace:BUG-1721 | ai:claude
    let mut resolved: std::collections::HashSet<uuid::Uuid> = std::collections::HashSet::new();
    let mut released = vec![false; flips.len()];
    loop {
        let mut progress = false;
        for (index, flip) in flips.iter().enumerate() {
            if released[index] {
                continue;
            }
            // The pre-fix `retain` KEPT a flip whose spec does not resolve in
            // the store (there was nothing to hold it on, and the write loop
            // skips it harmlessly). Release it so this rewrite cannot silently
            // drop it from `flips`, which `aida pull` also reports from.
            let Some(req) = store.get_requirement_by_spec_id(&flip.spec_id) else {
                released[index] = true;
                progress = true;
                continue;
            };
            let (blockers, criteria) = closure_holders_treating_resolved(req, store, &resolved);
            if blockers.is_empty() && criteria.is_empty() {
                resolved.insert(req.id);
                released[index] = true;
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    // BUG-1721: `ClosureBlocker.id` is the blocker's DISPLAY id (agreed > spec >
    // internal) while a flip carries whichever form its trailer used, so record
    // both forms — otherwise a cycle between agreed-id specs reads as an
    // ordinary blocker wait.
    let mut held_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (index, flip) in flips.iter().enumerate() {
        if released[index] {
            continue;
        }
        held_ids.insert(flip.spec_id.clone());
        if let Some(req) = store.get_requirement_by_spec_id(&flip.spec_id) {
            held_ids.insert(
                req.agreed_id
                    .clone()
                    .or_else(|| req.spec_id.clone())
                    .unwrap_or_else(|| req.id.to_string()),
            );
        }
    }
    let mut held = Vec::new();
    let mut completed = Vec::new();
    for (index, flip) in flips.iter().enumerate() {
        if released[index] {
            completed.push(flip.clone());
            continue;
        }
        let Some(req) = store.get_requirement_by_spec_id(&flip.spec_id) else {
            continue;
        };
        let (blockers, criteria) = closure_holders(req, store);
        let cycle_members = blockers
            .iter()
            .filter(|b| held_ids.contains(&b.id))
            .map(|b| b.id.clone())
            .collect();
        held.push(ClosureHold {
            flip: flip.clone(),
            blockers,
            cycle_members,
            criteria,
        });
    }
    *flips = completed;
    held
}

/// BUG-1551: the audit note recording that the code merged but closure is
/// held. Stored on the spec so `aida show` carries it.
// trace:BUG-1551 | ai:claude
pub(crate) fn closure_hold_comment(hold: &ClosureHold) -> String {
    let short = if hold.flip.sha.len() >= 7 {
        &hold.flip.sha[..7]
    } else {
        hold.flip.sha.as_str()
    };
    let id = &hold.flip.spec_id;
    let mut why = Vec::new();
    if !hold.blockers.is_empty() {
        if !hold.cycle_members.is_empty() {
            why.push(format!(
                "dependency cycle involving {}",
                hold.cycle_members.join(", ")
            ));
        } else {
            why.push(format!(
                "unresolved BlockedBy {}. BlockedBy gates completion as well as pickup; it \
             releases once every blocker is Completed, Rejected or Superseded (an epic by \
             its child rollup, an ADR once accepted)",
                aida_core::pickability::closure_blockers_label(&hold.blockers)
            ));
        }
    }
    // STORY-1430: say WHAT is unmet and WHO resolves it. trace:STORY-1430 | ai:claude
    if !hold.criteria.is_empty() {
        why.push(format!(
            "the spec's own declared closure criteria are unmet: {}. Whoever owns those \
             criteria (the spec's owner or the advisor) resolves them by checking the box \
             in the spec's Closure section, or removing the tag, via `aida edit {id} \
             --description ...` / `aida edit {id} --remove-tag {}`",
            closure_criteria_label(&hold.criteria),
            aida_core::pickability::CLOSURE_PENDING_TAG
        ));
    }
    format!(
        "{CLOSURE_HOLD_MARKER} Code merged to the default branch (commit {short}), but \
         completion is held at Done: {}. The next `aida pull` completes it once nothing \
         holds it; to replay by hand, run `aida db reconcile-status --spec {id} --since \
         {short}^`. If the blockers form a cycle, or you decide to ship regardless, a \
         human runs `aida edit {id} --status completed`. (merge sha: {})",
        why.join("; and "),
        hold.flip.sha
    )
}

/// BUG-1551: the merge sha a closure-hold note recorded (the newest note wins).
// trace:BUG-1551 | ai:claude
pub(crate) fn closure_hold_merge_sha(req: &aida_core::Requirement) -> Option<String> {
    req.comments
        .iter()
        .rev()
        .filter(|c| c.content.contains(CLOSURE_HOLD_MARKER))
        .find_map(|c| {
            let at = c.content.rfind("(merge sha: ")? + "(merge sha: ".len();
            let sha: String = c.content[at..]
                .chars()
                .take_while(|ch| ch.is_ascii_hexdigit())
                .collect();
            (!sha.is_empty()).then_some(sha)
        })
}

/// BUG-1551: specs currently HELD (Done + carrying a closure-hold note) whose
/// blockers have all since resolved — released as ordinary merge flips so each
/// `aida pull` completes them without needing their (possibly old) merge
/// commit in the scan window. Cheap: only held specs are considered.
// trace:BUG-1551 | ai:claude
pub(crate) fn collect_released_closure_holds(
    store: &aida_core::RequirementsStore,
) -> Vec<AutoBumpFlip> {
    store
        .requirements
        .iter()
        .filter(|r| matches!(r.status, RequirementStatus::Done))
        .filter_map(|r| {
            let sha = closure_hold_merge_sha(r)?;
            // trace:STORY-1430 | ai:claude
            let (blockers, criteria) = closure_holders(r, store);
            if !blockers.is_empty() || !criteria.is_empty() {
                return None;
            }
            let spec_id = r.agreed_id.clone().or_else(|| r.spec_id.clone())?;
            Some(AutoBumpFlip::new(spec_id, sha, r.status.clone()))
        })
        .collect()
}

/// BUG-1551: apply a closure hold to the freshest copy of the spec — land it
/// at Done (a pre-Done in-flight spec moves up to Done, since its code is on
/// the default branch) and record the merge in a deduped audit comment.
/// Deliberately does NOT stamp `completion_sha`: that field means "this commit
/// completed the spec", and stamping it would make the BUG-410 re-bump guard
/// refuse the eventual completion. Returns true when something changed.
// trace:BUG-1551 | ai:claude
pub(crate) fn apply_closure_hold(
    r: &mut aida_core::Requirement,
    hold: &ClosureHold,
    now: chrono::DateTime<chrono::Utc>,
    project_root: &std::path::Path,
) -> bool {
    if auto_bump_evidence_is_stale(project_root, r, &hold.flip.sha) {
        return false;
    }
    if !auto_bump_eligible_status(&r.status) {
        return false;
    }
    let mut changed = false;
    if !matches!(r.status, RequirementStatus::Done) {
        let prior = r.status.clone();
        r.set_status_from_str("Done");
        // BUG-1637: through the one shared history helper. trace:BUG-1637 | ai:claude
        aida_core::conflict::record_status_transition(
            r,
            aida_core::conflict::AUTO_BUMP_AUTHOR,
            &prior,
        );
        changed = true;
    }
    let already_noted = r.comments.iter().any(|c| {
        c.content.contains(CLOSURE_HOLD_MARKER)
            && (hold.flip.sha.is_empty() || c.content.contains(hold.flip.sha.as_str()))
    });
    if !already_noted {
        r.add_comment(aida_core::Comment::new(
            "aida-auto-bump".to_string(),
            closure_hold_comment(hold),
        ));
        changed = true;
    }
    if changed {
        r.modified_at = now;
    }
    changed
}

/// STORY-1436: the closure hold as a [`events::EventKind::GateHeld`] record.
// trace:STORY-1436 | ai:claude
pub(crate) fn closure_hold_reason(hold: &ClosureHold) -> String {
    format!(
        "merged ({}) but completion held at Done — {}",
        hold.flip.sha.chars().take(7).collect::<String>(),
        closure_hold_causes(hold)
    )
}

/// What holds `hold` — blockers and/or unmet criteria — without the
/// "held at Done" framing, for paths that do not land at Done.
// trace:TASK-1335 | ai:claude
pub(crate) fn closure_hold_causes(hold: &ClosureHold) -> String {
    let mut why = Vec::new();
    if !hold.blockers.is_empty() {
        why.push(format!(
            "blocked by {}",
            aida_core::pickability::closure_blockers_label(&hold.blockers)
        ));
    }
    if !hold.criteria.is_empty() {
        why.push(format!(
            "closure criteria unmet: {}",
            closure_criteria_label(&hold.criteria)
        ));
    }
    why.join("; ")
}

// trace:STORY-1436 | ai:claude
pub(crate) fn record_closure_hold_event(project_root: &std::path::Path, hold: &ClosureHold) {
    events::record_gate_held(
        project_root,
        events::GATE_CLOSURE_HOLD,
        Some(hold.flip.spec_id.clone()),
        None,
        &closure_hold_reason(hold),
    );
}

/// BUG-1551: print one line per held spec, naming the blocker.
// trace:BUG-1551 | ai:claude
pub(crate) fn report_closure_holds(holds: &[ClosureHold]) {
    for hold in holds {
        if !hold.blockers.is_empty() {
            if !hold.cycle_members.is_empty() {
                eprintln!(
                    "  {} {} stays Done — dependency cycle involving {}",
                    "↷".yellow(),
                    hold.flip.spec_id,
                    hold.cycle_members.join(", ")
                );
            } else {
                eprintln!(
                    "  {} {} stays Done — merged, but blocked by {} (completion waits for \
                     the blocker)",
                    "↷".yellow(),
                    hold.flip.spec_id,
                    aida_core::pickability::closure_blockers_label(&hold.blockers)
                );
            }
        }
        // trace:STORY-1430 | ai:claude
        if !hold.criteria.is_empty() {
            eprintln!(
                "  {} {} stays Done — merged, but its own closure criteria are unmet: {} \
                 (the owner or advisor resolves them; see `aida why {}`)",
                "↷".yellow(),
                hold.flip.spec_id,
                closure_criteria_label(&hold.criteria),
                hold.flip.spec_id
            );
        }
    }
}

// Shared per-requirement mutation for the TASK-246/BUG-219 stale-review-story
// flip (PR merged before the review lifecycle finished). Same dual-path use
// as `apply_auto_bump_flip`; re-checks the live status so a second pass sees
// Completed and stays idempotent. Must stay in lockstep with the candidate
// filter in `collect_stale_review_story_flips` — BUG-1543's review found
// that widening only the collector and not this re-check produces a silent
// no-op (a "would flip" candidate that this gate then quietly refuses).
// trace:TASK-1161 | ai:claude
// trace:BUG-1560 | ai:claude
pub(crate) fn apply_stale_review_flip(
    r: &mut aida_core::Requirement,
    sha: &str,
    pr_n: u64,
    now: chrono::DateTime<chrono::Utc>,
    project_root: &std::path::Path,
) -> bool {
    if !matches!(
        r.status,
        RequirementStatus::Draft | RequirementStatus::Approved | RequirementStatus::InProgress
    ) {
        return false;
    }
    if auto_bump_evidence_is_stale(project_root, r, sha) {
        return false;
    }
    // trace:STORY-1418 | ai:claude
    let prior = completion::mark_completed(r);
    // BUG-477: record the flip-to-Completed in the per-spec history too.
    // BUG-1637: through the one shared history helper. trace:BUG-1637 | ai:claude
    aida_core::conflict::record_status_transition(r, aida_core::conflict::AUTO_BUMP_AUTHOR, &prior);
    r.modified_at = now;
    // BUG-405 contract: a Completed spec must not keep a stale FailureReason.
    r.failure_reason = None;
    let info = r
        .implementation_info
        .get_or_insert_with(aida_core::ImplementationInfo::default);
    info.completed_at.get_or_insert(now);
    if info.completion_sha.is_none() && !sha.is_empty() {
        info.completion_sha = Some(sha.to_string());
        // STORY-634 (ADR-12 D2/D5): same repo-slug qualifier as the
        // auto-bump stamp; absent in the single-repo case.
        info.completion_repo = workspace_repo_slug(project_root);
    }
    r.add_comment(aida_core::Comment::new(
        "aida-auto-bump".to_string(),
        stale_review_audit_comment(&prior, pr_n),
    ));
    true
}

/// TASK-1296: how a stranded Review-PR spec's own PR resolved on the forge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StrandedReviewPrOutcome {
    /// PR merged, but its `(#N)` commit fell outside the git-log scan
    /// window — the window-independent counterpart of `apply_stale_review_flip`.
    Merged,
    /// PR closed WITHOUT merging. Never produces a merge commit, so the
    /// git-log scan has no evidence for it at all; the forge is the only
    /// source of truth.
    ClosedUnmerged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StrandedReviewPrResolution {
    pub(crate) spec_id: String,
    pub(crate) pr_n: u64,
    pub(crate) outcome: StrandedReviewPrOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StrandedReviewPrLookupFailure {
    pub(crate) spec_id: String,
    pub(crate) pr_n: u64,
    pub(crate) reason: String,
}

pub(crate) fn stranded_review_pr_lookup_failure_message(
    failures: &[StrandedReviewPrLookupFailure],
) -> Option<String> {
    if failures.is_empty() {
        return None;
    }
    let details = failures
        .iter()
        .map(|failure| {
            format!(
                "{} (PR #{}): {}",
                failure.spec_id, failure.pr_n, failure.reason
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    Some(format!(
        "stranded-spec resolution skipped {} spec{} because forge lookup failed: {}",
        failures.len(),
        if failures.len() == 1 { "" } else { "s" },
        details
    ))
}

pub(crate) fn report_stranded_review_pr_lookup_failures(
    failures: &[StrandedReviewPrLookupFailure],
) {
    if let Some(message) = stranded_review_pr_lookup_failure_message(failures) {
        eprintln!("  {} {}", "Warning:".yellow().bold(), message);
    }
}

/// TASK-1296: a "Review PR-N" spec left `Approved`/`InProgress` while its own
/// PR already reached a terminal state on the forge, undetected by the
/// git-log scan (`pr_to_sha`) because that scan only sees a *merge* commit
/// inside its window — a PR closed without merging never produces one at
/// all, and a PR merged far enough back can fall outside the window too.
/// Observed 2026-09-19: six such specs sat Approved routing to the reviewer
/// queue for PRs that had already merged or closed, inflating queue depth
/// with dead entries `aida queue gc` correctly leaves alone (it prunes on
/// the queue entry's TARGET spec status; here the review spec itself is the
/// stale Approved thing — gc has nothing to key off until this flip runs).
///
/// Bounded, not a poller: only the review-story-shaped specs still open
/// after the local scan reach the forge, so this rides `aida pull`'s
/// existing cadence rather than adding one. `metadata` lookup failures
/// (offline, no `gh`/`glab`, auth) fail open — "cannot confirm" leaves the
/// spec untouched — but are returned separately so `aida pull` cannot mistake
/// an unavailable forge for an open PR.
///
/// BUG-1543: also covers `Draft` — a review story left Draft by a failed
/// queueing step (the shape BUG-1230 records) is exactly the stranded case
/// this sweep exists to clean up, not an exclusion from it.
// trace:TASK-1296 | ai:claude
// trace:BUG-1432 | ai:codex
// trace:BUG-1543 | ai:claude
pub(crate) fn collect_stranded_review_pr_resolutions(
    store: &aida_core::RequirementsStore,
    pr_to_sha: &std::collections::BTreeMap<u64, String>,
    project_root: &std::path::Path,
) -> (
    Vec<StrandedReviewPrResolution>,
    Vec<StrandedReviewPrLookupFailure>,
) {
    let mut out = Vec::new();
    let mut failures = Vec::new();
    let mut sink = network_retry::NoopSink;
    for req in &store.requirements {
        if !matches!(
            req.status,
            RequirementStatus::Draft | RequirementStatus::Approved | RequirementStatus::InProgress
        ) {
            continue;
        }
        let Some(pr_n) = parse_review_story_pr_number(&req.title) else {
            continue;
        };
        // Already resolvable via the git-log-based merged path above — don't
        // pay for a redundant forge round trip.
        if pr_to_sha.contains_key(&pr_n) {
            continue;
        }
        let Some(spec_id) = req.spec_id.as_deref() else {
            continue;
        };
        let metadata = match crate::forge::forge_for(project_root).change_metadata(pr_n, &mut sink)
        {
            Ok(metadata) => metadata,
            Err(error) => {
                failures.push(StrandedReviewPrLookupFailure {
                    spec_id: spec_id.to_string(),
                    pr_n,
                    reason: error.to_string(),
                });
                continue;
            }
        };
        let outcome = match metadata.state {
            crate::forge::ChangeState::Merged => StrandedReviewPrOutcome::Merged,
            crate::forge::ChangeState::Closed => StrandedReviewPrOutcome::ClosedUnmerged,
            // Still open — untouched, per acceptance.
            crate::forge::ChangeState::Open => continue,
        };
        // trace:TASK-1600 | ai:codex
        if outcome == StrandedReviewPrOutcome::Merged
            && req
                .implementation_info
                .as_ref()
                .and_then(|i| i.reopened_at_sha.as_ref())
                .is_some()
        {
            continue;
        }
        out.push(StrandedReviewPrResolution {
            spec_id: spec_id.to_string(),
            pr_n,
            outcome,
        });
    }
    (out, failures)
}

/// Applies one [`StrandedReviewPrResolution`]. Re-checks the live status —
/// concurrent edits (or a second pull racing this one) may have moved it off
/// Draft/Approved/InProgress already.
// trace:TASK-1296 | ai:claude
// trace:BUG-1543 | ai:claude
pub(crate) fn apply_stranded_review_pr_resolution(
    r: &mut aida_core::Requirement,
    resolution: &StrandedReviewPrResolution,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    if !matches!(
        r.status,
        RequirementStatus::Draft | RequirementStatus::Approved | RequirementStatus::InProgress
    ) {
        return false;
    }
    if resolution.outcome == StrandedReviewPrOutcome::Merged
        && r.implementation_info
            .as_ref()
            .and_then(|i| i.reopened_at_sha.as_ref())
            .is_some()
    {
        return false;
    }
    let prior = r.status.clone();
    match resolution.outcome {
        StrandedReviewPrOutcome::Merged => {
            // trace:STORY-1418 | ai:claude
            completion::mark_completed(r);
            r.failure_reason = None;
            let info = r
                .implementation_info
                .get_or_insert_with(aida_core::ImplementationInfo::default);
            info.completed_at.get_or_insert(now);
            r.add_comment(aida_core::Comment::new(
                "aida-auto-bump".to_string(),
                format!(
                    "Auto-completed: PR #{} is merged on the forge (its merge commit was \
                     outside the local scan window).",
                    resolution.pr_n
                ),
            ));
        }
        StrandedReviewPrOutcome::ClosedUnmerged => {
            r.set_status_from_str("Rejected");
            r.add_comment(aida_core::Comment::new(
                "aida-auto-bump".to_string(),
                format!(
                    "Auto-rejected: PR #{} closed on the forge without merging.",
                    resolution.pr_n
                ),
            ));
        }
    }
    // BUG-1637: through the one shared history helper. trace:BUG-1637 | ai:claude
    aida_core::conflict::record_status_transition(r, aida_core::conflict::AUTO_BUMP_AUTHOR, &prior);
    r.modified_at = now;
    true
}

/// trace:STORY-86 | ai:claude
pub(crate) fn auto_bump_done_to_completed(
    project_root: &std::path::Path,
    store_path: &std::path::Path,
    pre_sha: Option<&str>,
    storage: &Storage,
) -> Result<Vec<AutoBumpFlip>> {
    auto_bump_done_to_completed_with(project_root, store_path, pre_sha, storage, true)
        .map(|(flips, _scanned)| flips)
}

/// [`auto_bump_done_to_completed`] with plan-followup filing (step 7)
/// optional. BUG-1633: `aida pull --code-only` flips against a store it did
/// not pull, so it defers followup filing instead of acting on stale data.
/// Returns `(flips, scanned)`: `scanned` is false when no commit range was
/// walked (HEAD off the default branch, no default branch, `git log`
/// failed), so a persisted scan start must be kept for a later pull.
// trace:STORY-86 trace:BUG-1633 | ai:claude
pub(crate) fn auto_bump_done_to_completed_with(
    project_root: &std::path::Path,
    store_path: &std::path::Path,
    pre_sha: Option<&str>,
    storage: &Storage,
    extract_followups: bool,
) -> Result<(Vec<AutoBumpFlip>, bool)> {
    use aida_core::git_ops;
    use std::process::Command as ProcessCommand;

    // ── Step 1: detect the default branch ──
    // Try `git symbolic-ref --short refs/remotes/origin/HEAD` first
    // (it's set at clone time). Fall back to current_branch ∈
    // {main, master}. If neither, skip silently — no remote means
    // no merges to react to.
    let default_branch: Option<String> = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8(o.stdout)
                .ok()
                .map(|s| s.trim().to_string())
                .and_then(|s| s.strip_prefix("origin/").map(|x| x.to_string()))
        });

    let current = git_ops::current_branch(project_root).ok();
    let default_branch = default_branch.or_else(|| match current.as_deref() {
        Some("main") | Some("master") => current.clone(),
        _ => None,
    });
    let Some(default_branch) = default_branch else {
        return Ok((Vec::new(), false));
    };
    if current.as_deref() != Some(default_branch.as_str()) {
        return Ok((Vec::new(), false));
    }

    // BUG-568: this scan only walks the LOCAL repo's default branch. If the
    // store is shared across multiple repos, a spec landed in a sibling repo
    // won't auto-complete here — warn loudly so the miss is visible.
    // trace:BUG-568 | ai:claude
    warn_multi_repo_scan_limited(project_root, "auto-bump (Done→Completed) scan");

    // ── Step 2: build the git log invocation ──
    // BUG-94: with `pre_sha = None` we can't form a `X..HEAD` range
    // because `HEAD~50` fails (`git rev-parse` exit 128) on repos with
    // fewer than 50 commits — `aida db sync --pull` would then silently
    // skip the bump on any small/young project. Use `--max-count=50 HEAD`
    // instead, which degrades gracefully when history is shorter. The
    // eligibility guard inside step 4 keeps over-broad windows from
    // double-firing.
    // BUG-536: read the full commit message (`%B`), not just the subject
    // (`%s`). A squash-merge of an umbrella PR carries only ONE `(SPEC-ID)`
    // trailer in its subject, but GitHub's default squash body concatenates
    // every constituent commit's full message — each with its own completion
    // trailer — so the body is where the folded child specs' ship signals
    // live. `%B` spans newlines, so switch to `-z` (NUL between commits) and a
    // `%x1f` (unit-separator) field split, keeping the parse unambiguous even
    // when a body contains tabs/newlines. trace:BUG-536 | ai:claude
    let mut log_args: Vec<String> = vec![
        "log".to_string(),
        "-z".to_string(),
        "--pretty=format:%H%x1f%B".to_string(),
    ];
    match pre_sha {
        Some(pre) => log_args.push(format!("{}..HEAD", pre)),
        None => {
            log_args.push("--max-count=50".to_string());
            log_args.push("HEAD".to_string());
        }
    }

    // ── Step 3: walk `git log` for (sha, subject) pairs ──
    let log_out = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_root)
        .args(&log_args)
        .output();
    let log = match log_out {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Ok((Vec::new(), false)),
    };
    let log_str = String::from_utf8_lossy(&log);

    // spec_id → first-seen commit_sha. First commit wins, which matches
    // "when did this spec land" semantics (later commits referencing
    // the same spec are follow-up edits, not the original landing).
    let mut candidates: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    // BUG-102: pr_number → first-seen commit_sha for `(#N)` squash-merge
    // suffixes. Used after the store loads to match review stories filed
    // by /aida-pr's auto-queue (their spec IDs are NEVER in commit
    // subjects, so the `candidates` scan above can't see them).
    let mut pr_to_sha: std::collections::BTreeMap<u64, String> = std::collections::BTreeMap::new();
    for record in log_str.split('\0') {
        let mut parts = record.splitn(2, '\x1f');
        let sha = parts.next().unwrap_or("").trim();
        let message = parts.next().unwrap_or("");
        // The subject is the first non-empty line of `%B`; the helpers below all
        // re-derive it the same way, but compute it once for the empty-guard.
        let subject = message
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        if sha.is_empty() || subject.is_empty() {
            continue;
        }
        // BUG-426: a plan commit (`docs(plans): …`) names the specs it PLANS
        // for in its trailing `(SPEC-ID …)` group, but a plan is a
        // pre-implementation artifact — the plan template leaves those specs
        // at Approved ("Plan only — not the implementation; specs stay
        // Approved"). Honoring that trailer as a completion signal
        // false-completes the planned (often umbrella) specs off a plan-only
        // commit; verified against the store, both historic `docs(plans):`
        // completions were false. Skip plan commits from the
        // completion-candidate scan. They still surface in git-linkage display
        // via `extract_spec_ids_from_commit`, and a plan PR that is genuinely
        // reviewed + merged still completes via its review-story covers path.
        // trace:BUG-426 | ai:claude
        // BUG-2134: a merge commit whose body carries a BUG-1919
        // non-completing-slice marker (a PR merged outside `aida pr ship`, so
        // its title trailer survived into the subject) completes no spec,
        // whatever status the spec is in — the same rule ship applies.
        // trace:BUG-2134 | ai:claude
        if !is_plan_commit_subject(subject) && !commit_declares_non_completing(message) {
            for spec_id in extract_spec_ids_from_commit(subject) {
                candidates.entry(spec_id).or_insert_with(|| sha.to_string());
            }
            // BUG-536: also harvest the completion trailers GitHub preserves in
            // a squash-merge BODY. When an umbrella PR is squash-merged, only
            // the lead spec reaches the subject trailer; every other spec it
            // covered is left stranded at Done/Approved until a manual edit
            // (observed 3× in one day). The squash body concatenates each
            // constituent commit's message, so its trailing `(SPEC-ID)` groups
            // ARE authoritative ship signals — the same body references that
            // `/aida-pr`'s covers chain (BUG-106) already trusts to complete
            // folded specs, here made to work WITHOUT a pre-recorded review
            // story. Gated on a `(#N)` squash/merge subject so a regular
            // commit's prose body can't mine false completions; the extractor's
            // BUG-412 code-like-line guard and the Step-4 eligibility guard are
            // the backstops. trace:BUG-536 | ai:claude
            if extract_pr_number_from_commit_subject(subject).is_some() {
                for spec_id in extract_referenced_spec_ids_from_commit(message) {
                    candidates.entry(spec_id).or_insert_with(|| sha.to_string());
                }
            }
        }
        if let Some(pr_n) = extract_pr_number_from_commit_subject(subject) {
            pr_to_sha.entry(pr_n).or_insert_with(|| sha.to_string());
        }
    }

    // TASK-1296: a Review-PR spec whose own PR closed WITHOUT merging never
    // produces a `(#N)` merge commit, so it can never show up in `pr_to_sha`
    // — the git-log scan above has no evidence to find. It also can't be
    // deferred behind the `candidates.is_empty() && pr_to_sha.is_empty()`
    // early-out below, since a closed-unmerged PR is precisely the case
    // where both of those stay empty. Load the store unconditionally and
    // ask the forge directly for the handful of Review-PR specs still
    // Approved/InProgress that the local scan didn't already resolve.
    // trace:TASK-1296 | ai:claude
    let store = storage.load()?;
    let (stranded_review_pr, stranded_review_pr_failures) =
        collect_stranded_review_pr_resolutions(&store, &pr_to_sha, project_root);
    report_stranded_review_pr_lookup_failures(&stranded_review_pr_failures);

    // BUG-1506: mirror image of the Done→Completed bump below. A spec that
    // reached main straight from Draft — never triaged, never through the
    // intermediate states — is invisible to `auto_bump_eligible_status`
    // (Draft is deliberately excluded from `git_merge_completes`, BUG-328),
    // so it was never bumped at all and sat reading Draft indefinitely. Land
    // it at Done instead: visible, off the open-backlog shelf, one human
    // confirmation short of Completed. trace:BUG-1506 | ai:claude
    let draft_landed = collect_draft_landed_candidates(project_root, &store, &candidates, None);
    if !draft_landed.is_empty() {
        if let Ok(confirmed) = apply_draft_to_done_bumps(project_root, storage, &draft_landed) {
            if !confirmed.is_empty() {
                println!(
                    "  {} {} Draft spec{} → {} (trailered commit already on the default \
                     branch; skipped intermediate states — confirm before closing)",
                    "auto-bumped".cyan(),
                    confirmed.len(),
                    if confirmed.len() == 1 { "" } else { "s" },
                    "Done".yellow().bold()
                );
                for (spec_id, sha) in &confirmed {
                    let short = if sha.len() >= 7 {
                        &sha[..7]
                    } else {
                        sha.as_str()
                    };
                    println!(
                        "    {} {}",
                        spec_id.bold(),
                        format!("(commit {})", short).dimmed()
                    );
                }
            }
        }
    }

    // BUG-1551: held specs whose blockers have since resolved.
    let released_holds = collect_released_closure_holds(&store);

    // BUG-1768: resolving a finding whose parent completed outside this pass is
    // work this pass must still do, so it has to count as a reason to continue
    // past the nothing-to-do guard. trace:BUG-1768 | ai:claude
    let orphaned_finding_pending = has_orphaned_failure_finding(&store);

    if candidates.is_empty()
        && pr_to_sha.is_empty()
        && stranded_review_pr.is_empty()
        && released_holds.is_empty()
        && !orphaned_finding_pending
    {
        return Ok((Vec::new(), true));
    }

    // ── Step 4: figure out which candidates are eligible to ship ──
    let mut flips: Vec<AutoBumpFlip> = Vec::new();
    for (spec_id, sha) in &candidates {
        let Some(req) = trailer_spec_or_skip(&store, spec_id, sha) else {
            continue;
        };
        if !auto_bump_eligible_status(&req.status) {
            continue;
        }
        flips.push(AutoBumpFlip::new(
            spec_id.clone(),
            sha.clone(),
            req.status.clone(),
        ));
    }
    // BUG-102: also scan the store for Done-status review stories whose
    // title encodes a PR number that just merged. Composes the two
    // designs (STORY-66/90 auto-queue review story, STORY-86 auto-bump
    // by commit-subject scan) that previously didn't talk to each other.
    if !pr_to_sha.is_empty() {
        for req in &store.requirements {
            if !matches!(req.status, RequirementStatus::Done) {
                continue;
            }
            let Some(pr_n) = parse_review_story_pr_number(&req.title) else {
                continue;
            };
            let Some(sha) = pr_to_sha.get(&pr_n) else {
                continue;
            };
            let Some(spec_id) = req.spec_id.as_deref() else {
                continue;
            };
            if flips.iter().any(|f| f.spec_id == spec_id) {
                continue;
            }
            flips.push(AutoBumpFlip::new(
                spec_id.to_string(),
                sha.clone(),
                req.status.clone(),
            ));
        }
    }
    // BUG-106: a cluster-mode PR squash-merges with the PR TITLE as the
    // commit subject — the *covered* specs' IDs never reach main's
    // commit log, so the `candidates` scan above matches none of them.
    // For each merged `(#N)` PR, find its auto-queued review story and
    // bump every Done spec the story `implements` (the 'covers' list
    // recorded by /aida-pr's auto-queue). The BUG-102 block above bumps
    // the review STORY; this block bumps the implementer SPECS it
    // covers. PRs with no review story just fall through.
    // trace:BUG-106 | ai:claude
    if !pr_to_sha.is_empty() {
        for (pr_n, sha) in &pr_to_sha {
            let Some(review_story) = store
                .requirements
                .iter()
                .find(|r| parse_review_story_pr_number(&r.title) == Some(*pr_n))
            else {
                continue;
            };
            for rel in &review_story.relationships {
                if !matches!(
                    &rel.rel_type,
                    aida_core::RelationshipType::Custom(n) if n.eq_ignore_ascii_case("implements")
                ) {
                    continue;
                }
                let Some(covered) = store.requirements.iter().find(|r| r.id == rel.target_id)
                else {
                    continue;
                };
                if !auto_bump_eligible_status(&covered.status) {
                    continue;
                }
                let Some(spec_id) = covered.spec_id.as_deref() else {
                    continue;
                };
                if flips.iter().any(|f| f.spec_id == spec_id) {
                    continue;
                }
                flips.push(AutoBumpFlip::new(
                    spec_id.to_string(),
                    sha.clone(),
                    covered.status.clone(),
                ));
            }
        }
    }

    // BUG-113: a `Done` review story whose covered specs have merged
    // (`Completed`, or being completed in this very pass) but whose `(#N)`
    // merge commit is no longer in the scan window — typically because the
    // story flipped to `Done` *after* the merge-carrying pull. The covers
    // chain graduates it without depending on `pr_to_sha`. Runs after the
    // candidate + BUG-102 + BUG-106 blocks so a covered spec flipped in
    // this same pass is already visible in `flips`. trace:BUG-113 | ai:claude
    retain_fresh_auto_bump_flips(project_root, &store, &mut flips);
    for flip in collect_covers_completed_review_flips(&store, &flips) {
        flips.push(flip);
    }
    // BUG-1551: release held specs whose blockers resolved since the merge.
    // Added before the open-PR guard so a newly-opened PR still defers them.
    // trace:BUG-1551 | ai:claude
    for flip in released_holds {
        if !flips.iter().any(|f| f.spec_id == flip.spec_id) {
            flips.push(flip);
        }
    }

    // BUG-1454: a trailer only completes the spec when this merge finishes
    // it. If another open PR references the same spec, the current merge is
    // only one deliverable: preserve Done so every open-work surface keeps the
    // remainder visible. An unavailable GitHub lookup is ambiguous and also
    // preserves Done; a later pull/reconcile can complete it once the forge is
    // reachable and no open PR remains. trace:BUG-1454 | ai:codex
    retain_fresh_auto_bump_flips(project_root, &store, &mut flips);
    let candidate_ids = flips.iter().map(|flip| flip.spec_id.clone());
    match specs_with_open_prs(project_root, candidate_ids) {
        Some(open) => {
            flips.retain(|flip| {
                let Some(pr) = open.get(&flip.spec_id) else {
                    return true;
                };
                eprintln!(
                    "  {}",
                    open_pr_withheld_message(&flip.spec_id, &flip.prior_status, *pr)
                );
                false
            });
        }
        None if !flips.is_empty() => {
            eprintln!(
                "  {} auto-bump deferred — could not verify whether candidate specs have open PRs",
                "↷".yellow()
            );
            flips.clear();
        }
        None => {}
    }

    // BUG-1551: BlockedBy gates closure, not just pickup. A spec whose code
    // merged while a BlockedBy predecessor is still unresolved stays at Done
    // with a note recording the merge, instead of silently completing past
    // the dependency. trace:BUG-1551 | ai:claude
    let closure_holds = split_closure_held_flips(&store, &mut flips);
    report_closure_holds(&closure_holds);

    // TASK-246 / BUG-219: a review story whose PR merged before the
    // review lifecycle finished — left at `InProgress` (a reviewer asked
    // for fixups, then the PR self-merged instead of a fresh /aida-review
    // pass, TASK-246) or at `Approved` (a reviewer session was never
    // spawned at all: self-merge, or the `--auto-complete` orchestrator
    // skipped phase 3, BUG-219). Auto-bump otherwise only handles
    // Done → Completed and the reviewer skill never re-runs, so the
    // `(#N)` merge commit landing on the default branch is the
    // authoritative "review is over" signal. Flip each to Completed with
    // an audit comment naming why review was skipped.
    // trace:TASK-246 trace:BUG-219 | ai:claude
    let mut stale_review_flips = collect_stale_review_story_flips(&store, &pr_to_sha, &flips);
    retain_fresh_stale_review_flips(project_root, &store, &mut stale_review_flips);

    // BUG-1768: the second nothing-to-do guard. A pass with an orphaned finding
    // to resolve and nothing to flip reaches here, so this one has to admit the
    // same reason to continue as the guard above it. trace:BUG-1768 | ai:claude
    let has_flip_work = !(flips.is_empty()
        && stale_review_flips.is_empty()
        && stranded_review_pr.is_empty()
        && closure_holds.is_empty());

    if !has_flip_work && !orphaned_finding_pending {
        return Ok((Vec::new(), true));
    }

    // ── Step 5: write the flips ──
    //
    // BUG-1768: gated on there actually being flips. Before this bug the guard
    // above made that implicit — the only way to reach here was with work to
    // write. An orphaned finding is now also a way to reach here, and on the
    // legacy store path this block's `update_atomically` rewrites the whole
    // store whether or not its closure changes a single spec, so an ungated
    // Step 5 would turn "reject one finding" into a full-store write, plus a
    // `GitBackend` open on the canonical path. trace:BUG-1768 | ai:claude
    let now = chrono::Utc::now();
    if has_flip_work && store_path.is_dir() {
        // Git-canonical store: targeted per-spec writes — read the ONE spec,
        // apply the flip, commit its one YAML with subject `update SPEC-ID`
        // (the same BUG-634 targeted path `aida edit` uses via
        // `GitBackend::update_requirement`). Replaces the old full-store
        // `update_atomically` save, which produced bulk "chore: update N
        // requirements" commits and carried the stale-full-save overwrite
        // risk. `get_requirement_by_spec_id` resolves agreed_id forms too
        // (the TASK-1-113/BUG-405 concern), and each fresh per-spec read is
        // the re-check-inside-the-window the atomic closure used to do.
        // trace:TASK-1161 | ai:claude
        use aida_core::db::DatabaseBackend;
        let backend = aida_core::db::GitBackend::new(store_path)?;
        for flip in &flips {
            // Plain lookup: a checked one on a bare GitBackend reloads the whole
            // store per spec. Trailer ids were vetted by trailer_spec_or_skip.
            // trace:TASK-1468 | ai:claude
            let Some(mut r) = backend.get_requirement_by_spec_id(&flip.spec_id)? else {
                continue;
            };
            if apply_auto_bump_flip(&mut r, flip, now, project_root) {
                backend.update_requirement(&r)?;
            }
        }
        // BUG-1551: closure holds — same targeted write, one commit each.
        for hold in &closure_holds {
            // Plain lookup: a checked one on a bare GitBackend reloads the whole
            // store per spec. Trailer ids were vetted by trailer_spec_or_skip.
            // trace:TASK-1468 | ai:claude
            let Some(mut r) = backend.get_requirement_by_spec_id(&hold.flip.spec_id)? else {
                continue;
            };
            if apply_closure_hold(&mut r, hold, now, project_root) {
                backend.update_requirement(&r)?;
                // STORY-1436: first time this merge is held — record the
                // non-completion (deduped with the audit note, so a re-pull
                // of the same held merge does not re-count it).
                // trace:STORY-1436 | ai:claude
                record_closure_hold_event(project_root, hold);
            }
        }
        // TASK-246 / BUG-219: review stories whose PR merged before the
        // review lifecycle finished — same targeted write, one commit each.
        for (spec_id, sha, pr_n, _) in &stale_review_flips {
            // Plain lookup: a checked one on a bare GitBackend reloads the whole
            // store per spec. These ids come from the loaded store's own rows.
            // trace:TASK-1468 | ai:claude
            let Some(mut r) = backend.get_requirement_by_spec_id(spec_id)? else {
                continue;
            };
            if apply_stale_review_flip(&mut r, sha, *pr_n, now, project_root) {
                backend.update_requirement(&r)?;
            }
        }
        // TASK-1296 / BUG-1543: review stories stranded Draft/Approved/InProgress
        // whose PR reached a terminal state the git-log scan above couldn't see
        // (closed without merging, or merged outside the scan window).
        for resolution in &stranded_review_pr {
            // Plain lookup: a checked one on a bare GitBackend reloads the whole
            // store per spec. These ids come from the loaded store's own rows.
            // trace:TASK-1468 | ai:claude
            let Some(mut r) = backend.get_requirement_by_spec_id(&resolution.spec_id)? else {
                continue;
            };
            if apply_stranded_review_pr_resolution(&mut r, resolution, now) {
                backend.update_requirement(&r)?;
            }
        }
    } else if has_flip_work {
        // Legacy YAML/SQLite store: keep the atomic full-store write.
        let flips_for_write = flips.clone();
        let stale_for_write = stale_review_flips.clone();
        let stranded_for_write = stranded_review_pr.clone();
        let holds_for_write = closure_holds.clone();
        storage.update_atomically(|s| {
            // BUG-1551: closure holds, atomic-store path.
            for hold in &holds_for_write {
                if let Some(r) = s.requirements.iter_mut().find(|r| {
                    r.spec_id.as_deref() == Some(hold.flip.spec_id.as_str())
                        || r.agreed_id.as_deref() == Some(hold.flip.spec_id.as_str())
                }) {
                    apply_closure_hold(r, hold, now, project_root);
                }
            }
            for flip in &flips_for_write {
                // TASK-1-113: match agreed_id as well as spec_id — the
                // eligibility scan above resolves via the agreed-aware
                // `get_requirement_by_spec_id`, so a node-aware spec whose
                // commit subject carries the agreed_id (e.g. `(BUG-42)` for
                // canonical `BUG-1-099`) must re-find by the same key here or
                // the flip is silently dropped. Mirrors the reconcile-status
                // fix. trace:BUG-405 | ai:claude
                if let Some(r) = s.requirements.iter_mut().find(|r| {
                    r.spec_id.as_deref() == Some(flip.spec_id.as_str())
                        || r.agreed_id.as_deref() == Some(flip.spec_id.as_str())
                }) {
                    apply_auto_bump_flip(r, flip, now, project_root);
                }
            }
            // TASK-246 / BUG-219: review stories whose PR merged before the
            // review lifecycle finished. The helper re-checks the status
            // inside the atomic window — keeps this idempotent (a second
            // `aida pull` sees Completed and skips).
            for (spec_id, sha, pr_n, _) in &stale_for_write {
                if let Some(r) = s
                    .requirements
                    .iter_mut()
                    .find(|r| r.spec_id.as_deref() == Some(spec_id.as_str()))
                {
                    apply_stale_review_flip(r, sha, *pr_n, now, project_root);
                }
            }
            // TASK-1296: same stranded-review-pr write, atomic-store path.
            for resolution in &stranded_for_write {
                if let Some(r) = s
                    .requirements
                    .iter_mut()
                    .find(|r| r.spec_id.as_deref() == Some(resolution.spec_id.as_str()))
                {
                    apply_stranded_review_pr_resolution(r, resolution, now);
                }
            }
        })?;
    }

    // Filter the report to only specs we actually still flipped after
    // the inside-the-atomic-window re-check. Cheapest correct answer:
    // re-load and intersect. In the common case the lists are equal.
    let after = storage.load().unwrap_or_else(|_| store.clone());
    let confirmed: Vec<AutoBumpFlip> = flips
        .into_iter()
        .filter(|flip| {
            after
                .get_requirement_by_spec_id(&flip.spec_id)
                .map(|r| matches!(r.status, RequirementStatus::Completed))
                .unwrap_or(false)
        })
        .collect();

    // TASK-1296: report the stranded review-pr specs that flipped Completed
    // or Rejected because their PR reached a terminal state on the forge.
    // trace:TASK-1296 | ai:claude
    let confirmed_stranded: Vec<&StrandedReviewPrResolution> = stranded_review_pr
        .iter()
        .filter(|resolution| {
            let expect = match resolution.outcome {
                StrandedReviewPrOutcome::Merged => RequirementStatus::Completed,
                StrandedReviewPrOutcome::ClosedUnmerged => RequirementStatus::Rejected,
            };
            after
                .get_requirement_by_spec_id(&resolution.spec_id)
                .map(|r| r.status == expect)
                .unwrap_or(false)
        })
        .collect();
    if !confirmed_stranded.is_empty() {
        let completed_n = confirmed_stranded
            .iter()
            .filter(|r| r.outcome == StrandedReviewPrOutcome::Merged)
            .count();
        let rejected_n = confirmed_stranded.len() - completed_n;
        if completed_n > 0 {
            println!(
                "  {} {} review stor{} → {} (PR merged on the forge)",
                "auto-completed".cyan(),
                completed_n,
                if completed_n == 1 { "y" } else { "ies" },
                "Completed".green().bold()
            );
        }
        if rejected_n > 0 {
            println!(
                "  {} {} review stor{} → {} (PR closed without merging)",
                "auto-rejected".cyan(),
                rejected_n,
                if rejected_n == 1 { "y" } else { "ies" },
                "Rejected".red().bold()
            );
        }
        for resolution in &confirmed_stranded {
            println!(
                "    {} {}",
                resolution.spec_id.bold(),
                format!("(PR #{})", resolution.pr_n).dimmed()
            );
        }
    }
    for resolution in &confirmed_stranded {
        let action = match resolution.outcome {
            StrandedReviewPrOutcome::Merged => "auto-completed",
            StrandedReviewPrOutcome::ClosedUnmerged => "auto-rejected",
        };
        record_role_activity(&resolution.spec_id, action);
    }

    // TASK-246 / BUG-219: report the review stories that flipped to
    // Completed because their PR merged before review finished. Kept
    // separate from `confirmed` so the Done→Completed summary count
    // stays accurate; printed here so every caller (`aida pull`,
    // `aida db sync --pull`) surfaces it uniformly.
    // trace:TASK-246 trace:BUG-219 | ai:claude
    let confirmed_stale: Vec<&(String, String, u64, RequirementStatus)> = stale_review_flips
        .iter()
        .filter(|(spec_id, _, _, _)| {
            after
                .get_requirement_by_spec_id(spec_id)
                .map(|r| matches!(r.status, RequirementStatus::Completed))
                .unwrap_or(false)
        })
        .collect();
    if !confirmed_stale.is_empty() {
        let n = confirmed_stale.len();
        println!(
            "  {} {} review stor{} → {} (PR merged before review finished)",
            "auto-completed".cyan(),
            n,
            if n == 1 { "y" } else { "ies" },
            "Completed".green().bold()
        );
        for (spec_id, _, pr_n, prior) in &confirmed_stale {
            println!(
                "    {} {}",
                spec_id.bold(),
                format!("(PR #{}, was {})", pr_n, prior).dimmed()
            );
        }
    }
    for (spec_id, _, _, _) in &confirmed_stale {
        record_role_activity(spec_id, "auto-completed");
    }

    // BUG-1286: one terminal record per completed spec, including every
    // member named by a multi-spec merge trailer.
    for flip in &confirmed {
        emit_spec_completed(project_root, &flip.spec_id, &flip.sha, None, "auto-bump");
    }
    for (spec_id, sha, pr_n, _) in &confirmed_stale {
        emit_spec_completed(project_root, spec_id, sha, Some(*pr_n), "auto-bump");
    }
    for resolution in &confirmed_stranded {
        if resolution.outcome == StrandedReviewPrOutcome::Merged {
            emit_spec_completed(
                project_root,
                &resolution.spec_id,
                "",
                Some(resolution.pr_n),
                "auto-bump",
            );
        }
    }

    // BUG-1529: a spec that just flipped Done → Completed here may carry a
    // stale "changes requested" / "rejected" verdict from a round that was
    // refused, reworked, and — since it just landed — evidently addressed.
    // Nothing else ever recorded that the refusal was answered, so the
    // verdict file would say REFUSED forever even though the work shipped.
    // Close it out now, at the same moment the merge is detected, rather than
    // leaving a permanent false positive for every reader of the verdict
    // corpus. Best-effort like the `emit_spec_completed` calls above: a
    // missing or unwritable verdict file must never fail the bump itself.
    // trace:BUG-1529 | ai:claude
    for flip in &confirmed {
        let _ = review_verdict::close_verdict_on_merge(project_root, &flip.spec_id, &flip.sha);
    }
    for (spec_id, sha, _, _) in &confirmed_stale {
        let _ = review_verdict::close_verdict_on_merge(project_root, spec_id, sha);
    }
    for resolution in &confirmed_stranded {
        if resolution.outcome == StrandedReviewPrOutcome::Merged {
            let _ = review_verdict::close_verdict_on_merge(
                project_root,
                &resolution.spec_id,
                &format!("PR-{}", resolution.pr_n),
            );
        }
    }

    // STORY-1421: the same Done→Completed moment is where an outstanding
    // non-blocking finding on an APPROVED verdict would otherwise lose its
    // last reader — carry it into `aida findings` right here, alongside the
    // BUG-1529 verdict-closing pass above.
    // trace:STORY-1421 | ai:claude
    for flip in &confirmed {
        emit_nonblocking_findings_on_completion(
            project_root,
            store_path,
            &flip.spec_id,
            &flip.sha,
            None,
        );
    }
    for (spec_id, sha, pr_n, _) in &confirmed_stale {
        emit_nonblocking_findings_on_completion(
            project_root,
            store_path,
            spec_id,
            sha,
            Some(*pr_n),
        );
    }
    for resolution in &confirmed_stranded {
        if resolution.outcome == StrandedReviewPrOutcome::Merged {
            emit_nonblocking_findings_on_completion(
                project_root,
                store_path,
                &resolution.spec_id,
                "",
                Some(resolution.pr_n),
            );
        }
    }

    // ── Step 6: activity log ──
    for flip in &confirmed {
        record_role_activity(&flip.spec_id, "auto-completed");
    }

    let mut completed_events: Vec<(String, String)> = confirmed
        .iter()
        .map(|f| (f.spec_id.clone(), f.sha.clone()))
        .collect();
    completed_events.extend(
        confirmed_stale
            .iter()
            .map(|(spec_id, sha, _, _)| (spec_id.clone(), sha.clone())),
    );
    let auto_resolved_failures =
        auto_resolve_failure_bugs_for_completed_specs(&after, &completed_events, project_root);
    if !auto_resolved_failures.is_empty() {
        if store_path.is_dir() {
            use aida_core::db::DatabaseBackend;
            let backend = aida_core::db::GitBackend::new(store_path)?;
            for (finding_id, completed_spec, completion_ref) in &auto_resolved_failures {
                // Plain lookup: a checked one on a bare GitBackend reloads the whole
                // store per spec. These ids come from the loaded store's own rows.
                // trace:TASK-1468 | ai:claude
                let Some(mut r) = backend.get_requirement_by_spec_id(finding_id)? else {
                    continue;
                };
                if reject_resolved_auto_complete_failure_bug(
                    &mut r,
                    completed_spec,
                    completion_ref,
                    now,
                ) {
                    backend.update_requirement(&r)?;
                }
            }
        } else {
            let resolved_for_write = auto_resolved_failures.clone();
            storage.update_atomically(|s| {
                for (finding_id, completed_spec, completion_ref) in &resolved_for_write {
                    if let Some(r) = s.requirements.iter_mut().find(|r| {
                        r.spec_id.as_deref() == Some(finding_id.as_str())
                            || r.agreed_id.as_deref() == Some(finding_id.as_str())
                    }) {
                        reject_resolved_auto_complete_failure_bug(
                            r,
                            completed_spec,
                            completion_ref,
                            now,
                        );
                    }
                }
            })?;
        }
        println!(
            "  {} {} auto-complete failure finding{} → {}",
            "auto-resolved".cyan(),
            auto_resolved_failures.len(),
            if auto_resolved_failures.len() == 1 {
                ""
            } else {
                "s"
            },
            "Rejected".green().bold()
        );
    }

    // ── Step 7: plan followups ── TASK-96: file followup TASKs for each
    // just-merged spec. Non-interactive (this runs inside `aida pull`);
    // idempotent via the followups marker, so specs already handled at
    // `aida queue done` time are skipped. Best-effort.
    // trace:TASK-96 | ai:claude
    if extract_followups {
        for flip in &confirmed {
            let _ =
                extract_plan_followups(storage, project_root, &flip.spec_id, &flip.spec_id, false);
        }
    }

    Ok((confirmed, true))
}

/// TASK-226: manual replay of the Done → Completed scan over a wider
/// range than `aida pull` saw. Used to recover specs stranded at Done
/// when:
/// - the YAML was unreadable at pull time (BUG-96 deletion);
/// - the spec was flipped to Done after the referencing commit was
///   already on local main;
/// - the user cold-cloned and never had a pull moment where both the
///   spec and the commit were visible together.
///
/// The handler reuses `auto_bump_done_to_completed`'s pure helpers
/// (extract_spec_ids_from_commit + extract_pr_number_from_commit_subject
/// + parse_review_story_pr_number) but does its own scan because the
///   pull-time helper hard-codes pre_sha-or-HEAD~50 ranges. Here we accept
///   `--since REF` for a bounded replay or default to a 200-commit window.
///   `--spec SPEC-ID` narrows the candidate set to a single requirement.
///   `--dry-run` previews without writing.
///   trace:TASK-226 | ai:claude
/// BUG-418: pick the message `reconcile-status` prints when nothing flipped.
/// The misleading case it fixes: a referencing commit IS on the default branch
/// but the spec is already `Completed` (a prior reconcile/pull graduated it).
/// The old generic "no commit references it" text read as "recovery failed" to
/// an operator who had in fact already succeeded. When `already_terminal` is
/// non-empty we say "already Completed — nothing to do"; otherwise we fall back
/// to the genuine no-match guidance. `already_terminal` carries `(spec_id,
/// status)` for each candidate whose referencing commit landed but is past an
/// eligible status. trace:BUG-418 | ai:claude
pub(crate) fn reconcile_no_flip_message(
    spec: Option<&str>,
    already_terminal: &[(String, RequirementStatus)],
) -> String {
    if !already_terminal.is_empty() {
        return match spec {
            Some(s) => {
                let status = already_terminal
                    .iter()
                    .find(|(id, _)| id.eq_ignore_ascii_case(s))
                    .map(|(_, st)| st.clone())
                    .unwrap_or(RequirementStatus::Completed);
                format!(
                    "Nothing to reconcile for {}: it is already {} (a \
                     referencing commit is on the default branch — a prior \
                     reconcile or pull already graduated it).",
                    s, status
                )
            }
            None => {
                let n = already_terminal.len();
                format!(
                    "Nothing to reconcile: {} spec{} referenced by commits in \
                     the scan range {} already Completed (a prior reconcile or \
                     pull graduated them); no other eligible spec had a \
                     referencing commit.",
                    n,
                    if n == 1 { "" } else { "s" },
                    if n == 1 { "is" } else { "are" },
                )
            }
        };
    }
    match spec {
        Some(s) => format!(
            "No eligible flips for {}. Either it's not at a status the replay \
             can graduate (Approved, Planned, In Progress, Done, or an \
             Approved/In-Progress review story), or no commit in the scan \
             range references it.",
            s
        ),
        None => "No eligible flips. All eligible specs in the store either have \
                 no referencing commit in the scan range, or are already \
                 Completed/Rejected/Draft."
            .to_string(),
    }
}

/// Counts candidate spec ids that appear in `haystack` as WHOLE ids
/// (case-insensitive): `BUG-1` must not match inside `BUG-10` or `XBUG-1`.
// trace:TASK-1446 | ai:claude
pub(crate) fn count_ids_mentioned(haystack: &str, candidate_ids: &[String]) -> usize {
    let hay = haystack.to_ascii_lowercase();
    let is_id_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    candidate_ids
        .iter()
        .filter(|id| {
            let needle = id.to_ascii_lowercase();
            if needle.is_empty() {
                return false;
            }
            hay.match_indices(&needle).any(|(at, m)| {
                let before = hay[..at].chars().next_back();
                let after = hay[at + m.len()..].chars().next();
                !before.is_some_and(is_id_char) && !after.is_some_and(is_id_char)
            })
        })
        .count()
}

/// TASK-1446 (BUG-1506 AC3): pure arithmetic for the reconcile-status sweep's
/// "direction A" summary count — how many candidates in the scanned window
/// are pre-Done (Draft, or one of Approved/Planned/InProgress about to flip
/// straight to Completed, or a stale review story flipping the same way)
/// with an already-merged trailered commit. Split out from
/// `handle_db_reconcile_status` so the count is unit-testable without a git
/// fixture. `Done`/`NeedsAttention` prior statuses are deliberately excluded
/// — "pre-Done" means strictly before `Done` in the pipeline.
// trace:TASK-1446 | ai:claude
pub(crate) fn count_pre_done_merged(
    draft_landed: usize,
    stale_review_flips: usize,
    flip_prior_statuses: impl Iterator<Item = RequirementStatus>,
) -> usize {
    draft_landed
        + stale_review_flips
        + flip_prior_statuses
            .filter(|s| {
                matches!(
                    s,
                    RequirementStatus::Approved
                        | RequirementStatus::Planned
                        | RequirementStatus::InProgress
                )
            })
            .count()
}

pub(crate) fn handle_db_reconcile_status(
    store_path: &std::path::Path,
    since: Option<&str>,
    spec: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    use std::process::Command as ProcessCommand;

    let project_root = store_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("could not derive project root from store path"))?;

    let default_branch: Option<String> = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8(o.stdout)
                .ok()
                .map(|s| s.trim().to_string())
                .and_then(|s| s.strip_prefix("origin/").map(|x| x.to_string()))
        });
    let current = aida_core::git_ops::current_branch(project_root).ok();
    let default_branch = default_branch.or_else(|| match current.as_deref() {
        Some("main") | Some("master") => current.clone(),
        _ => None,
    });
    let Some(default_branch) = default_branch else {
        anyhow::bail!(
            "could not detect default branch (no `origin/HEAD` symref and not on main/master) — \
             reconcile-status only scans the default branch"
        );
    };
    if current.as_deref() != Some(default_branch.as_str()) {
        anyhow::bail!(
            "reconcile-status must run on the default branch ({}). Currently on `{}`. \
             Switch with `git checkout {}` first.",
            default_branch,
            current.as_deref().unwrap_or("(detached)"),
            default_branch
        );
    }

    // BUG-568: the manual replay scans only the LOCAL repo's default branch —
    // same single-repo blind spot as the live auto-bump. In a shared-store
    // multi-repo workspace it silently propagates the miss instead of fixing
    // it, so warn loudly. trace:BUG-568 | ai:claude
    warn_multi_repo_scan_limited(project_root, "reconcile-status scan");

    // TASK-1335: pin the scanned tip once. Every candidate reason refers to
    // this snapshot, never to a HEAD reread after the branch moved.
    // trace:TASK-1335 | ai:claude
    let head_oid = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--verify", "HEAD^{commit}"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|oid| git_arg_guard::is_hex_sha(oid))
        .ok_or_else(|| anyhow::anyhow!("could not resolve the default branch tip (HEAD)"))?;
    let scan = reconcile_evidence::ScanRange {
        branch: default_branch.clone(),
        oid: head_oid.clone(),
        since: since.map(str::to_string),
        max_count: 200,
    };

    // BUG-536: full-message (`%B`) scan, NUL-record-separated, so the manual
    // replay sees the same squash-body completion trailers the live auto-bump
    // now reads — otherwise `reconcile-status` couldn't recover the umbrella
    // children that prompted this fix (their trailer is only in a merge BODY).
    // trace:BUG-536 | ai:claude
    let mut log_args: Vec<String> = vec![
        "log".to_string(),
        "-z".to_string(),
        "--pretty=format:%H%x1f%B".to_string(),
    ];
    match since {
        Some(s) => {
            // A dash-led `--since` would otherwise reach git as an option
            // (`--output=<path>..HEAD` writes a file). trace:BUG-1622 | ai:claude
            git_arg_guard::reject_option_like("--since", s)?;
            log_args.push(git_arg_guard::END_OF_OPTIONS.to_string());
            log_args.push(format!("{}..{}", s, head_oid));
            log_args.push("--".to_string());
        }
        None => {
            log_args.push(format!("--max-count={}", scan.max_count));
            log_args.push(head_oid.clone());
        }
    }
    let log_out = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_root)
        .args(&log_args)
        .output();
    let log = match log_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(o) => anyhow::bail!(
            "git log failed (resolving `--since {}`?): {}",
            since.unwrap_or("<auto>"),
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => anyhow::bail!("git log failed: {}", e),
    };

    let mut candidates: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    let mut pr_to_sha: std::collections::BTreeMap<u64, String> = std::collections::BTreeMap::new();
    // TASK-1335: which accepted form first supplied each candidate, for the
    // per-candidate report. Attribution itself is unchanged.
    let mut candidate_sources: std::collections::BTreeMap<
        String,
        reconcile_evidence::EvidenceSource,
    > = std::collections::BTreeMap::new();
    for record in log.split('\0') {
        let mut parts = record.splitn(2, '\x1f');
        let sha = parts.next().unwrap_or("").trim();
        let message = parts.next().unwrap_or("");
        let subject = message
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        if sha.is_empty() || subject.is_empty() {
            continue;
        }
        // BUG-426: skip plan commits — their trailer names the planned specs,
        // not delivered ones. Mirrors the gate in `auto_bump_done_to_completed`
        // so the manual reconcile replay can't false-complete a planned spec
        // the live pull already (correctly) left alone. trace:BUG-426 | ai:claude
        // BUG-2134: and a non-completing-slice merge, as the live scan does.
        // trace:BUG-2134 | ai:claude
        if !is_plan_commit_subject(subject) && !commit_declares_non_completing(message) {
            for id in extract_spec_ids_from_commit(subject) {
                if let std::collections::btree_map::Entry::Vacant(slot) = candidates.entry(id) {
                    candidate_sources.insert(
                        slot.key().clone(),
                        reconcile_evidence::EvidenceSource::SubjectTrailer,
                    );
                    slot.insert(sha.to_string());
                }
            }
            // BUG-536: harvest the squash-body completion trailers too (gated on
            // a `(#N)` squash/merge subject), mirroring the live auto-bump scan
            // so a manual replay recovers umbrella children stranded by a
            // single-trailer squash. trace:BUG-536 | ai:claude
            if extract_pr_number_from_commit_subject(subject).is_some() {
                for id in extract_referenced_spec_ids_from_commit(message) {
                    if let std::collections::btree_map::Entry::Vacant(slot) = candidates.entry(id) {
                        candidate_sources.insert(
                            slot.key().clone(),
                            reconcile_evidence::EvidenceSource::SquashBodyTrailer,
                        );
                        slot.insert(sha.to_string());
                    }
                }
            }
        }
        if let Some(pr_n) = extract_pr_number_from_commit_subject(subject) {
            pr_to_sha.entry(pr_n).or_insert_with(|| sha.to_string());
        }
    }

    let storage = Storage::new(store_path);
    let store = storage.load()?;

    // BUG-1506: pre-Done specs — specifically Draft — whose trailered commit
    // is already in the scan range. This is the manual-replay twin of the
    // draft-landing pass in the live pull-time scanner: same eligibility
    // model, same `Done` landing (not `Completed`), so an operator recovering
    // a stranded Draft with a wider `--since` window gets the same outcome a
    // fresh pull would have given it at merge time. trace:BUG-1506 | ai:claude
    let draft_landed = collect_draft_landed_candidates(project_root, &store, &candidates, spec);

    // Build the planned-flip list. For --spec, we narrow to that one
    // candidate (matched against either spec_id or review-story title).
    let mut flips: Vec<AutoBumpFlip> = Vec::new();
    // BUG-418: track specs whose referencing commit IS in the scan range but
    // that are already terminal (Completed/Released) — they're correctly
    // skipped, but the generic "no eligible flips" message reads to an
    // operator as "recovery failed" when the spec was in fact already
    // recovered (a prior run / pull / dry-run-then-pull bumped it). Remember
    // them so we can print an explicit "already Completed — nothing to do"
    // message instead of the misleading no-op text. trace:BUG-418 | ai:claude
    let mut already_terminal: Vec<(String, RequirementStatus)> = Vec::new();
    // TASK-1335: every candidate's reason is recorded here BEFORE a filter
    // drops it, so the report never infers "no match" from an empty flip
    // list. trace:TASK-1335 | ai:claude
    let mut outcomes = reconcile_evidence::ReconcileOutcomes::default();
    let outcome_for = |spec_id: &str,
                       sha: &str,
                       source: reconcile_evidence::EvidenceSource,
                       req: Option<&Requirement>,
                       reason: reconcile_evidence::CandidateReason| {
        reconcile_evidence::CandidateOutcome {
            spec_id: spec_id.to_string(),
            identity: req.map(reconcile_evidence::CandidateIdentity::of),
            sha: sha.to_string(),
            source,
            reason,
        }
    };
    for (spec_id, sha) in &candidates {
        if let Some(target) = spec {
            if !spec_id.eq_ignore_ascii_case(target) {
                continue;
            }
        }
        let source = candidate_sources
            .get(spec_id)
            .copied()
            .unwrap_or(reconcile_evidence::EvidenceSource::SubjectTrailer);
        let Some(req) = trailer_spec_or_skip(&store, spec_id, sha) else {
            let reason = if store.get_requirement_unambiguous(spec_id).is_err() {
                reconcile_evidence::CandidateReason::AmbiguousSpec
            } else {
                reconcile_evidence::CandidateReason::UnknownSpec
            };
            outcomes.record(outcome_for(spec_id, sha, source, None, reason));
            continue;
        };
        if !auto_bump_eligible_status(&req.status) {
            if matches!(req.status, RequirementStatus::Completed) {
                already_terminal.push((spec_id.clone(), req.status.clone()));
                outcomes.record(outcome_for(
                    spec_id,
                    sha,
                    source,
                    Some(req),
                    reconcile_evidence::CandidateReason::AlreadyTerminal,
                ));
            } else if !draft_landed.iter().any(|(id, _)| id == spec_id) {
                // A Draft landing at Done is reported by its own path.
                outcomes.record(outcome_for(
                    spec_id,
                    sha,
                    source,
                    Some(req),
                    reconcile_evidence::CandidateReason::Ineligible,
                ));
            }
            continue;
        }
        outcomes.record(outcome_for(
            spec_id,
            sha,
            source,
            Some(req),
            reconcile_evidence::CandidateReason::WouldComplete,
        ));
        flips.push(AutoBumpFlip::new(
            spec_id.clone(),
            sha.clone(),
            req.status.clone(),
        ));
    }
    if !pr_to_sha.is_empty() {
        for req in &store.requirements {
            if !matches!(req.status, RequirementStatus::Done) {
                continue;
            }
            let Some(pr_n) = parse_review_story_pr_number(&req.title) else {
                continue;
            };
            let Some(sha) = pr_to_sha.get(&pr_n) else {
                continue;
            };
            let Some(sid) = req.spec_id.as_deref() else {
                continue;
            };
            if let Some(target) = spec {
                if !sid.eq_ignore_ascii_case(target) {
                    continue;
                }
            }
            if flips.iter().any(|f| f.spec_id == sid) {
                continue;
            }
            outcomes.record(outcome_for(
                sid,
                sha,
                reconcile_evidence::EvidenceSource::ReviewStoryMerge,
                Some(req),
                reconcile_evidence::CandidateReason::WouldComplete,
            ));
            flips.push(AutoBumpFlip::new(
                sid.to_string(),
                sha.clone(),
                req.status.clone(),
            ));
        }
    }

    // BUG-219: review stories stranded short of Completed because their
    // PR merged without the review lifecycle finishing — at `Approved`
    // (a reviewer session was never spawned) or `InProgress` (fixups
    // requested, PR self-merged instead). The pull-time auto-bump catches
    // this live; reconcile-status is the manual replay, so it must handle
    // the same case over its wider scan window. The BUG-102 block above
    // already covers the Done-state review story, so this completes the
    // {Approved, InProgress, Done} set. trace:BUG-219 | ai:claude
    let mut stale_review_flips = collect_stale_review_story_flips(&store, &pr_to_sha, &flips);
    if let Some(target) = spec {
        stale_review_flips.retain(|(sid, ..)| sid.eq_ignore_ascii_case(target));
    }
    // TASK-1335: this independent path gets the same per-candidate record
    // (scan-time UUID included) so its write-seam decision is reportable.
    // trace:TASK-1335 | ai:claude
    for (sid, sha, ..) in &stale_review_flips {
        outcomes.record(outcome_for(
            sid,
            sha,
            reconcile_evidence::EvidenceSource::ReviewStoryMerge,
            store.get_requirement_by_spec_id(sid),
            reconcile_evidence::CandidateReason::WouldComplete,
        ));
    }
    let before_fresh_stale = stale_review_flips.clone();
    retain_fresh_stale_review_flips(project_root, &store, &mut stale_review_flips);
    for (sid, ..) in &before_fresh_stale {
        if !stale_review_flips.iter().any(|(kept, ..)| kept == sid) {
            outcomes.set_reason(sid, reconcile_evidence::CandidateReason::StaleAfterReopen);
        }
    }

    // BUG-113: the covers chain — a `Done` review story whose covered
    // specs have merged (`Completed` in the store, or flipped earlier in
    // this replay) — the window-independent safety net for review stories
    // the `(#N)` scan missed because their PR's merge commit is outside
    // the replay range. Mirrors the auto-bump path; honours `--spec`.
    // trace:BUG-113 | ai:claude
    let mark_stale = |outcomes: &mut reconcile_evidence::ReconcileOutcomes,
                      before: &[AutoBumpFlip],
                      after: &[AutoBumpFlip]| {
        for flip in before {
            if !after.iter().any(|kept| kept.spec_id == flip.spec_id) {
                outcomes.set_reason(
                    &flip.spec_id,
                    reconcile_evidence::CandidateReason::StaleAfterReopen,
                );
            }
        }
    };
    let before_fresh = flips.clone();
    retain_fresh_auto_bump_flips(project_root, &store, &mut flips);
    mark_stale(&mut outcomes, &before_fresh, &flips);
    let mut covers_completed_flips = collect_covers_completed_review_flips(&store, &flips);
    if let Some(target) = spec {
        covers_completed_flips.retain(|f| f.spec_id.eq_ignore_ascii_case(target));
    }
    for flip in covers_completed_flips {
        outcomes.record(outcome_for(
            &flip.spec_id,
            &flip.sha,
            reconcile_evidence::EvidenceSource::CoversChain,
            store.get_requirement_by_spec_id(&flip.spec_id),
            reconcile_evidence::CandidateReason::WouldComplete,
        ));
        flips.push(flip);
    }

    // BUG-1454: replay must enforce the same open-PR guard as pull-time
    // auto-bump. Otherwise a manual Completed → Done recovery would be undone
    // immediately by the already-landed trailer in this wider scan.
    let before_fresh = flips.clone();
    retain_fresh_auto_bump_flips(project_root, &store, &mut flips);
    mark_stale(&mut outcomes, &before_fresh, &flips);

    // TASK-1335: a checked lookup per candidate, against a forge classified
    // WITHOUT repairing config. Any unavailable answer still defers the whole
    // normal batch (all or nothing, as before), but each candidate keeps its
    // own reason: unavailable, open, or checked-clear-but-batch-deferred.
    // trace:TASK-1335 | ai:claude
    let mut forge_class: Option<Result<forge::ForgeKind, String>> = None;
    let open_checks = if flips.is_empty() {
        Vec::new()
    } else {
        let forge_kind = forge_class.get_or_insert_with(|| forge::checked_forge_kind(project_root));
        reconcile_evidence::check_open_changes(
            project_root,
            forge_kind,
            flips.iter().map(|flip| flip.spec_id.clone()),
            false,
        )
    };
    let batch_unavailable = open_checks
        .iter()
        .any(|(_, check)| matches!(check, reconcile_evidence::OpenChangeCheck::Unavailable(_)));
    for (spec_id, check) in &open_checks {
        let reason = match check {
            reconcile_evidence::OpenChangeCheck::Clear if batch_unavailable => {
                reconcile_evidence::CandidateReason::BatchDeferred
            }
            reconcile_evidence::OpenChangeCheck::Clear => continue,
            reconcile_evidence::OpenChangeCheck::Open(pr) => {
                reconcile_evidence::CandidateReason::OpenChange(*pr)
            }
            reconcile_evidence::OpenChangeCheck::Unavailable(why) => {
                reconcile_evidence::CandidateReason::RequiredCheckUnavailable(why.clone())
            }
        };
        outcomes.set_reason(spec_id, reason);
    }
    if batch_unavailable {
        flips.clear();
    } else {
        flips.retain(|flip| {
            !open_checks.iter().any(|(id, check)| {
                id == &flip.spec_id && matches!(check, reconcile_evidence::OpenChangeCheck::Open(_))
            })
        });
    }

    // BUG-1551: the replay honours the same closure gate as the live
    // auto-bump — an unresolved BlockedBy predecessor keeps the spec at Done
    // (merge recorded in a note) rather than completing past it.
    // trace:BUG-1551 | ai:claude
    // TASK-1335: one least-fixed-point closure pass over EVERY completion
    // this run would grant — normal flips and the independent stale-review
    // flips — so the dry-run preview and the live write seam agree. A held
    // normal flip keeps its existing Done landing + note; a held stale
    // review story is simply not completed (no new landing policy).
    // trace:TASK-1335 | ai:claude
    // The covers chain uses the same DIRECT rooted rule as the write seam: a
    // covers candidate whose only covered spec this pass just held loses its
    // support, which can in turn change the closure picture — so the two
    // filters iterate to a fixed point (each pass only removes candidates).
    let mut closure_holds: Vec<ClosureHold> = Vec::new();
    loop {
        let mut all_completions: Vec<AutoBumpFlip> = flips.clone();
        all_completions.extend(stale_review_flips.iter().map(|(sid, sha, _, prior)| {
            AutoBumpFlip::new(sid.clone(), sha.clone(), prior.clone())
        }));
        let mut changed = false;
        for hold in split_closure_held_flips(&store, &mut all_completions) {
            changed = true;
            if stale_review_flips
                .iter()
                .any(|(sid, ..)| *sid == hold.flip.spec_id)
            {
                stale_review_flips.retain(|(sid, ..)| *sid != hold.flip.spec_id);
                outcomes.set_reason(
                    &hold.flip.spec_id,
                    reconcile_evidence::CandidateReason::Held(closure_hold_causes(&hold)),
                );
            } else {
                flips.retain(|flip| flip.spec_id != hold.flip.spec_id);
                outcomes.set_reason(
                    &hold.flip.spec_id,
                    reconcile_evidence::CandidateReason::ClosureHeld,
                );
                closure_holds.push(hold);
            }
        }
        let anchors: std::collections::HashSet<uuid::Uuid> = flips
            .iter()
            .filter(|f| {
                outcomes.get(&f.spec_id).map(|o| o.source)
                    != Some(reconcile_evidence::EvidenceSource::CoversChain)
            })
            .filter_map(|f| store.get_requirement_by_spec_id(&f.spec_id).map(|r| r.id))
            .collect();
        let unsupported: Vec<String> = flips
            .iter()
            .filter(|f| {
                outcomes.get(&f.spec_id).map(|o| o.source)
                    == Some(reconcile_evidence::EvidenceSource::CoversChain)
                    && !covers_chain_rooted(&store, &f.spec_id, &anchors)
            })
            .map(|f| f.spec_id.clone())
            .collect();
        for id in &unsupported {
            changed = true;
            flips.retain(|flip| flip.spec_id != *id);
            outcomes.set_reason(
                id,
                reconcile_evidence::CandidateReason::Held(
                    "no covered spec is Completed or completing in this run".to_string(),
                ),
            );
        }
        if !changed {
            break;
        }
    }
    let targeted = spec.is_some();
    for outcome in outcomes.entries() {
        if let Some(line) = reconcile_evidence::render_outcome(outcome, targeted) {
            eprintln!("  {line}");
        }
    }
    // A dry run reports the scan's holds; a writing run reports the holds the
    // live seam actually decided (below), never an obsolete scan payload.
    if dry_run {
        report_closure_holds(&closure_holds);
    }

    // TASK-1446 (BUG-1506 AC3): the sweep runs — and reports — in BOTH
    // directions, not just the one that produces a flip.
    //
    //   direction A: pre-Done specs (Draft/Approved/Planned/InProgress) whose
    //   trailered commit is already on the default branch — `draft_landed`
    //   plus the eligible `flips`/`stale_review_flips` entries that started
    //   short of `Done`. This is what the rest of this function actually acts
    //   on.
    //
    //   direction B: the mirror image — specs already `Completed` that a
    //   commit in THIS scan window still names, where an open PR still
    //   references them. Nothing flips here (an already-Completed spec is
    //   terminal); it is a pure diagnostic surfacing "shipped, but a PR is
    //   still touching this" for an operator to look at.
    // trace:TASK-1446 | ai:claude
    let pre_done_merged_count = count_pre_done_merged(
        draft_landed.len(),
        stale_review_flips.len(),
        flips.iter().map(|f| f.prior_status.clone()),
    );
    let completed_candidate_ids: Vec<String> = candidates
        .iter()
        .filter(|(spec_id, _)| {
            spec.map(|t| spec_id.eq_ignore_ascii_case(t))
                .unwrap_or(true)
        })
        .filter(|(spec_id, _)| {
            store
                .get_requirement_by_spec_id(spec_id)
                .map(|r| matches!(r.status, RequirementStatus::Completed))
                .unwrap_or(false)
        })
        .map(|(spec_id, _)| spec_id.clone())
        .collect();
    let completed_with_open_pr_count = if completed_candidate_ids.is_empty() {
        Ok(0)
    } else {
        // TASK-1446: ONE `gh pr list` call for every open PR, not one
        // `specs_with_open_prs`-style search per candidate — `specs_with_open_prs`
        // is right-sized for `flips` (already small: only would-flip
        // candidates), but this direction can hold every already-Completed
        // spec a wide `--since`-less (200-commit) scan touches, and a
        // network round trip per candidate was measured to blow well past
        // an operator's `timeout 120` on this repo's own history.
        // TASK-1335: same read-only forge classification as the required
        // check; its failure is labeled as this diagnostic's, never as a
        // completion refusal. trace:TASK-1335 | ai:claude
        let forge_kind = forge_class.get_or_insert_with(|| forge::checked_forge_kind(project_root));
        reconcile_evidence::count_completed_with_open_changes(
            project_root,
            forge_kind,
            &completed_candidate_ids,
        )
    };
    println!(
        "{} sweep: {} pre-Done spec{} with a merged trailer, {} Completed spec{} still \
         referenced by an open PR",
        "↔".cyan(),
        pre_done_merged_count,
        if pre_done_merged_count == 1 { "" } else { "s" },
        completed_with_open_pr_count
            .as_ref()
            .map(|n| n.to_string())
            .unwrap_or_else(|_| "unknown (open-PR diagnostic unavailable)".to_string()),
        if completed_with_open_pr_count == Ok(1) {
            ""
        } else {
            "s"
        },
    );
    if let Err(why) = &completed_with_open_pr_count {
        eprintln!(
            "  {} could not check whether already-Completed specs are still referenced by an \
             open PR: {why}. This diagnostic does not gate completion.",
            "warning:".yellow()
        );
    }

    // TASK-1335: exit matrix. A real candidate that lost a REQUIRED check is
    // an ordinary error (dry-run too); everything else keeps its existing
    // success/no-op meaning. trace:TASK-1335 | ai:claude
    let required_unavailable: Vec<reconcile_evidence::CandidateOutcome> = outcomes
        .required_unavailable()
        .into_iter()
        .cloned()
        .collect();
    let required_unavailable_error = |other: reconcile_evidence::OtherEffects| {
        let refs: Vec<&reconcile_evidence::CandidateOutcome> =
            required_unavailable.iter().collect();
        anyhow::anyhow!(reconcile_evidence::required_unavailable_error(
            &refs, &scan, dry_run, other
        ))
    };

    if flips.is_empty()
        && stale_review_flips.is_empty()
        && draft_landed.is_empty()
        && closure_holds.is_empty()
    {
        if !required_unavailable.is_empty() {
            return Err(required_unavailable_error(
                reconcile_evidence::OtherEffects::default(),
            ));
        }
        if outcomes
            .entries()
            .iter()
            .any(|o| matches!(o.reason, reconcile_evidence::CandidateReason::OpenChange(_)))
        {
            return Ok(());
        }
        // BUG-418: disambiguate "already recovered" from "nothing matched".
        // If a referencing commit was found but the spec is already terminal,
        // say so plainly — the operator who just ran a recovery needs to read
        // "it's done" as success, not "no commit references it" as failure.
        // trace:BUG-418 | ai:claude
        // An id no requirement owns is reported only to a targeted run.
        let matched_but_ineligible = outcomes
            .entries()
            .iter()
            .filter(|o| match o.reason {
                reconcile_evidence::CandidateReason::AlreadyTerminal => false,
                reconcile_evidence::CandidateReason::UnknownSpec => spec.is_some(),
                _ => true,
            })
            .count();
        if already_terminal.is_empty() && matched_but_ineligible > 0 {
            // TASK-1335: a commit DID match; the reason is printed above
            // (targeted) or available per spec, so do not claim no match.
            match spec {
                Some(s) => println!("No eligible flips for {s}: see the reason above."),
                None => {
                    println!(
                    "No eligible flips: {matched_but_ineligible} spec{} named by commits in the \
                     scan range {} not eligible (rerun with --spec <ID> for the reason).",
                    if matched_but_ineligible == 1 { "" } else { "s" },
                    if matched_but_ineligible == 1 { "is" } else { "are" },
                )
                }
            }
        } else {
            println!("{}", reconcile_no_flip_message(spec, &already_terminal));
        }
        println!("Scanned {scan}.");
        return Ok(());
    }

    if dry_run {
        if !draft_landed.is_empty() {
            println!(
                "{} would flip {} Draft spec{} → Done (trailered commit already on the \
                 default branch; skipped intermediate states):",
                "dry-run:".dimmed(),
                draft_landed.len(),
                if draft_landed.len() == 1 { "" } else { "s" }
            );
            for (spec_id, sha) in &draft_landed {
                let short = if sha.len() >= 7 {
                    &sha[..7]
                } else {
                    sha.as_str()
                };
                println!(
                    "  {} {}",
                    spec_id.bold(),
                    format!("(commit {})", short).dimmed()
                );
            }
        }
        if !flips.is_empty() {
            println!(
                "{} would flip {} spec{} → Completed:",
                "dry-run:".dimmed(),
                flips.len(),
                if flips.len() == 1 { "" } else { "s" }
            );
            for flip in &flips {
                let short = if flip.sha.len() >= 7 {
                    &flip.sha[..7]
                } else {
                    &flip.sha
                };
                println!(
                    "  {} {}",
                    flip.spec_id.bold(),
                    format!("(was {}, commit {})", flip.prior_status, short).dimmed()
                );
            }
        }
        if !stale_review_flips.is_empty() {
            println!(
                "{} would flip {} review stor{} → Completed (PR merged before review finished):",
                "dry-run:".dimmed(),
                stale_review_flips.len(),
                if stale_review_flips.len() == 1 {
                    "y"
                } else {
                    "ies"
                }
            );
            for (sid, sha, pr_n, prior) in &stale_review_flips {
                let short = if sha.len() >= 7 { &sha[..7] } else { sha };
                println!(
                    "  {} ({}, PR #{}, was {})",
                    sid.bold(),
                    short.dimmed(),
                    pr_n,
                    prior
                );
            }
        }
        if !required_unavailable.is_empty() {
            return Err(required_unavailable_error(
                reconcile_evidence::OtherEffects::default(),
            ));
        }
        return Ok(());
    }

    let now = chrono::Utc::now();
    let flips_for_write = flips.clone();
    let stale_for_write = stale_review_flips.clone();
    let holds_for_write = closure_holds.clone();
    // TASK-1335: every write-seam candidate — holds, normal flips, and the
    // independent stale-review flips — with its scan-time identity, so the
    // seam decides ALL of them against the live store before ANY mutation.
    // Order: holds, then flips, then stale-review flips (scan order within).
    // trace:TASK-1335 | ai:claude
    let seam_item = |spec_id: &str, sha: &str, kind: SeamKind| {
        let (uuid, source) = outcomes
            .get(spec_id)
            .map(|o| (o.identity.as_ref().map(|id| id.uuid), o.source))
            .unwrap_or((None, reconcile_evidence::EvidenceSource::SubjectTrailer));
        SeamItem {
            spec_id: spec_id.to_string(),
            sha: sha.to_string(),
            uuid,
            source,
            kind,
        }
    };
    let seam_items: Vec<SeamItem> = holds_for_write
        .iter()
        .map(|h| seam_item(&h.flip.spec_id, &h.flip.sha, SeamKind::Hold))
        .chain(
            flips_for_write
                .iter()
                .map(|f| seam_item(&f.spec_id, &f.sha, SeamKind::Flip)),
        )
        .chain(
            stale_for_write
                .iter()
                .map(|(sid, sha, ..)| seam_item(sid, sha, SeamKind::StaleReview)),
        )
        .collect();
    let mut seam_verdicts: Vec<SeamVerdict> = Vec::new();
    storage.update_atomically(|s| {
        seam_verdicts = plan_reconcile_seam(project_root, s, &seam_items);
        let target_of = |item: &SeamItem| -> Option<usize> {
            match item.uuid {
                Some(uuid) => s.requirements.iter().position(|r| r.id == uuid),
                None => s
                    .get_requirement_unambiguous(&item.spec_id)
                    .ok()
                    .flatten()
                    .map(|r| r.id)
                    .and_then(|id| s.requirements.iter().position(|r| r.id == id)),
            }
        };
        let targets: Vec<Option<usize>> = seam_items.iter().map(target_of).collect();
        let (hold_verdicts, rest) = seam_verdicts.split_at(holds_for_write.len());
        let (flip_verdicts, stale_verdicts) = rest.split_at(flips_for_write.len());
        let (hold_targets, rest_targets) = targets.split_at(holds_for_write.len());
        let (flip_targets, stale_targets) = rest_targets.split_at(flips_for_write.len());
        // BUG-1551: closure holds — land at Done + record the merge.
        // TASK-1335: only a hold the LIVE graph still supports is written, with
        // its CURRENT blockers/criteria — never the scan's payload.
        for (verdict, target) in hold_verdicts.iter().zip(hold_targets) {
            let SeamVerdict::Hold(current) = verdict else {
                continue;
            };
            if let Some(index) = *target {
                apply_closure_hold(&mut s.requirements[index], current, now, project_root);
            }
        }
        for ((flip, verdict), target) in flips_for_write.iter().zip(flip_verdicts).zip(flip_targets)
        {
            if *verdict != SeamVerdict::Apply {
                continue;
            }
            // TASK-1-113: match agreed_id as well as spec_id. flip.spec_id is
            // harvested from the commit subject's `(SPEC-ID)` ref, which is
            // the AGREED id (e.g. TASK-131); for a node-aware spec the stored
            // `spec_id` is the canonical form (e.g. TASK-1-106). Matching only
            // `spec_id` here silently skipped every agreed≠canonical spec —
            // the dry-run/candidate path uses `get_requirement_by_spec_id`
            // (agreed-aware), so the apply diverged from the preview.
            // trace:TASK-1-113 | ai:claude
            // TASK-1335: the seam verdict already resolved the id to the
            // scan-time requirement (agreed or native form), so write exactly
            // that requirement, once.
            if let Some(r) = target.map(|index| &mut s.requirements[index]) {
                // BUG-477: record the reconcile-driven Done→Completed bump
                // in the per-spec history, matching the manual edit path's
                // status field_change shape. trace:BUG-477
                // trace:STORY-1418 | ai:claude
                let prior_status = completion::mark_completed(r);
                // BUG-1637: through the one shared history helper. trace:BUG-1637 | ai:claude
                aida_core::conflict::record_status_transition(
                    r,
                    aida_core::conflict::RECONCILE_AUTHOR,
                    &prior_status,
                );
                r.modified_at = now;
                // BUG-405 contract (review finding): a Completed spec must not
                // keep a stale FailureReason (it drives a false "CI red"
                // finding in `aida findings list`). Clear it on every
                // flip-to-Completed path, not just the primary one.
                r.failure_reason = None;
                let info = r
                    .implementation_info
                    .get_or_insert_with(aida_core::ImplementationInfo::default);
                info.completed_at.get_or_insert(now);
                // BUG-113: a covers-chain flip can carry an empty sha when
                // the covered spec was completed manually (no merge sha) —
                // don't stamp `Some("")`.
                if info.completion_sha.is_none() && !flip.sha.is_empty() {
                    info.completion_sha = Some(flip.sha.clone());
                }
                // STORY-582: same durable processing-record capture on the
                // manual reconcile-status replay path. trace:STORY-582
                let record = build_processing_record(project_root, &flip.spec_id, &flip.sha);
                r.add_processing_record(record);
            }
        }
        // BUG-219: review stories whose PR merged before review finished.
        // Re-check the status inside the atomic window for idempotency
        // and key the audit comment off the live (pre-flip) status.
        // BUG-1560: this inline re-check duplicates `apply_stale_review_flip`
        // rather than calling it, so it needed the same Draft widening by
        // hand — kept in lockstep with the other two sites deliberately,
        // not by omission.
        // TASK-1335: status/reopen/identity/closure were decided for this
        // path by the seam verdict above, against the live store.
        for (((_, sha, pr_n, _), verdict), target) in stale_for_write
            .iter()
            .zip(stale_verdicts)
            .zip(stale_targets)
        {
            if *verdict != SeamVerdict::Apply {
                continue;
            }
            if let Some(r) = target.map(|index| &mut s.requirements[index]) {
                // trace:STORY-1418 | ai:claude
                let prior = completion::mark_completed(r);
                // BUG-477: record the reconcile stale-review flip-to-Completed
                // in the per-spec history too, mirroring the manual edit
                // path's status field_change shape. trace:BUG-477
                // BUG-1637: through the one shared history helper. trace:BUG-1637 | ai:claude
                aida_core::conflict::record_status_transition(
                    r,
                    aida_core::conflict::RECONCILE_AUTHOR,
                    &prior,
                );
                r.modified_at = now;
                // BUG-405 contract (review finding): a Completed spec must not
                // keep a stale FailureReason (it drives a false "CI red"
                // finding in `aida findings list`). Clear it on every
                // flip-to-Completed path, not just the primary one.
                r.failure_reason = None;
                let info = r
                    .implementation_info
                    .get_or_insert_with(aida_core::ImplementationInfo::default);
                info.completed_at.get_or_insert(now);
                if info.completion_sha.is_none() {
                    info.completion_sha = Some(sha.clone());
                }
                r.add_comment(aida_core::Comment::new(
                    "aida-auto-bump".to_string(),
                    stale_review_audit_comment(&prior, *pr_n),
                ));
            }
        }
    })?;

    // TASK-1335: "applied" is what the write seam actually decided for this
    // invocation, never "Completed after the write" — a peer's concurrent
    // completion is not ours to count, report, or emit.
    // trace:TASK-1335 | ai:claude
    let mut confirmed: Vec<AutoBumpFlip> = Vec::new();
    let mut confirmed_stale: Vec<(String, String, u64, RequirementStatus)> = Vec::new();
    let mut seam_reports: Vec<String> = Vec::new();
    let mut verdict_iter = seam_verdicts.into_iter();
    let hold_verdicts: Vec<SeamVerdict> =
        verdict_iter.by_ref().take(holds_for_write.len()).collect();
    let flip_verdicts: Vec<SeamVerdict> = verdict_iter.by_ref().take(flips.len()).collect();
    let stale_verdicts: Vec<SeamVerdict> = verdict_iter.collect();
    // The holds the live seam actually kept, with their current payloads.
    let live_holds: Vec<ClosureHold> = hold_verdicts
        .iter()
        .filter_map(|v| match v {
            SeamVerdict::Hold(current) => Some(current.clone()),
            _ => None,
        })
        .collect();
    let mut decide = |spec_id: &str, verdict: SeamVerdict| -> bool {
        let reason = match verdict {
            SeamVerdict::Apply => reconcile_evidence::CandidateReason::Applied,
            SeamVerdict::Hold(_) => reconcile_evidence::CandidateReason::ClosureHeld,
            SeamVerdict::Changed(why) => {
                reconcile_evidence::CandidateReason::ChangedDuringApply(why)
            }
            SeamVerdict::SameAs(other) => {
                reconcile_evidence::CandidateReason::SameRequirementAs(other)
            }
        };
        let applied = reason == reconcile_evidence::CandidateReason::Applied;
        outcomes.set_reason(spec_id, reason);
        if let Some(line) = outcomes
            .get(spec_id)
            .and_then(|o| reconcile_evidence::render_outcome(o, targeted))
        {
            seam_reports.push(line);
        }
        applied
    };
    for (hold, verdict) in closure_holds.iter().zip(hold_verdicts) {
        decide(&hold.flip.spec_id, verdict);
    }
    for (flip, verdict) in flips.into_iter().zip(flip_verdicts) {
        if decide(&flip.spec_id, verdict) {
            confirmed.push(flip);
        }
    }
    for (stale, verdict) in stale_review_flips.into_iter().zip(stale_verdicts) {
        if decide(&stale.0, verdict) {
            confirmed_stale.push(stale);
        }
    }
    for line in &seam_reports {
        eprintln!("  {line}");
    }
    report_closure_holds(&live_holds);

    for flip in &confirmed {
        record_role_activity(&flip.spec_id, "reconcile-status");
    }
    for (spec_id, _, _, _) in &confirmed_stale {
        record_role_activity(spec_id, "reconcile-status");
    }

    // BUG-1286: reconcile-status is a first-class completion source too.
    for flip in &confirmed {
        emit_spec_completed(
            project_root,
            &flip.spec_id,
            &flip.sha,
            None,
            "reconcile-status",
        );
    }
    for (spec_id, sha, pr_n, _) in &confirmed_stale {
        emit_spec_completed(project_root, spec_id, sha, Some(*pr_n), "reconcile-status");
    }

    print_auto_bump_summary(&confirmed);
    if !confirmed_stale.is_empty() {
        let n = confirmed_stale.len();
        println!(
            "  {} {} review stor{} → {} (PR merged before review finished)",
            "auto-completed".cyan(),
            n,
            if n == 1 { "y" } else { "ies" },
            "Completed".green().bold()
        );
        for (spec_id, _, pr_n, prior) in &confirmed_stale {
            println!(
                "    {} {}",
                spec_id.bold(),
                format!("(PR #{}, was {})", pr_n, prior).dimmed()
            );
        }
    }

    // BUG-1506: apply the Draft → Done flips found above. A separate atomic
    // write from the Completed-flip block above it — different target status,
    // different re-check (`Draft` only) — so it can't be folded into
    // `AutoBumpFlip`'s Completed-only write without teaching that path a
    // second destination status. trace:BUG-1506 | ai:claude
    let confirmed_draft = apply_draft_to_done_bumps(project_root, &storage, &draft_landed)?;
    if !confirmed_draft.is_empty() {
        println!(
            "  {} {} Draft spec{} → {} (trailered commit already on the default branch; \
             skipped intermediate states — confirm before closing)",
            "auto-bumped".cyan(),
            confirmed_draft.len(),
            if confirmed_draft.len() == 1 { "" } else { "s" },
            "Done".yellow().bold()
        );
        for (spec_id, sha) in &confirmed_draft {
            let short = if sha.len() >= 7 {
                &sha[..7]
            } else {
                sha.as_str()
            };
            println!(
                "    {} {}",
                spec_id.bold(),
                format!("(commit {})", short).dimmed()
            );
        }
    }
    if !required_unavailable.is_empty() {
        return Err(required_unavailable_error(
            reconcile_evidence::OtherEffects {
                drafts_landed: confirmed_draft.len(),
                review_stories_completed: confirmed_stale.len(),
            },
        ));
    }
    Ok(())
}

/// TASK-1335: one write-seam candidate — a closure hold, a normal
/// Done→Completed flip, or an independent stale-review flip — with the
/// requirement its evidence resolved to at scan time.
#[derive(Clone, Debug)]
struct SeamItem {
    spec_id: String,
    sha: String,
    uuid: Option<uuid::Uuid>,
    source: reconcile_evidence::EvidenceSource,
    kind: SeamKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SeamKind {
    Hold,
    Flip,
    StaleReview,
}

/// TASK-1335: what the write seam decided for one [`SeamItem`].
#[derive(Clone, Debug, PartialEq, Eq)]
enum SeamVerdict {
    Apply,
    /// The live store no longer supports the scan-time decision.
    Changed(String),
    /// A scan-time hold is still held by the live graph: write THIS payload.
    Hold(ClosureHold),
    /// Another accepted id of the same requirement is applied instead.
    SameAs(String),
}

/// TASK-1335: decide every seam item against the LIVE store under the writer
/// lock, before any mutation. Per item: the evidence id must still resolve
/// unambiguously to the scan-time UUID, the status must still be one its
/// path completes (or holds), and reopen fences must not cover the
/// evidence. Completions are then consolidated per UUID (the first accepted
/// item in scan order wins; its aliases are reported, not re-applied), and —
/// to a fixed point, since dropping one completion can withdraw another's
/// support — covers support must be ROOTED in a live Completed requirement
/// or a surviving non-covers completion (never another covers candidate),
/// and the live closure graph is evaluated as a least fixed point over the
/// surviving completions. Finally each scan-time hold is re-derived from the
/// live graph: still held → [`SeamVerdict::Hold`] with the CURRENT payload;
/// no longer held → refused as changed (a rerun decides; no completion and
/// no obsolete note). Covers support is deliberately DIRECT (BUG-113's
/// existing rule: a covered spec Completed or completing in this run's
/// normal batch), so a story covering only another covers candidate
/// completes on a later run, once that candidate is Completed. Pure local
/// reads; no forge I/O under the lock.
// trace:TASK-1335 | ai:claude
// trace:TASK-1600 | ai:codex
fn plan_reconcile_seam(
    project_root: &std::path::Path,
    store: &aida_core::RequirementsStore,
    items: &[SeamItem],
) -> Vec<SeamVerdict> {
    let mut verdicts: Vec<SeamVerdict> = items
        .iter()
        .map(|item| {
            let current = match store.get_requirement_unambiguous(&item.spec_id) {
                Ok(Some(r)) => r,
                Ok(None) => {
                    return SeamVerdict::Changed(
                        "its ID no longer resolves to a requirement".to_string(),
                    )
                }
                Err(_) => {
                    return SeamVerdict::Changed(
                        "its ID now names more than one requirement".to_string(),
                    )
                }
            };
            if item.uuid.is_some_and(|uuid| uuid != current.id) {
                return SeamVerdict::Changed(
                    "its ID now resolves to a different requirement".to_string(),
                );
            }
            let status_ok = match item.kind {
                SeamKind::Hold | SeamKind::Flip => auto_bump_eligible_status(&current.status),
                SeamKind::StaleReview => matches!(
                    current.status,
                    RequirementStatus::Draft
                        | RequirementStatus::Approved
                        | RequirementStatus::InProgress
                ),
            };
            if !status_ok {
                return SeamVerdict::Changed(format!("its status changed to {}", current.status));
            }
            if auto_bump_evidence_is_stale(project_root, current, &item.sha) {
                return SeamVerdict::Changed("it was reopened after this evidence".to_string());
            }
            SeamVerdict::Apply
        })
        .collect();

    // One completion per requirement, whichever accepted id named it.
    let completes = |kind: SeamKind| kind != SeamKind::Hold;
    let resolved_uuid = |item: &SeamItem| {
        item.uuid.or_else(|| {
            store
                .get_requirement_by_spec_id(&item.spec_id)
                .map(|r| r.id)
        })
    };
    for index in 0..items.len() {
        if verdicts[index] != SeamVerdict::Apply || !completes(items[index].kind) {
            continue;
        }
        let Some(uuid) = resolved_uuid(&items[index]) else {
            continue;
        };
        if let Some(first) = (0..index).find(|&earlier| {
            verdicts[earlier] == SeamVerdict::Apply
                && completes(items[earlier].kind)
                && resolved_uuid(&items[earlier]) == Some(uuid)
        }) {
            verdicts[index] = SeamVerdict::SameAs(items[first].spec_id.clone());
        }
    }

    loop {
        let mut changed = false;
        let anchors: std::collections::HashSet<uuid::Uuid> = items
            .iter()
            .zip(&verdicts)
            .filter(|(item, v)| {
                **v == SeamVerdict::Apply
                    && item.kind == SeamKind::Flip
                    && item.source != reconcile_evidence::EvidenceSource::CoversChain
            })
            .filter_map(|(item, _)| resolved_uuid(item))
            .collect();
        for index in 0..items.len() {
            if verdicts[index] != SeamVerdict::Apply
                || items[index].source != reconcile_evidence::EvidenceSource::CoversChain
            {
                continue;
            }
            if !covers_chain_rooted(store, &items[index].spec_id, &anchors) {
                verdicts[index] =
                    SeamVerdict::Changed("no covered spec is still completed".to_string());
                changed = true;
            }
        }
        let surviving: Vec<usize> = (0..items.len())
            .filter(|&i| verdicts[i] == SeamVerdict::Apply && completes(items[i].kind))
            .collect();
        let mut released: Vec<AutoBumpFlip> = surviving
            .iter()
            .map(|&i| {
                AutoBumpFlip::new(
                    items[i].spec_id.clone(),
                    items[i].sha.clone(),
                    RequirementStatus::Done,
                )
            })
            .collect();
        for hold in split_closure_held_flips(store, &mut released) {
            let Some(&index) = surviving
                .iter()
                .find(|&&i| items[i].spec_id == hold.flip.spec_id)
            else {
                continue;
            };
            // Every completion was released by the scan's closure pass, so
            // a hold here is new since the scan.
            verdicts[index] = SeamVerdict::Changed(format!(
                "its completion is now held ({})",
                closure_hold_causes(&hold)
            ));
            changed = true;
        }
        if !changed {
            break;
        }
    }

    // Scan-time holds: re-derive the hold from the LIVE graph (treating the
    // surviving completions as resolved, the same least fixed point), so the
    // writer records the current blockers/criteria — or nothing at all when
    // the hold no longer applies (conservatively refused, rerun decides).
    let resolved: std::collections::HashSet<uuid::Uuid> = items
        .iter()
        .zip(&verdicts)
        .filter(|(item, v)| **v == SeamVerdict::Apply && completes(item.kind))
        .filter_map(|(item, _)| resolved_uuid(item))
        .collect();
    let mut held_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (item, verdict) in items.iter().zip(&verdicts) {
        if item.kind == SeamKind::Hold && *verdict == SeamVerdict::Apply {
            held_ids.insert(item.spec_id.clone());
            if let Some(req) = store.get_requirement_by_spec_id(&item.spec_id) {
                held_ids.insert(
                    req.agreed_id
                        .clone()
                        .or_else(|| req.spec_id.clone())
                        .unwrap_or_else(|| req.id.to_string()),
                );
            }
        }
    }
    for (item, verdict) in items.iter().zip(verdicts.iter_mut()) {
        if item.kind != SeamKind::Hold || *verdict != SeamVerdict::Apply {
            continue;
        }
        let Some(req) = store.get_requirement_by_spec_id(&item.spec_id) else {
            continue;
        };
        let (blockers, criteria) = closure_holders_treating_resolved(req, store, &resolved);
        *verdict = if blockers.is_empty() && criteria.is_empty() {
            SeamVerdict::Changed("its closure hold no longer applies".to_string())
        } else {
            let cycle_members = blockers
                .iter()
                .filter(|b| held_ids.contains(&b.id))
                .map(|b| b.id.clone())
                .collect();
            SeamVerdict::Hold(ClosureHold {
                flip: AutoBumpFlip::new(item.spec_id.clone(), item.sha.clone(), req.status.clone()),
                blockers,
                cycle_members,
                criteria,
            })
        };
    }
    verdicts
}

/// TASK-1335: [`collect_covers_completed_review_flips`]'s condition re-read
/// at the write seam with ROOTED support: the review story is still a `Done`
/// review story and an `implements` target is Completed in the live store
/// or is one of `anchors` (surviving non-covers completions). Another covers
/// candidate never counts, so a covers cycle cannot justify itself.
// trace:TASK-1335 | ai:claude
fn covers_chain_rooted(
    store: &aida_core::RequirementsStore,
    review_id: &str,
    anchors: &std::collections::HashSet<uuid::Uuid>,
) -> bool {
    let Some(review) = store.get_requirement_by_spec_id(review_id) else {
        return false;
    };
    if !matches!(review.status, RequirementStatus::Done)
        || parse_review_story_pr_number(&review.title).is_none()
    {
        return false;
    }
    review.relationships.iter().any(|rel| {
        matches!(
            &rel.rel_type,
            aida_core::RelationshipType::Custom(n) if n.eq_ignore_ascii_case("implements")
        ) && store
            .requirements
            .iter()
            .find(|r| r.id == rel.target_id)
            .is_some_and(|covered| {
                matches!(covered.status, RequirementStatus::Completed)
                    || anchors.contains(&covered.id)
            })
    })
}

/// STORY-86 / BUG-328: print the auto-bump summary line after a successful
/// pull/reconcile. Stays out of the way when nothing flipped.
/// trace:STORY-86 BUG-328 | ai:claude,codex
pub(crate) fn print_auto_bump_summary(flips: &[AutoBumpFlip]) {
    if flips.is_empty() {
        return;
    }
    let n = flips.len();
    if n == 1 {
        let flip = &flips[0];
        let short = if flip.sha.len() >= 7 {
            &flip.sha[..7]
        } else {
            &flip.sha
        };
        println!(
            "  {} 1 spec → {}: {} {}",
            "auto-bumped".cyan(),
            "Completed".green().bold(),
            flip.spec_id.bold(),
            format!("(was {}, commit {})", flip.prior_status, short).dimmed()
        );
        return;
    }
    let plural = if n == 1 { "" } else { "s" };
    println!(
        "  {} {} spec{} → {}",
        "auto-bumped".cyan(),
        n,
        plural,
        "Completed".green().bold()
    );
    // Show the first 5 spec IDs so the user can see what landed.
    const PREVIEW: usize = 5;
    for flip in flips.iter().take(PREVIEW) {
        let short = if flip.sha.len() >= 7 {
            &flip.sha[..7]
        } else {
            &flip.sha
        };
        println!(
            "    {} {}",
            flip.spec_id.bold(),
            format!("(was {}, commit {})", flip.prior_status, short).dimmed()
        );
    }
    if n > PREVIEW {
        println!("    {} {} more", "…".dimmed(), n - PREVIEW);
    }
}

/// Walk `git log <pre-pull-sha>..HEAD` on the orphan store and print a
/// per-category summary of what landed in this pull: specs added,
/// modified (with status flips when detectable), deleted, plus a comment
/// count. Stays out of the way when nothing changed and falls back
/// quietly on any git failure (the pull itself already succeeded; we
/// don't want a flaky summary to look like a failed pull).
/// trace:TASK-73 | ai:claude
pub(crate) fn print_pull_summary(store_path: &std::path::Path, pre_sha: &str) {
    use std::collections::BTreeMap;

    let range = format!("{}..HEAD", pre_sha);
    let log = match aida_core::git_ops::host_git_command(store_path)
        .args([
            "log",
            "--name-status",
            // Separate D and A lines instead of an `R<score>` pair, which the
            // one-tab parse below would skip. trace:BUG-1616 | ai:claude
            "--no-renames",
            "--pretty=format:%H%x09%s",
            range.as_str(),
        ])
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return, // soft failure — pull already succeeded, don't shout
    };
    if log.trim().is_empty() {
        println!("  {}", "(no new commits)".dimmed());
        return;
    }

    // Lines are either `<sha>\t<subject>` (commit header) or
    // `<status>\t<path>` (name-status row).
    let mut added: BTreeMap<String, String> = BTreeMap::new(); // spec_id -> req_type
    let mut modified: BTreeMap<String, String> = BTreeMap::new();
    let mut deleted: BTreeMap<String, String> = BTreeMap::new();
    let mut comment_subjects: Vec<String> = Vec::new(); // commit subjects mentioning comments
    let mut commit_count: usize = 0;
    let mut current_subject_is_comment = false;
    // TASK-75: track commits so we can attribute status changes to their
    // source. Map sha → (subject, source-label). trace:TASK-75 | ai:claude
    let mut commit_sources: Vec<(String, String, String)> = Vec::new();
    let mut current_sha: Option<String> = None;
    let mut modified_specs_per_commit: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for line in log.lines() {
        if line.is_empty() {
            continue;
        }
        let tabs = line.bytes().filter(|b| *b == b'\t').count();
        if tabs == 1 {
            // Could be either a commit header (sha is 40 hex chars) or a
            // name-status line (status letter is 1-2 chars). Disambiguate
            // by length of the first column.
            let mut parts = line.splitn(2, '\t');
            let first = parts.next().unwrap_or("");
            let rest = parts.next().unwrap_or("");
            if first.len() >= 7 && first.chars().all(|c| c.is_ascii_hexdigit()) {
                // Commit header
                commit_count += 1;
                let subj = rest.trim();
                current_subject_is_comment = subj.contains("comment on ")
                    || subj.contains("add comment")
                    || subj.starts_with("comment:");
                if current_subject_is_comment {
                    comment_subjects.push(subj.to_string());
                }
                current_sha = Some(first.to_string());
                let source = classify_commit_source(subj);
                commit_sources.push((first.to_string(), subj.to_string(), source));
            } else {
                // name-status row
                if !rest.starts_with("objects/") || !rest.ends_with(".yaml") {
                    continue;
                }
                let spec_id = spec_id_from_orphan_path(rest);
                let req_type = req_type_from_orphan_path(rest);
                match first.chars().next() {
                    Some('A') => {
                        added.insert(spec_id, req_type);
                    }
                    Some('M') => {
                        // Modifications driven by comment commits shouldn't
                        // double-count under "modified" — they're already
                        // surfaced via comment_subjects.
                        if !current_subject_is_comment {
                            modified.insert(spec_id.clone(), req_type);
                        }
                        if let Some(sha) = &current_sha {
                            modified_specs_per_commit
                                .entry(sha.clone())
                                .or_default()
                                .push(spec_id);
                        }
                    }
                    Some('D') => {
                        deleted.insert(spec_id, req_type);
                    }
                    _ => {}
                }
            }
        }
    }

    // TASK-75: walk each commit's diff to extract status transitions
    // (`-status: X` / `+status: Y` lines in the YAML hunk) and attribute
    // them to the commit's source classification.
    // trace:TASK-75 | ai:claude
    let status_changes = extract_status_changes_from_commits(
        store_path,
        &commit_sources,
        &modified_specs_per_commit,
    );

    if added.is_empty()
        && modified.is_empty()
        && deleted.is_empty()
        && comment_subjects.is_empty()
        && status_changes.is_empty()
    {
        println!(
            "  {} ({} commits, no spec changes — internal churn / chore-only)",
            "(no spec changes)".dimmed(),
            commit_count
        );
        return;
    }

    println!();
    println!("{} ({} commits):", "Changes pulled".bold(), commit_count);
    if !added.is_empty() {
        println!(
            "  {} {} spec{} added",
            "+".green().bold(),
            added.len(),
            if added.len() == 1 { "" } else { "s" }
        );
        for (id, _) in added.iter().take(8) {
            println!("    {}", id);
        }
        if added.len() > 8 {
            println!("    {}", format!("(+{} more)", added.len() - 8).dimmed());
        }
    }
    if !modified.is_empty() {
        println!(
            "  {} {} spec{} modified",
            "~".cyan(),
            modified.len(),
            if modified.len() == 1 { "" } else { "s" }
        );
        for (id, _) in modified.iter().take(8) {
            println!("    {}", id);
        }
        if modified.len() > 8 {
            println!("    {}", format!("(+{} more)", modified.len() - 8).dimmed());
        }
    }
    if !deleted.is_empty() {
        println!(
            "  {} {} spec{} deleted",
            "−".red(),
            deleted.len(),
            if deleted.len() == 1 { "" } else { "s" }
        );
        for (id, _) in deleted.iter().take(8) {
            println!("    {}", id);
        }
    }
    if !comment_subjects.is_empty() {
        println!(
            "  {} {} comment commit{}",
            "💬".dimmed(),
            comment_subjects.len(),
            if comment_subjects.len() == 1 { "" } else { "s" }
        );
        for s in comment_subjects.iter().take(5) {
            println!("    {}", s.dimmed());
        }
        if comment_subjects.len() > 5 {
            println!(
                "    {}",
                format!("(+{} more)", comment_subjects.len() - 5).dimmed()
            );
        }
    }
    if !status_changes.is_empty() {
        // TASK-75: group status changes by their commit source so the
        // user can see which PR-N merge / commit / manual action drove
        // each transition. trace:TASK-75 | ai:claude
        let mut by_source: BTreeMap<String, Vec<&StatusTransition>> = BTreeMap::new();
        for tr in &status_changes {
            by_source.entry(tr.source.clone()).or_default().push(tr);
        }
        let total = status_changes.len();
        println!(
            "  {} {} status change{}",
            "~".cyan(),
            total,
            if total == 1 { "" } else { "s" }
        );
        for (source, list) in &by_source {
            println!("    {}", format!("({}):", source).dimmed());
            for tr in list.iter().take(8) {
                println!(
                    "      {:<12} {} → {}",
                    tr.spec_id,
                    tr.from.yellow(),
                    tr.to.green()
                );
            }
            if list.len() > 8 {
                println!("      {}", format!("(+{} more)", list.len() - 8).dimmed());
            }
        }
    }
    println!(
        "  {}",
        "Run `aida history --since \"5 min ago\" --events` for the full event stream.".dimmed()
    );
}
