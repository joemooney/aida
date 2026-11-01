//! Merge and ship approval gates (`aida pr ship`, drain merge phase, release/burndown).
// trace:STORY-1889 | ai:claude
// mandate-gate

use crate::pr_cmd::{
    append_ship_activity_line, commit_committed_at, log_ship_activity, normalized_spec_ids,
    pr_gate_subject,
};
use crate::{auto_complete, pr_ship};
use aida_core::git_ops::RemoteCredentialsExt as _;
use anyhow::Result;
use colored::Colorize;

/// TASK-1448: the drain merge phase's approval-covers-head gate. `Err` is a
/// shelvable, never-retried `StaleApproval` failure carrying the same
/// refusal text `aida pr ship` prints — the drain parks the spec and moves
/// on rather than exiting. Pure over the verdicts + policy + head so every
/// branch is testable without a forge.
///
/// BUG-1918: approval AUTHORITY is checked first, exactly as `aida pr ship`
/// checks it — no approval, an unattested one, or one recorded by an author
/// session shelves the spec; the drain's own provenance-stamped verdict is
/// not an approval.
// trace:TASK-1448 trace:BUG-1918 | ai:claude
pub(crate) fn drain_merge_approval_gate(
    candidates: &[crate::gate::review_verdict::RecordedVerdict],
    policy: &crate::gate::ship::ShipApprovalPolicy,
    head_sha: Option<&str>,
    pr: u64,
) -> Result<(), auto_complete::PhaseFailure> {
    if let Some(refusal) = crate::gate::ship::approval_authority_refusal(candidates, policy) {
        return Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::StaleApproval,
            crate::gate::ship::approval_authority_refusal_message(pr, &refusal),
        ));
    }
    match crate::gate::ship::approval_head_refusal(candidates, head_sha) {
        None => Ok(()),
        Some(refusal) => Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::StaleApproval,
            crate::gate::ship::approval_head_refusal_message(pr, &refusal),
        )),
    }
}

/// TASK-1458: the `MergeOptions.match_head` pin for the drain's phase-4
/// merge — the approved commit once `drain_merge_approval_gate` has passed.
/// Kept beside the gate so the wiring test drives the same pair of calls the
/// real `merge()` makes.
// trace:TASK-1458 | ai:claude
pub(crate) fn drain_merge_match_head(
    candidates: &[crate::gate::review_verdict::RecordedVerdict],
    head_sha: Option<&str>,
) -> Option<String> {
    crate::gate::ship::approved_match_head(candidates, head_sha)
}

/// What the approval gate decides once approval AUTHORITY holds.
// trace:TASK-1945 | ai:claude
#[derive(Debug)]
pub(crate) enum HeadDecision {
    /// An approval covers the head: merge pinned to this (`None`: nothing to
    /// pin to).
    Covered(Option<String>),
    /// TASK-1960: the head moved only by clean merges of the base branch,
    /// with the PR's own diff unchanged, so the attested approval carries
    /// over to `proof.new_head`. The outcome is `None` for a quiet site.
    CarriedOver(
        crate::gate::review_carryover::CarryOver,
        Option<crate::gate::review_carryover::RecordOutcome>,
    ),
    /// No approval covers the head; `message` says why, including a refused
    /// carry-over. Only `--override-stale-approval` can pass it.
    Stale {
        refusal: crate::gate::ship::ApprovalHeadRefusal,
        message: String,
    },
}

/// The approval gate's decision with no output, log or event of its own:
/// `Err(message)` when no authorized, non-author approval exists (BUG-1918,
/// never overridable), else whether it covers `head_sha`. A carry-over is
/// tried through `carry_over`; a quiet site records nothing, so through one
/// this reads only. `aida pr ship` and the review watcher's readiness report
/// both decide here, so the two cannot disagree.
// trace:TASK-1945 | ai:claude
pub(crate) fn approval_decision(
    pr_number: u64,
    candidates: &[crate::gate::review_verdict::RecordedVerdict],
    head_sha: Option<&str>,
    policy: &crate::gate::ship::ShipApprovalPolicy,
    carry_over: Option<&crate::gate::review_carryover::CarryOverSite<'_>>,
) -> std::result::Result<HeadDecision, String> {
    // BUG-1918: the approval must come from an authorized, non-author seat.
    // Checked first: `--override-stale-approval` excuses a head that moved
    // past a real approval, never a missing or self-recorded one.
    // trace:BUG-1918 | ai:claude
    if let Some(refusal) = crate::gate::ship::approval_authority_refusal(candidates, policy) {
        return Err(crate::gate::ship::approval_authority_refusal_message(
            pr_number, &refusal,
        ));
    }
    let Some(refusal) = crate::gate::ship::approval_head_refusal(candidates, head_sha) else {
        return Ok(HeadDecision::Covered(
            crate::gate::ship::approved_match_head(candidates, head_sha),
        ));
    };
    let mut message = crate::gate::ship::approval_head_refusal_message(pr_number, &refusal);
    // TASK-1960: a head that moved only by clean merges of the base branch,
    // with the PR's own diff unchanged, keeps the attested approval. The
    // merge is pinned to the proven head, so a later push still refuses.
    // trace:TASK-1960 | ai:claude
    match carry_over.and_then(|site| {
        crate::gate::review_carryover::carry_over_for_refusal(site, pr_number, &refusal)
    }) {
        Some(Ok((proof, outcome))) => return Ok(HeadDecision::CarriedOver(proof, outcome)),
        Some(Err(why)) => {
            message = format!(
                "{message} The approval did not carry over: {}.",
                why.reason()
            );
        }
        None => {}
    }
    Ok(HeadDecision::Stale { refusal, message })
}

/// TASK-1448 + TASK-1458: `aida pr ship`'s approval-covers-head gate.
/// `Ok(pin)` = merge, pinned to `pin` (`MergeOptions.match_head`, i.e. gh
/// `--match-head-commit`) so a push between this check and the merge is
/// refused by the forge instead of landing unreviewed. The pin is the
/// approved head when an open approval covers it, `None` when there is no
/// approval, and — under `--override-stale-approval` — the head the override
/// was granted for, after a durable `advisor-activity.jsonl` record naming
/// both shas. `Err` = refused (logged as a failed merge step).
// trace:TASK-1458 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn pr_ship_approval_gate(
    main_worktree: &std::path::Path,
    pr_number: u64,
    candidates: &[crate::gate::review_verdict::RecordedVerdict],
    head_sha: Option<&str>,
    policy: &crate::gate::ship::ShipApprovalPolicy,
    override_stale_approval: bool,
    delete_branch: bool,
    carry_over: Option<&crate::gate::review_carryover::CarryOverSite<'_>>,
) -> Result<Option<String>> {
    let decision = match approval_decision(pr_number, candidates, head_sha, policy, carry_over) {
        Ok(decision) => decision,
        Err(message) => {
            log_ship_activity(
                main_worktree,
                Some(pr_number),
                &pr_ship::ShipStep::Merge { delete_branch },
                &pr_ship::StepOutcome::Failed(message.clone()),
            );
            crate::events::record_gate_held(
                main_worktree,
                crate::events::GATE_APPROVAL_AUTHORITY,
                None,
                Some(pr_number),
                &message,
            );
            return Err(crate::gate::ship::ShipRefusal::Review.error(message));
        }
    };
    // STORY-1655: name the mode when a solo-human approval is relied on.
    // trace:STORY-1655 | ai:claude
    // Same stream as merge_path_approval_gate. trace:TASK-1955 | ai:claude (TASK-1808)
    if let Some(notice) = crate::gate::ship::solo_approval_notice(candidates, policy) {
        eprintln!("  {notice}");
    }
    let (refusal, message) = match decision {
        HeadDecision::Covered(pin) => return Ok(pin),
        HeadDecision::CarriedOver(proof, outcome) => {
            // A quiet (preflight) site records nothing and stays silent; the
            // gate that pins the merge announces it once.
            if let Some(outcome) = outcome {
                println!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    crate::gate::review_carryover::carry_over_line(&proof, &outcome)
                );
                append_ship_activity_line(
                    main_worktree,
                    &crate::gate::review_carryover::carry_over_event(
                        &chrono::Utc::now().to_rfc3339(),
                        "aida pr ship",
                        &proof,
                    ),
                );
            }
            return Ok(Some(proof.new_head));
        }
        HeadDecision::Stale { refusal, message } => (refusal, message),
    };
    // BUG-1918: only a non-author session may ship commits no approval
    // covered; the override is neither honoured nor advertised to an author.
    // trace:BUG-1918 | ai:claude
    let override_refusal = crate::gate::ship::stale_override_refusal(policy);
    if !override_stale_approval || override_refusal.is_some() {
        let message = match (&override_refusal, override_stale_approval) {
            (Some(why), true) => {
                format!("{message} `--override-stale-approval` refused: {why}.")
            }
            _ => message,
        };
        log_ship_activity(
            main_worktree,
            Some(pr_number),
            &pr_ship::ShipStep::Merge { delete_branch },
            &pr_ship::StepOutcome::Failed(message.clone()),
        );
        // STORY-1436: the stale-approval gate HELD. trace:STORY-1436 | ai:claude
        crate::events::record_gate_held(
            main_worktree,
            crate::events::GATE_STALE_APPROVAL,
            None,
            Some(pr_number),
            &message,
        );
        return Err(
            crate::gate::ship::ShipRefusal::Review.error(match override_refusal {
                None => {
                    format!("{message} (To merge anyway, re-run with `--override-stale-approval`.)")
                }
                Some(_) => message,
            }),
        );
    }
    append_ship_activity_line(
        main_worktree,
        &crate::gate::ship::format_stale_approval_override_event(
            &chrono::Utc::now().to_rfc3339(),
            pr_number,
            &refusal,
        ),
    );
    eprintln!(
        "  {} {message} — shipping anyway (--override-stale-approval)",
        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
    );
    Ok(head_sha
        .map(str::trim)
        .filter(|sha| !sha.is_empty())
        .map(str::to_string))
}

/// BUG-1918: the approval candidates for merging PR `pr` (read from `roots`)
/// and the [`ShipApprovalPolicy`] they are judged by — the one input every
/// merge path (`aida pr ship`, the drain, `aida release --after-pr`,
/// burndown) builds the same way.
// trace:BUG-1918 | ai:claude
pub(crate) fn merge_approval_inputs(
    roots: &[&std::path::Path],
    main_worktree: &std::path::Path,
    pr: u64,
    known_ids: &[String],
    head_branch: Option<&str>,
    head_sha: Option<&str>,
) -> (
    Vec<crate::gate::review_verdict::RecordedVerdict>,
    crate::gate::ship::ShipApprovalPolicy,
) {
    let (candidates, policy, _) =
        merge_approval_subject(roots, main_worktree, pr, known_ids, head_branch, head_sha);
    (candidates, policy)
}

/// [`merge_approval_inputs`] plus the spec ids the PR ships, which the
/// corpus gate takes as its hints.
// trace:TASK-1945 | ai:claude
pub(crate) fn merge_approval_subject(
    roots: &[&std::path::Path],
    main_worktree: &std::path::Path,
    pr: u64,
    known_ids: &[String],
    head_branch: Option<&str>,
    head_sha: Option<&str>,
) -> (
    Vec<crate::gate::review_verdict::RecordedVerdict>,
    crate::gate::ship::ShipApprovalPolicy,
    Vec<String>,
) {
    let (ids, branch, forge_resolved, authorship_ids) =
        pr_gate_subject(main_worktree, pr, known_ids, head_branch);
    let candidates = crate::gate::ship::merge_gate_verdict_candidates(roots, pr, &ids, head_sha);
    // An unconfirmed subject (forge unreachable) names no author set: every
    // recorded author counts. trace:BUG-1918 | ai:claude
    // trace:BUG-2199 | ai:claude
    let author_ids: &[String] = if forge_resolved { &authorship_ids } else { &[] };
    let policy = ship_approval_policy(main_worktree, author_ids, branch.as_deref(), &candidates);
    (candidates, policy, ids)
}

/// BUG-1918: the approval gate for merge paths other than `aida pr ship`
/// (`aida release --after-pr`, burndown): approval AUTHORITY, then head
/// coverage, with no override. `Ok(pin)` = merge pinned to `pin`
/// (`MergeOptions.match_head`); `Err` = the refusal text, already recorded
/// as a held gate.
// trace:BUG-1918 | ai:claude
pub(crate) fn merge_path_approval_gate(
    project_root: &std::path::Path,
    pr: u64,
    known_ids: &[String],
    head_branch: Option<&str>,
    head_sha: Option<&str>,
) -> std::result::Result<Option<String>, String> {
    let main_worktree = crate::main_worktree_root_from(project_root);
    let (candidates, policy) = merge_approval_inputs(
        &[project_root, main_worktree.as_path()],
        &main_worktree,
        pr,
        known_ids,
        head_branch,
        head_sha,
    );
    // TASK-1960: the base is read from the forge (prove time and again right
    // before the caller merges). trace:TASK-1960 | ai:claude
    let site = crate::gate::review_carryover::CarryOverSite::new(&main_worktree, None);
    merge_path_approval_decision(
        &main_worktree,
        pr,
        &candidates,
        &policy,
        head_sha,
        &site,
        &|| {
            crate::forge::forge_for(project_root)
                .change_metadata(pr, &mut crate::network_retry::NoopSink)
                .ok()
                .map(|m| m.base_ref)
        },
    )
}

/// [`merge_path_approval_gate`] once its inputs are gathered: authority,
/// then head coverage with the TASK-1960 carry-over through `site`, whose
/// proven base must still match `base_now()` (the PR's base at merge time).
// trace:BUG-1918 | ai:claude
// trace:TASK-1960 | ai:claude
pub(crate) fn merge_path_approval_decision(
    main_worktree: &std::path::Path,
    pr: u64,
    candidates: &[crate::gate::review_verdict::RecordedVerdict],
    policy: &crate::gate::ship::ShipApprovalPolicy,
    head_sha: Option<&str>,
    site: &crate::gate::review_carryover::CarryOverSite<'_>,
    base_now: &dyn Fn() -> Option<String>,
) -> std::result::Result<Option<String>, String> {
    if let Some(refusal) = crate::gate::ship::approval_authority_refusal(candidates, policy) {
        let message = crate::gate::ship::approval_authority_refusal_message(pr, &refusal);
        crate::events::record_gate_held(
            main_worktree,
            crate::events::GATE_APPROVAL_AUTHORITY,
            None,
            Some(pr),
            &message,
        );
        return Err(message);
    }
    // STORY-1655: name the mode when a solo-human approval is relied on.
    // trace:STORY-1655 | ai:claude
    if let Some(notice) = crate::gate::ship::solo_approval_notice(candidates, policy) {
        eprintln!("  {notice}");
    }
    if let Some(refusal) = crate::gate::ship::approval_head_refusal(candidates, head_sha) {
        let mut message = crate::gate::ship::approval_head_refusal_message(pr, &refusal);
        // TASK-1960: the same carry-over `aida pr ship` applies.
        // trace:TASK-1960 | ai:claude
        match crate::gate::review_carryover::carry_over_for_refusal(site, pr, &refusal) {
            Some(Ok((proof, outcome))) => {
                // Every caller merges right after this gate returns, so the
                // PR's base is re-read here: a base retargeted since the
                // proof refuses. trace:TASK-1960 | ai:claude
                if let Some(why) = crate::gate::review_carryover::proven_base_refusal(
                    pr,
                    &proof.base_branch(),
                    base_now().as_deref(),
                ) {
                    crate::events::record_gate_held(
                        main_worktree,
                        crate::events::GATE_STALE_APPROVAL,
                        None,
                        Some(pr),
                        &why,
                    );
                    return Err(why);
                }
                if let Some(outcome) = &outcome {
                    eprintln!(
                        "  {}",
                        crate::gate::review_carryover::carry_over_line(&proof, outcome)
                    );
                }
                append_ship_activity_line(
                    main_worktree,
                    &crate::gate::review_carryover::carry_over_event(
                        &chrono::Utc::now().to_rfc3339(),
                        "merge",
                        &proof,
                    ),
                );
                return Ok(Some(proof.new_head));
            }
            Some(Err(why)) => {
                message = format!(
                    "{message} The approval did not carry over: {}.",
                    why.reason()
                );
            }
            None => {}
        }
        crate::events::record_gate_held(
            main_worktree,
            crate::events::GATE_STALE_APPROVAL,
            None,
            Some(pr),
            &message,
        );
        return Err(message);
    }
    Ok(crate::gate::ship::approved_match_head(candidates, head_sha))
}

/// Read-only positive review authority for dispatch and review views. Reuse
/// shipping's durable grant/receipt, identity, independence and time checks.
/// Unlike shipping, a view never fetches a missing commit or grants a solo
/// self-review exception. Missing local evidence stays unverified.
// trace:BUG-2398 | ai:codex
pub(crate) fn independent_approval_authorized(
    project_root: &std::path::Path,
    spec_ids: &[String],
    branch: Option<&str>,
    verdict: &crate::gate::review_verdict::RecordedVerdict,
) -> bool {
    if !verdict.kind.approves() || verdict.is_closed() {
        return false;
    }
    let candidates = std::slice::from_ref(verdict);
    let ids: Vec<String> = normalized_spec_ids(spec_ids)
        .into_iter()
        .filter(|id| !id.starts_with("PR-") && !id.starts_with("MR-"))
        .collect();
    let author_sessions = if ids.is_empty() {
        crate::gate::review_authority::all_author_sessions(project_root, branch)
    } else {
        crate::gate::review_authority::author_sessions_at(project_root, &ids, branch)
    };
    let policy = crate::gate::ship::ShipApprovalPolicy {
        author_sessions,
        verified_authority: crate::gate::review_authority::verified_authority_keys(
            project_root,
            candidates,
        ),
        reviewed_committed_at: verdict
            .reviewed_sha
            .as_deref()
            .and_then(|sha| commit_committed_at(project_root, sha)),
        shipper: Vec::new(),
        solo_human: false,
        verified_solo: Default::default(),
    };
    crate::gate::ship::approval_authority_refusal(candidates, &policy).is_none()
}

/// BUG-1918: the approval policy for the specs a PR ships — its author
/// sessions (authoring leases on those specs or on the PR's head branch, live
/// or recorded in the durable authorship ledger; with NO spec id, every live
/// authoring lease and ledger record, failing closed), which recorded
/// approvals' authority re-validates against its durable store, and the
/// committer date of the commit the newest approval reviewed. There is no
/// review opt-out. A missing or malformed ledger is warned about.
// trace:BUG-1918 | ai:claude
pub(crate) fn ship_approval_policy(
    main_worktree: &std::path::Path,
    spec_ids: &[String],
    head_branch: Option<&str>,
    candidates: &[crate::gate::review_verdict::RecordedVerdict],
) -> crate::gate::ship::ShipApprovalPolicy {
    let ids = normalized_spec_ids(spec_ids);
    crate::gate::review_authority::warn_on_ledger_read(main_worktree);
    let author_sessions = if ids.is_empty() {
        crate::gate::review_authority::all_author_sessions(main_worktree, head_branch)
    } else {
        crate::gate::review_authority::author_sessions_at(main_worktree, &ids, head_branch)
    };
    let approvals: Vec<crate::gate::review_verdict::RecordedVerdict> = candidates
        .iter()
        .filter(|v| v.kind.approves() && !v.is_closed())
        .cloned()
        .collect();
    let reviewed_committed_at = crate::gate::ship::newest_approval(&approvals)
        .and_then(|a| a.reviewed_sha.as_deref())
        .and_then(|sha| commit_committed_at_fetching(main_worktree, sha));
    crate::gate::ship::ShipApprovalPolicy {
        author_sessions,
        verified_authority: crate::gate::review_authority::verified_authority_keys(
            main_worktree,
            candidates,
        ),
        reviewed_committed_at,
        // BUG-1918: who is merging — `--override-stale-approval` is refused
        // to an author. trace:BUG-1918 | ai:claude
        shipper: crate::gate::review_authority::RecorderIdentity::current(
            &crate::gate::review_authority::visible_leases(main_worktree),
        )
        .tokens,
        // STORY-1655: the solo mode is re-read at ship time, never trusted
        // from the verdict. trace:STORY-1655 | ai:claude
        solo_human: crate::gate::solo_review::effective(main_worktree),
        verified_solo: crate::gate::review_authority::verified_solo_keys(candidates),
    }
}

/// [`commit_committed_at`], fetching a full sha from `origin` once when it is
/// not present locally (a reviewed commit since force-pushed away).
// trace:BUG-1918 | ai:claude
fn commit_committed_at_fetching(
    repo: &std::path::Path,
    sha: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    commit_committed_at(repo, sha).or_else(|| {
        let sha = sha.trim();
        if sha.len() != 40 || !sha.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let fetched = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["fetch", "--quiet", "--no-tags", "origin", sha])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // trace:BUG-2181 | ai:claude
            .remote_credentials(repo, "origin")
            .status()
            .is_ok_and(|s| s.success());
        fetched.then(|| commit_committed_at(repo, sha)).flatten()
    })
}

#[cfg(test)]
#[path = "tests/task_1448_drain_merge_approval_gate_tests.rs"]
mod task_1448_drain_merge_approval_gate_tests;

#[cfg(test)]
#[path = "tests/task_1458_pr_ship_approval_gate_tests.rs"]
mod task_1458_pr_ship_approval_gate_tests;
