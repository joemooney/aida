//! `aida human` review-awaiting list, advisor worklist, and the unblock/audit sweep (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// STORY-611: one row in the "Reviews awaiting you" bucket — a spec with a
/// code-review surface that the human still has to act on.
// trace:STORY-611 | ai:claude
pub(crate) struct ReviewAwaiting {
    pub(crate) spec_id: String,
    /// Short human-readable surface label (e.g. "PR-42" / "branch foo").
    pub(crate) surface: String,
    /// True when an Approved reviewer verdict already exists for the PR — the
    /// spec is in the awaiting-MERGE micro-state, NOT awaiting review.
    pub(crate) reviewed: bool,
    /// BUG-722: true when this is a loose draft WIP branch (a `Draft` spec on a
    /// pushed branch with commits but no open PR). Rendered under `wip-branches`
    /// but NOT a review gate — nothing awaits a human until there's a PR or a
    /// Done claim.
    // trace:BUG-722 | ai:claude
    pub(crate) wip: bool,
}

/// STORY-611: classify whether an open PR already carries an Approved reviewer
/// verdict (the State-3 "reviewed + approved → awaiting merge" signal). Reuses
/// the same verdict-file convention the orchestrator-resume probe reads:
/// `.aida/review-verdicts/PR-{n}.json`. trace:STORY-611 | ai:claude
pub(crate) fn pr_has_approved_verdict(project_root: &std::path::Path, pr_number: u64) -> bool {
    let path = project_root
        .join(".aida")
        .join("review-verdicts")
        .join(format!("PR-{pr_number}.json"));
    matches!(
        read_verdict_file(&path),
        Ok(auto_complete::ReviewerOutcome::Verdict(
            auto_complete::Verdict::Approved
        ))
    )
}

/// BUG-1672: the verdict a review COMMENT on the spec carries, when the
/// comment is one. Reviewers that only post prose (a fresh reviewer subagent,
/// a human at the keyboard) write no verdict file at all, so the comment is
/// the only record of the decision.
///
/// Only a comment that is plainly a review counts: its first line starts with
/// `VERDICT:`, or its head (the text before the first `:`) is `Review` or
/// begins `Review ` / `Review(` — `Review:`, `Review (fresh Opus reviewer):`,
/// `Review round 4 (...):`. Anything else (a proxy decision quoting a review,
/// an orchestrator note, the `REVIEW FINDINGS TO ADDRESS (` rework block) is
/// not a review. The verdict is read from the first line only: a
/// request-changes / reject word blocks regardless of anything else on the
/// line; `PARTIAL` counts as request-changes (as `VerdictKind::parse` does);
/// an approval word approves unless the line qualifies it (`WITHHELD`,
/// `PENDING`, `NOT APPROVED`), in which case it is `Unknown`, which never
/// approves. `APPROVE WITH NITS` is an approval — the wording the review
/// skill's accepted-with-nits path posts.
///
/// Returns `None` for a comment that is not a review at all.
// trace:BUG-1672 | ai:claude
pub(crate) fn review_comment_verdict(content: &str) -> Option<review_verdict::VerdictKind> {
    if review_verdict::is_findings_block(content) {
        return None;
    }
    let first = content.lines().find(|l| !l.trim().is_empty())?.trim();
    let lower = first.to_ascii_lowercase();
    let head = lower.split(':').next().unwrap_or(&lower).trim();
    let is_review = lower.starts_with("verdict:")
        || head == "review"
        || head.starts_with("review ")
        || head.starts_with("review(");
    if !is_review {
        return None;
    }
    // Word-level scan of the upper-cased first line: verdict words are written
    // in capitals by every writer, and the whole-word match keeps a prose
    // "reviewer" or "approved-by" from counting.
    let upper = first.to_ascii_uppercase();
    let words: Vec<&str> = upper
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .map(|w| w.trim_matches('_'))
        .filter(|w| !w.is_empty())
        .collect();
    let has = |w: &str| words.contains(&w);
    let has_seq = |a: &str, b: &str| words.windows(2).any(|p| p[0] == a && p[1] == b);
    let request_changes = has("REQUEST_CHANGES")
        || has("REQUESTCHANGES")
        || has_seq("REQUEST", "CHANGES")
        || has_seq("CHANGES", "REQUESTED")
        || has_seq("NEEDS", "CHANGES");
    if request_changes || has("PARTIAL") {
        return Some(review_verdict::VerdictKind::RequestChanges);
    }
    if has("REJECT") || has("REJECTED") {
        return Some(review_verdict::VerdictKind::Rejected);
    }
    if has("APPROVE") || has("APPROVED") || has("LGTM") {
        let qualified = has("WITHHELD")
            || has("PENDING")
            || has_seq("NOT", "APPROVED")
            || has_seq("NOT", "APPROVE");
        return Some(if qualified {
            review_verdict::VerdictKind::Unknown
        } else {
            review_verdict::VerdictKind::Approved
        });
    }
    Some(review_verdict::VerdictKind::Unknown)
}

/// BUG-1672: the LATEST review comment on the spec and the verdict it
/// carries. Top-level comments only, newest by `created_at`; the newest
/// comment that is a review decides, so a round-4 APPROVE after a round-3
/// REQUEST_CHANGES approves and a REQUEST_CHANGES after an earlier APPROVE
/// blocks.
///
/// Every review comment counts, whoever posted it: the author field is the
/// operator's name on reviewer and implementer comments alike, and the
/// session that moved the spec to done is not recorded in its history, so
/// there is no reliable way to tell the implementer's own "looks good" apart.
/// The strict first-line shape `review_comment_verdict` requires is the
/// guard instead.
// trace:BUG-1672 | ai:claude
pub(crate) fn latest_review_comment_verdict(
    comments: &[aida_core::Comment],
) -> Option<(review_verdict::VerdictKind, chrono::DateTime<chrono::Utc>)> {
    comments
        .iter()
        .filter_map(|c| review_comment_verdict(&c.content).map(|k| (k, c.created_at)))
        // trace:BUG-1672 | ai:codex — equal timestamps cannot clear a blocker.
        .max_by_key(|(kind, at)| (*at, !kind.approves()))
}

/// BUG-1672: when a verdict file was last written, as UTC. `None` when the
/// file is missing or the filesystem gives no modification time.
// trace:BUG-1672 | ai:claude
pub(crate) fn verdict_file_written_at(
    path: &std::path::Path,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(chrono::DateTime::<chrono::Utc>::from(modified))
}

/// BUG-1672: has an open PR for `req` already been approved by an independent
/// review? The one predicate `reviews_awaiting_human` uses to split
/// awaiting-review from awaiting-merge. Three records can approve:
///  1. the drain's PR-keyed verdict file (`PR-<n>.json`, the original signal);
///  2. the spec-keyed verdict file `aida review <SPEC>` records (not one a
///     merge has already closed out);
///  3. the latest review comment on the spec (the fresh-reviewer flow posts
///     only a comment).
///
/// The newest review wins across all eligible sources. An approval must be
/// strictly newer than every blocking candidate; ties or missing timestamps
/// cannot clear a blocker. File times remain filesystem mtimes (BUG-1681 owns
/// switching to recorded_at).
// trace:BUG-1672 | ai:codex
pub(crate) fn spec_review_approved(
    project_root: &std::path::Path,
    req: &aida_core::Requirement,
    spec_id: &str,
    pr_number: u64,
) -> bool {
    let mut candidates = Vec::new();
    if let Some((kind, at)) = latest_review_comment_verdict(&req.comments) {
        candidates.push((kind.approves(), Some(at)));
    }

    let pr_path = review_verdict::verdict_path(project_root, &format!("PR-{pr_number}"));
    if pr_path.is_file() {
        // Preserve the drain reader's conflict/escalation checks. An unknown
        // or unreadable verdict is blocking evidence, never an approval.
        candidates.push((
            pr_has_approved_verdict(project_root, pr_number),
            verdict_file_written_at(&pr_path),
        ));
    }

    let ids = [
        Some(spec_id),
        req.agreed_id.as_deref(),
        req.spec_id.as_deref(),
    ];
    for path in ids
        .into_iter()
        .flatten()
        .filter(|id| !id.trim().is_empty() && *id != "???")
        .map(|id| review_verdict::verdict_path(project_root, id))
        .filter(|p| p.is_file())
    {
        let record = std::fs::read_to_string(&path)
            .ok()
            .and_then(|body| review_verdict::parse_recorded_verdict(&body));
        if record.as_ref().is_some_and(|v| v.is_closed()) {
            continue;
        }
        candidates.push((
            record.is_some_and(|v| v.kind.approves()),
            verdict_file_written_at(&path),
        ));
    }
    review_candidates_approved(&candidates)
}

// trace:BUG-1672 | ai:codex
pub(crate) fn review_candidates_approved(
    candidates: &[(bool, Option<chrono::DateTime<chrono::Utc>>)],
) -> bool {
    candidates.iter().any(|(approves, at)| {
        *approves
            && candidates.iter().all(|(other_approves, other_at)| {
                *other_approves
                    || matches!((at, other_at), (Some(at), Some(other_at)) if at > other_at)
            })
    })
}

/// BUG-1291: any valid local reviewer decision means this PR has already
/// been reviewed. The orphan sweep must not turn RequestChanges or Rejected
/// back into fresh reviewer work merely because GitHub has no decision.
// trace:BUG-1291 | ai:codex
pub(crate) fn pr_has_local_verdict(project_root: &std::path::Path, pr_number: u64) -> bool {
    let path = project_root
        .join(".aida")
        .join("review-verdicts")
        .join(format!("PR-{pr_number}.json"));
    matches!(
        read_verdict_file(&path),
        Ok(auto_complete::ReviewerOutcome::Verdict(_))
            | Ok(auto_complete::ReviewerOutcome::EscalatedToHuman { .. })
    )
}

/// BUG-1549: gather every locally-recorded verdict candidate for `pr` — every
/// spec id parsed from the PR title (ALL of them, in title order, each read
/// directly rather than stopping at the first file on disk the way the old
/// `review_verdict::read_recorded_verdict_any` did), plus the PR-keyed
/// record. The ONLY verdict read on the awaiting-you review path: its
/// result feeds `awaiting_you::classify_pr_review` via `pr_review_decision`.
// trace:BUG-1549 | ai:claude
pub(crate) fn verdict_candidates_for_pr(
    project_root: &std::path::Path,
    pr: &status_cleanup::OpenPrItem,
) -> Vec<review_verdict::RecordedVerdict> {
    let mut candidates: Vec<review_verdict::RecordedVerdict> =
        pr_ship::extract_spec_ids_from_text(&pr.title)
            .iter()
            .filter(|id| !id.trim().is_empty())
            .filter_map(|id| review_verdict::read_recorded_verdict(project_root, id))
            .collect();
    if let Some(pr_verdict) =
        review_verdict::read_recorded_verdict(project_root, &format!("PR-{}", pr.number))
    {
        candidates.push(pr_verdict);
    }
    candidates
}

/// BUG-1549: the ONE review decision for `pr` — `awaiting_you::classify_pr_review`
/// over `verdict_candidates_for_pr`. Both the mergeable suppression set
/// (`local_suppressed_prs`) and the review rows (`pr_review_rows`) derive
/// from this and nothing else, so "suppressed" and "has a row explaining it"
/// cannot disagree.
// trace:BUG-1549 | ai:claude
pub(crate) fn pr_review_decision(
    project_root: &std::path::Path,
    pr: &status_cleanup::OpenPrItem,
) -> awaiting_you::PrReviewDecision {
    let candidates = verdict_candidates_for_pr(project_root, pr);
    awaiting_you::classify_pr_review(&candidates, pr.head_sha.as_deref())
}

/// BUG-1549: the set of PR numbers a LOCAL (no-network) verdict record
/// removes from the mergeable set — exactly the PRs whose
/// `pr_review_decision` is `suppressed`.
// trace:BUG-1549 | ai:claude
pub(crate) fn local_suppressed_prs(
    project_root: &std::path::Path,
    prs: &[status_cleanup::OpenPrItem],
) -> std::collections::HashSet<u64> {
    prs.iter()
        .filter(|pr| pr_review_decision(project_root, pr).suppressed)
        .map(|pr| pr.number)
        .collect()
}

/// BUG-1549: the report's review rows (blocked / stale-approval /
/// rework-ready) for `prs`, each rendered from that PR's
/// `pr_review_decision`. A head-less PR is not skipped: its decision can
/// suppress, so its row must exist.
// trace:BUG-1549 | ai:claude
// STORY-1420: production uses `pr_review_rows_routed`; this keeps the
// STORY-1419 every-recorder-live contract for the plumbing tests.
#[cfg(test)]
pub(crate) fn pr_review_rows<'a>(
    project_root: &std::path::Path,
    prs: impl IntoIterator<Item = &'a status_cleanup::OpenPrItem>,
    seat: Option<&str>,
) -> awaiting_you::PrReviewRows {
    let mut rows = awaiting_you::PrReviewRows::default();
    for pr in prs {
        let decision = pr_review_decision(project_root, pr);
        rows.add(
            pr.number,
            pr.head_sha.as_deref(),
            &pr.head_branch,
            &decision,
            seat,
        );
    }
    rows
}

/// STORY-1420: [`pr_review_rows`] with the rework-ready row routed by the
/// recorder's liveness (agent registry + pid, never the network).
// trace:STORY-1420 | ai:claude
pub(crate) fn pr_review_rows_routed<'a>(
    project_root: &std::path::Path,
    prs: impl IntoIterator<Item = &'a status_cleanup::OpenPrItem>,
    reader: awaiting_you::ReworkReader<'_>,
) -> awaiting_you::PrReviewRows {
    let mut rows = awaiting_you::PrReviewRows::default();
    let liveness = |who: &str| {
        awaiting_you::classify_recorder(who, |name| {
            agent_registry::named_agent_liveness(project_root, name)
        })
    };
    for pr in prs {
        let decision = pr_review_decision(project_root, pr);
        rows.add_routed(
            pr.number,
            pr.head_sha.as_deref(),
            &pr.head_branch,
            &decision,
            reader,
            liveness,
        );
    }
    rows
}

/// BUG-550: the set of SPEC-IDs referenced by commits that exist on some ref
/// but are NOT yet reachable from the default branch — i.e. specs with an
/// in-flight (unmerged) review surface. This is the cheap prefilter that lets
/// the reviews bucket widen its candidate set past the Done specs WITHOUT a
/// regression: one `git log --all --not <default>` pass replaces N per-spec
/// `git log --all --grep` history scans, and the full per-spec linkage probe
/// then runs only for this small set (the handful with open work) rather than
/// every active spec. A spec whose commits are all on the default branch is
/// merged (Shipped) and correctly absent — no false negatives for a genuinely
/// open surface; the at-worst false positive (local default behind origin)
/// is harmless, since the per-spec classifier re-checks and drops it. Ids are
/// resolved through the same trailer / `trace:` parsers `collect_git_linkage`
/// uses, so membership matches what the classifier later confirms.
/// trace:BUG-550 | ai:claude
pub(crate) fn specs_with_unmerged_commits(
    project_root: &std::path::Path,
) -> std::collections::HashSet<String> {
    use std::process::Command as PCmd;
    let mut out = std::collections::HashSet::new();
    let Some(default_ref) = resolve_default_branch_ref(project_root) else {
        return out;
    };
    let log = PCmd::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "log",
            "--all",
            "--not",
            &default_ref,
            "--pretty=format:%B%x1e",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    for record in log.split('\u{1e}') {
        let body = record.trim();
        if body.is_empty() {
            continue;
        }
        let mut resolved = extract_spec_ids_from_commit(body);
        resolved.extend(extract_referenced_spec_ids_from_commit(body));
        resolved.extend(extract_trace_line_spec_ids(body));
        for r in resolved {
            out.insert(r.to_ascii_uppercase());
        }
    }
    out
}

/// STORY-611: enumerate the specs that have a code-review surface awaiting the
/// human — branch / open-PR specs not yet merged, the SAME surface `aida
/// review <SPEC>` locates. Candidate set (BUG-550) = every non-archived spec
/// that isn't `Completed` (merged) or `Rejected` (abandoned) — work that could
/// still carry an open PR, regardless of whether someone ran `aida queue
/// done`. For each we run the exact surface-detection `handle_review_spec`
/// uses (`collect_git_linkage_opts` + `change_lookup_for_branch` +
/// `classify_review_surface`) and keep only the `OpenChange` / `BranchNoChange`
/// surfaces; `Shipped` (already merged) and `Local` (never pushed) are dropped,
/// so the open-PR linkage — not the Done flag — is the real gate. Each row is
/// tagged `reviewed` so the caller can split awaiting-review from the
/// awaiting-MERGE micro-state (the REFINEMENT comment's case 3).
/// trace:STORY-611 trace:BUG-550 | ai:claude
pub(crate) fn reviews_awaiting_human(
    project_root: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
) -> Vec<ReviewAwaiting> {
    // BUG-550: the candidate set is "specs that could have an OPEN review
    // surface", NOT "specs someone remembered to mark Done". Gating on
    // `status == Done` made an open PR invisible whenever the PR was opened at
    // the keyboard without a `queue done` (observed: PR #879 / BUG-549 stayed
    // Approved). Substrate-as-bouncer: let the open-PR linkage be the gate
    // (`classify_review_surface` drops merged/never-pushed specs), not a
    // manually-maintained status flag.
    //
    // We still exclude Completed (merged ⇒ classifies as `Shipped` ⇒ dropped
    // anyway) and Rejected (abandoned, not headed to merge) purely to bound
    // cost — those are the bulk of a mature store. The remaining set
    // (Draft/Approved/Planned/InProgress/Done/NeedsAttention) is "active,
    // not-yet-shipped" work.
    //
    // To keep this hot, offline command fast even with the wider set, a single
    // `git log` (`specs_with_unmerged_commits`) names the specs that actually
    // have unmerged commits; the per-spec linkage probe — and its skip of the
    // source-tree walk via `collect_git_linkage_opts(.., false)` — then runs
    // ONLY for those. Active specs with nothing in flight cost nothing beyond
    // the one shared log pass. trace:BUG-550 | ai:claude
    let candidates: Vec<aida_core::Requirement> = match backend.list_requirements(false) {
        Ok(reqs) => reqs
            .into_iter()
            .filter(|r| {
                !r.archived
                    && !matches!(
                        r.status,
                        aida_core::RequirementStatus::Completed
                            | aida_core::RequirementStatus::Rejected
                    )
            })
            .collect(),
        Err(_) => return Vec::new(),
    };
    let unmerged = specs_with_unmerged_commits(project_root);
    let open_prs = collect_open_prs(project_root);
    let mut out = Vec::new();
    for req in &candidates {
        let Some(spec_id) = req.spec_id.clone().or_else(|| req.agreed_id.clone()) else {
            continue;
        };
        // BUG-550: skip the per-spec history scan for specs with no unmerged
        // commit — they can only classify as Shipped/Local and be dropped, so
        // probing them is pure cost. This is what keeps the widened candidate
        // set from regressing `aida human`'s latency. trace:BUG-550 | ai:claude
        if !unmerged.contains(&spec_id.to_ascii_uppercase()) {
            continue;
        }
        let ids = vec![spec_id.clone()];
        let linkage = collect_git_linkage_opts(project_root, &ids, false);
        let change = linkage
            .branch
            .as_deref()
            .map(|b| change_lookup_for_branch(project_root, b));
        let surface = classify_review_surface(&linkage, change);
        // BUG-582: the load-bearing invariant — a Completed/Rejected spec, or a
        // merged/never-pushed surface, is NEVER reviews-awaiting. This is the
        // gate the count and the render both flow through (the single `out`
        // vec), so they stay consistent (BUG-579). The candidate pre-filter
        // above already drops most of these for cost, but it reads the cache;
        // this re-checks the live status so a stale cache row left over from a
        // lingering Agent-tool worktree branch can't slip a finished spec back
        // onto the operator's seat. trace:BUG-582 | ai:claude
        // BUG-722: the single classification gate. Layers view-state (deferred/
        // archived hidden exactly as `aida list` hides them), branch-type (the
        // `aida-store` orphan store branch is never a code-review target), and
        // the draft-WIP split on top of the BUG-582 status/surface invariant.
        // Pure ⇒ the false-positive classes are unit-tested without a repo.
        // trace:BUG-722 | ai:claude
        let bucket = classify_human_review_bucket(
            req.status.clone(),
            req.archived,
            requirement_is_deferred(req),
            &surface,
        );
        if bucket == HumanReviewBucket::Excluded {
            continue;
        }
        let wip = bucket == HumanReviewBucket::WipBranch;
        match surface {
            ReviewSurface::OpenChange {
                number, ref branch, ..
            } => {
                if open_prs.by_branch.get(branch).is_some_and(|pr| pr.is_draft) {
                    continue;
                }
                let forge = crate::forge::resolve_forge_kind(project_root);
                // BUG-1672: the PR-keyed file is only the drain's record; a
                // keyboard `aida review` writes the spec-keyed file and a
                // fresh-reviewer subagent posts only a comment. Read all three
                // so an already-approved spec lands under awaiting-merge, not
                // back on the operator's review seat. trace:BUG-1672 | ai:claude
                let reviewed = spec_review_approved(project_root, req, &spec_id, number);
                out.push(ReviewAwaiting {
                    spec_id,
                    surface: format!("{}-{}", forge.change_noun(), number),
                    reviewed,
                    wip,
                });
            }
            ReviewSurface::BranchNoChange { branch, .. } => {
                out.push(ReviewAwaiting {
                    spec_id,
                    surface: format!("branch {branch}"),
                    // No PR ⇒ no verdict file ⇒ never the awaiting-merge state.
                    reviewed: false,
                    wip,
                });
            }
            // Already merged or never pushed — nothing for the human to review.
            // (Unreachable: `classify_human_review_bucket` already dropped these
            // above; kept exhaustive for the match.)
            ReviewSurface::Shipped { .. } | ReviewSurface::Local => {}
        }
    }
    out.sort_by(|a, b| a.spec_id.cmp(&b.spec_id));
    out
}

pub(crate) fn handle_list_human(short: bool, backend: &aida_core::CachedGitBackend) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;
    let in_flight_scopes = in_flight_lease_role_map(&project_root);
    let facts = collect_open_facts(&store, &in_flight_scopes);
    // STORY-620: the operator/advisor seat split is a resolved policy now — the
    // configurable buckets (to-groom/decompose/ready-to-close/triage) show here
    // only when assigned to the operator. trace:STORY-620
    let seat_policy = seats::SeatPolicy::load(&project_root);

    // trace:BUG-535 — display-id → title, built from the SAME store load the
    // facts came from (no extra scan, no N+1), so every bucket can render the
    // spec's title beside its bare id via the shared `spec_title_cell` helper
    // (the same id+title rendering BUG-532 added for `burndown plan`).
    let titles: std::collections::HashMap<String, String> = store
        .requirements
        .iter()
        .filter_map(|req| {
            let id = req
                .agreed_id
                .clone()
                .or_else(|| req.spec_id.clone())
                .unwrap_or_else(|| req.id.to_string());
            (!req.title.is_empty()).then(|| (id, req.title.clone()))
        })
        .collect();

    // TASK-747: specs explicitly routed `--for human` are part of the
    // human-attention set even when the status/tag classifier wouldn't derive
    // them. We UNION the explicitly-routed members with the derived membership
    // (the route is additive, never subtractive — Risk #6 in the SPIKE-57
    // design doc). trace:TASK-747 | ai:claude
    let routed_ids = human_routed_spec_ids(&project_root, &store);

    // Classify with the SAME classifier as `burndown explain`, then keep only
    // the specs the canonical `human_required` predicate (SPIKE-57/TASK-746)
    // classifies as needing a human — the rest self-resolve. Routing the filter
    // through the named predicate keeps this view and the predicate from
    // drifting. BUG-564: the orthogonal `human_only` marker is now folded in at
    // the view layer — `collect_open_facts` enriches each fact with
    // `req.human_only`, so a spec that is human-required PURELY via the marker
    // (e.g. init's High "Commit AIDA scaffolding" task, a non-human bucket)
    // surfaces here exactly as it does in the queue's human-only bucket, which
    // reads the SAME `req.human_only`. Before this fix the marker was fed as a
    // hardcoded `false`, so the two human views contradicted each other on a new
    // user's first interaction. trace:STORY-562 trace:TASK-746 trace:BUG-564
    let classified: Vec<(
        burndown::OpenFacts,
        burndown::OpenBucket,
        Vec<burndown::Reason>,
    )> = facts
        .into_iter()
        .map(|f| {
            let (bucket, reasons) = burndown::explain_reasons(&f);
            (f, bucket, reasons)
        })
        // STORY-618/620: keep the derived buckets assigned to the OPERATOR
        // seat. The fixed-operator buckets (held-for-review, build-supervised)
        // always qualify; the configurable buckets (to-groom / decompose /
        // ready-to-close) qualify only when the seat policy assigns them to the
        // operator (default: advisor → they show on `aida advisor`, not here).
        // `operator_seat` also excludes `AwaitingDecision`, rendered richer by
        // the first-class "decisions-awaiting" bucket below. trace:STORY-620
        .filter(|(f, bucket, _)| {
            // BUG-564: a spec carrying the `human_only` marker is human-required
            // regardless of its derived bucket — the canonical `human_required`
            // predicate ORs the marker in, and so must this view. Without this
            // clause a human-only spec whose bucket is NOT a needs-human bucket
            // (the init scaffolding-commit task is High/Approved → a non-human
            // bucket) was invisible here while the queue showed it.
            // trace:BUG-564 | ai:claude
            //
            // BUG-572: but `human_only` must NOT override the to-groom→advisor
            // seat routing. An ungroomed Draft (the `Ungroomed` / to-groom
            // bucket) carrying `human_only=true` is still advisor grooming work —
            // you cannot make a human decision on a draft that has no formulated
            // question. Excluding `Ungroomed` here keeps it off the operator seat;
            // `handle_advisor_worklist` still surfaces it under to-groom (its
            // filter gates on `advisor_seat()`, not `human_only`). A GROOMED
            // human_only spec (any non-`Ungroomed` bucket — e.g. BUG-564's
            // High/Approved scaffolding task) still reaches the operator.
            // trace:BUG-572 | ai:claude
            (f.human_only && !matches!(bucket, burndown::OpenBucket::Ungroomed))
                || bucket.operator_seat()
                || (seats::CONFIGURABLE_KEYS.contains(&bucket.key())
                    && seat_policy.seat_of(bucket.key()) == seats::Seat::Operator)
        })
        .collect();

    // TASK-747: the explicitly-routed members the derived classification did
    // NOT already surface — shown as their own "routed to you" group so the
    // operator sees push-routed work alongside the derived bottleneck. De-dupe
    // by SPEC-ID against the derived set (Risk #5). trace:TASK-747 | ai:claude
    let derived_ids: std::collections::HashSet<String> = classified
        .iter()
        .map(|(f, _, _)| f.id.to_ascii_uppercase())
        .collect();
    let mut routed_only: Vec<String> = routed_ids
        .iter()
        .filter(|id| !derived_ids.contains(*id))
        .cloned()
        .collect();
    routed_only.sort();

    // STORY-611: the THREE first-class pending-for-human buckets, each → its
    // drain verb. The banner already promises "a decision, review, or triage";
    // deliver all three as real lists, not pointers. trace:STORY-611
    //  1. Decisions awaiting you  → `aida questions answer <spec> <choice>`
    //  2. Reviews awaiting you    → `aida review <SPEC>` (the must-have)
    //  3. Triage (findings)       → `aida findings list`
    let pending_decisions = collect_decision_requests(backend)
        .map(|(pending, _)| pending)
        .unwrap_or_default();
    let reviews_all = reviews_awaiting_human(&project_root, backend);
    // BUG-722: loose draft WIP branches (a `Draft` spec on a pushed branch with
    // no PR) are visible but NOT a review gate — split them out first so they
    // never masquerade as reviews-awaiting. trace:BUG-722
    let wip_branches: Vec<&ReviewAwaiting> = reviews_all.iter().filter(|r| r.wip).collect();
    // Split case 1 (code-review-needed) from case 3 of the REFINEMENT comment
    // (already reviewed → awaiting MERGE, a distinct micro-state, NOT a review).
    let (awaiting_merge, awaiting_review): (Vec<&ReviewAwaiting>, Vec<&ReviewAwaiting>) =
        reviews_all
            .iter()
            .filter(|r| !r.wip)
            .partition(|r| r.reviewed);
    // Findings stay a pointer + live count (cheap, no recompute): open findings
    // targeting an open spec. STORY-620: triage is a configurable seat; it only
    // counts toward (and renders on) the operator's list when the policy
    // assigns `triage` to the operator (default: advisor → `aida advisor`).
    // trace:STORY-611 trace:STORY-620
    let open_findings = if seat_policy.seat_of("triage") == seats::Seat::Operator {
        collect_findings_by_spec(&store)
            .values()
            .flat_map(|ids| ids.iter())
            .collect::<std::collections::HashSet<_>>()
            .len()
    } else {
        0
    };

    // `--short`: bare IDs, one per line — usable in `$(...)` / xargs. No
    // header, footer, color, or grouping (mirrors `aida list --short`).
    // Derived members first, then the explicitly-routed-only tail (TASK-747).
    if short {
        for (f, _, _) in &classified {
            println!("{}", f.id);
        }
        for id in &routed_only {
            println!("{id}");
        }
        return Ok(());
    }

    println!(
        "{} what needs a human",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    let nothing = classified.is_empty()
        && routed_only.is_empty()
        && pending_decisions.is_empty()
        && awaiting_review.is_empty()
        && awaiting_merge.is_empty()
        && wip_branches.is_empty()
        && open_findings == 0;
    if nothing {
        println!(
            "  {}",
            "Nothing needs you right now — everything open is in flight, deferred, \
             or self-resolving."
                .dimmed()
        );
        return Ok(());
    }
    let total = classified.len()
        + routed_only.len()
        + pending_decisions.len()
        + awaiting_review.len()
        + open_findings;
    // BUG-722: the header counts true gates (decision/review/triage). When the
    // only open items are informational (awaiting-merge or loose wip-branches),
    // skip the "0 items need …" line and let those sections speak for
    // themselves. trace:BUG-722
    if total > 0 {
        println!(
            "  {} {} open {} need a decision, review, or triage from you",
            "→".green(),
            total,
            if total == 1 { "item" } else { "items" },
        );
    }

    // STORY-611 — bucket 1: Decisions awaiting you. Upgrade from the old
    // one-line pointer to a real list of the pending DecisionRequests; each
    // resolvable inline with `aida human answer <spec> <choice>`.
    // trace:STORY-611
    if !pending_decisions.is_empty() {
        println!(
            "\n{} {} — {}",
            "●".yellow(),
            "decisions-awaiting".bold(),
            "answer with `aida human answer <spec> <choice>`".dimmed()
        );
        for req in &pending_decisions {
            let display_id = req.display_id();
            let title = spec_title_cell(&titles, &display_id);
            println!("    {}{}", display_id.cyan(), title);
            if let Some(dr) = &req.decision_request {
                println!("      {} {}", "?".magenta(), dr.question.dimmed());
                for (i, choice) in dr.choices.iter().enumerate() {
                    let rec = if dr.recommended == Some(i) {
                        " (recommended)".green().to_string()
                    } else {
                        String::new()
                    };
                    println!(
                        "        {}{} {}",
                        format!("{}.", i + 1).bold(),
                        rec,
                        choice.label.dimmed()
                    );
                }
            }
        }
    }

    // STORY-611 — bucket 2: Reviews awaiting you (the must-have). Specs with a
    // code-review surface (open PR / branch-with-commits) not yet merged — the
    // SAME surface `aida review <SPEC>` locates. Each line → `aida review
    // <SPEC>` (or the `aida human review <SPEC>` alias). trace:STORY-611
    if !awaiting_review.is_empty() {
        println!(
            "\n{} {} — {}",
            "●".yellow(),
            "reviews-awaiting".bold(),
            "review with `aida human review <spec>` (or `aida review <spec>`)".dimmed()
        );
        for r in &awaiting_review {
            let title = spec_title_cell(&titles, &r.spec_id);
            println!(
                "    {}{} {}",
                r.spec_id.cyan(),
                title,
                format!("[{}]", r.surface).dimmed()
            );
        }
    }

    // STORY-611 — the awaiting-MERGE micro-state (REFINEMENT case 3): already
    // reviewed + approved, just needs the merge button. A distinct indicator,
    // NOT part of the reviews bucket. trace:STORY-611
    if !awaiting_merge.is_empty() {
        println!(
            "\n{} {} — {}",
            "●".green(),
            "awaiting-merge".bold(),
            "reviewed + approved — just needs merging".dimmed()
        );
        for r in &awaiting_merge {
            let title = spec_title_cell(&titles, &r.spec_id);
            println!(
                "    {}{} {}",
                r.spec_id.cyan(),
                title,
                format!("[{}]", r.surface).dimmed()
            );
        }
    }

    // BUG-722 — loose work-in-progress: a draft spec sitting on a pushed WIP
    // branch with no PR. Surfaced so it isn't invisible, but explicitly NOT a
    // review gate — nothing awaits the human until there's a PR or a Done claim
    // (operator decision 2026-07-12). A hollow marker distinguishes it from the
    // solid gate buckets above. trace:BUG-722
    if !wip_branches.is_empty() {
        println!(
            "\n{} {} — {}",
            "○".dimmed(),
            "wip-branches".bold(),
            "loose draft work-in-progress — not awaiting review".dimmed()
        );
        for r in &wip_branches {
            let title = spec_title_cell(&titles, &r.spec_id);
            println!(
                "    {}{} {}",
                r.spec_id.cyan(),
                title,
                format!("[{}]", r.surface).dimmed()
            );
        }
    }

    // Group by bucket, ordered most-actionable-first (matches the precedence in
    // `explain_open` / the explain view). trace:STORY-562
    //
    // STORY-611: `AwaitingDecision` is intentionally DROPPED from this derived
    // set — the first-class "decisions-awaiting" bucket above supersedes it
    // (same specs, richer output: the actual choices). Keeping both would
    // double-list every pending decision, exactly the conflation the
    // REFINEMENT comment warns against. trace:STORY-611
    // STORY-618/620: render order for the derived buckets. The fixed-operator
    // buckets (held-for-review, build-supervised) always belong here; the
    // configurable ones (ready-to-close / to-groom / decompose) appear only when
    // the seat policy moved them to the operator — the `classified` filter above
    // already dropped the rest, so listing them here just sets their order.
    // trace:STORY-620 (was BUG-543's ReadyToClose-leads order)
    // BUG-579: the render order is the shared `HUMAN_DERIVED_BUCKET_ORDER`
    // constant so the catch-all below (and its test) check against the SAME
    // source of truth — count and render can never silently disagree.
    // trace:BUG-579 | ai:claude
    let order = burndown::HUMAN_DERIVED_BUCKET_ORDER;
    for bucket in order {
        let rows: Vec<&(
            burndown::OpenFacts,
            burndown::OpenBucket,
            Vec<burndown::Reason>,
        )> = classified.iter().filter(|(_, b, _)| *b == bucket).collect();
        if rows.is_empty() {
            continue;
        }
        // The bucket header glosses the derived reason — shared by every row in
        // the bucket (the first reason of the first row).
        let sample_reason = rows[0]
            .2
            .first()
            .map(|r| r.text.clone())
            .unwrap_or_default();
        println!(
            "\n{} {} — {}",
            "●".yellow(),
            bucket.key().bold(),
            sample_reason.dimmed()
        );
        for (f, _, reasons) in rows {
            // Repeat the derived reason only when it differs from the header
            // (e.g. per-spec decision tags); otherwise just the ID + title stays
            // scannable. trace:BUG-535 — append the (truncated) title beside the
            // id via the shared `spec_title_cell` helper so the bucket is a
            // readable wall of titles, not bare ids.
            let derived = reasons.first().map(|r| r.text.clone()).unwrap_or_default();
            let title = spec_title_cell(&titles, &f.id);
            if derived == sample_reason {
                println!("    {}{}", f.id.cyan(), title);
            } else {
                println!(
                    "    {}{} {}",
                    f.id.cyan(),
                    title,
                    format!("({derived})").dimmed()
                );
            }
            // Fold in the additional reasons (finding-links + residual notes)
            // so the operator sees every reason it's still open — same as
            // `burndown explain`. trace:STORY-562
            for r in reasons.iter().skip(1) {
                let tag = match r.source {
                    burndown::ReasonSource::Finding => "finding".magenta(),
                    burndown::ReasonSource::Residual => "note".yellow(),
                    burndown::ReasonSource::Derived => continue,
                };
                println!("      {} {}", tag, r.text.dimmed());
            }
        }
    }

    // BUG-579: catch-all — render EVERY classified row that the ordered loop and
    // the first-class decisions-awaiting bucket did NOT already show. Without
    // this, any classified spec whose bucket isn't in `order` and which lacks a
    // posed `DecisionRequest` (the canonical case: an `AwaitingDecision` Spike
    // with no formal question) is COUNTED in `total` but never RENDERED — the
    // phantom "N items need a human" with no bucket to act on. This restores the
    // count==render invariant and future-proofs against any new OpenBucket
    // variant. The `has_pending_decision` rows are deliberately NOT re-rendered
    // here (the decisions-awaiting bucket already lists them, richer).
    // trace:BUG-579 | ai:claude
    let needs_attention: Vec<&(
        burndown::OpenFacts,
        burndown::OpenBucket,
        Vec<burndown::Reason>,
    )> = classified
        .iter()
        .filter(|(f, b, _)| !burndown::classified_row_is_rendered(*b, f.has_pending_decision))
        .collect();
    if !needs_attention.is_empty() {
        println!(
            "\n{} {} — {}",
            "●".yellow(),
            "needs-attention".bold(),
            "counted above — surface or groom so nothing is invisible".dimmed()
        );
        for (f, bucket, reasons) in needs_attention {
            let title = spec_title_cell(&titles, &f.id);
            println!("    {}{}", f.id.cyan(), title);
            // Lead with the bucket-specific hint (AwaitingDecision-without-a-
            // posed-question gets the actionable "pose a question / groom" line);
            // then fold in any derived reason for context.
            println!(
                "      {} {}",
                "?".magenta(),
                burndown::needs_attention_hint(*bucket).dimmed()
            );
            if let Some(r) = reasons.first() {
                if !r.text.is_empty() {
                    println!("      {} {}", "note".yellow(), r.text.dimmed());
                }
            }
        }
    }

    // TASK-747: the explicitly-routed-only group — specs pushed `--for human`
    // that the derived classifier didn't already surface. Listed last because
    // the derived buckets are most-actionable-first; an explicit route is a
    // deliberate "this needs you" the operator filed, shown so push-routed work
    // never gets lost behind derived membership. trace:TASK-747 | ai:claude
    if !routed_only.is_empty() {
        println!(
            "\n{} {} — {}",
            "●".yellow(),
            "routed-to-human".bold(),
            "explicitly routed to you (`queue add --for human`)".dimmed()
        );
        for id in &routed_only {
            // trace:BUG-535 — title beside the id, same as the derived buckets.
            println!("    {}{}", id.cyan(), spec_title_cell(&titles, id));
        }
    }

    // STORY-611 — bucket 3: Triage (findings). A pointer + live count is the
    // accepted shape (cheap; the full list lives in `aida findings list`).
    // trace:STORY-562 trace:STORY-611
    if open_findings > 0 {
        println!(
            "\n{} {} — {}",
            "●".magenta(),
            "triage-findings".bold(),
            format!(
                "{} finding{} awaiting triage — `aida findings list`",
                open_findings,
                if open_findings == 1 { "" } else { "s" }
            )
            .dimmed()
        );
    }

    // TASK-823: only print pointers that lead somewhere actionable — an
    // unconditional "see `aida questions list` / `aida findings list`" footer is
    // noise when those views are empty (and `questions list` shows only ANSWERED
    // decisions, so pointing at it as an "inbox" misleads). Build the footer from
    // the parts that actually have content. trace:TASK-823 | ai:claude
    let mut hints: Vec<String> = Vec::new();
    // Pending decisions already render as their own bucket above; the pointer is
    // only useful as "the rest of the inbox, incl. answered" — show it only when
    // there's a decision history to browse.
    if !pending_decisions.is_empty() {
        hints.push("answered decisions: `aida questions list`".to_string());
    }
    if open_findings > 0 {
        hints.push("all findings: `aida findings list`".to_string());
    }
    // STORY-618/622: cross-link the advisor worklist only when it has work — an
    // ungroomed draft to groom, or a finding to triage. (Cheap store scan; the
    // full set is `aida advisor`.) A solo operator wearing both hats then knows
    // the second list to check; when there's no advisor work, no noise.
    let has_ungroomed_draft = store.requirements.iter().any(|r| {
        !r.archived
            && !r.deferred
            && matches!(r.status, RequirementStatus::Draft)
            && !findings::is_finding(&r.tags.iter().cloned().collect::<Vec<_>>())
    });
    let has_triage = collect_findings_by_spec(&store)
        .values()
        .any(|ids| !ids.is_empty());
    let advisor_has_work = has_ungroomed_draft || has_triage;
    if advisor_has_work {
        hints.push("advisor work: `aida advisor`".to_string());
    }
    if !hints.is_empty() {
        println!(
            "\n  {} {}",
            crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
            hints.join(" · ").dimmed()
        );
    }
    Ok(())
}

/// STORY-618: bare `aida advisor` — the advisor's actionable worklist, the
/// mirror of bare `aida human` but grouped by ADVISOR action. The seat
/// complement of [`handle_list_human`]: the routine dispositions that need
/// cross-spec awareness + product judgment (not the operator's strategic
/// sign-off). Five buckets, each pointing at its canonical verb:
///   GROOM    — ungroomed drafts awaiting approve/defer/reject
///   DECOMPOSE/CLOSE — umbrella epics to decompose or close (advisor_seat)
///   DISTILL  — under-specified specs to turn into pre-recorded questions
///   TRIAGE   — findings awaiting triage
///   BLESS    — approved-but-unqueued backlog ready to groom onto the queue
/// Read-only; writes nothing. trace:STORY-618 | ai:claude
pub(crate) fn handle_advisor_worklist(
    short: bool,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;

    // Shared id→title cell (same construction as `aida human`). trace:BUG-535
    let titles: std::collections::HashMap<String, String> = store
        .requirements
        .iter()
        .filter_map(|req| {
            let id = req
                .agreed_id
                .clone()
                .or_else(|| req.spec_id.clone())
                .unwrap_or_else(|| req.id.to_string());
            (!req.title.is_empty()).then(|| (id, req.title.clone()))
        })
        .collect();

    // STORY-620: the configurable buckets (to-groom/decompose/ready-to-close/
    // triage) appear here only when the seat policy assigns them to the advisor
    // (the default).
    let seat_policy = seats::SeatPolicy::load(&project_root);

    // GROOM / DECOMPOSE / CLOSE come from the SAME open-facts classifier the
    // human view uses — keep the advisor_seat buckets the policy assigns to the
    // advisor. trace:STORY-618 trace:STORY-620
    let in_flight_role_map = in_flight_lease_role_map(&project_root);
    let advisor_rows: Vec<(
        burndown::OpenFacts,
        burndown::OpenBucket,
        Vec<burndown::Reason>,
    )> = collect_open_facts(&store, &in_flight_role_map)
        .into_iter()
        .map(|f| {
            let (bucket, reasons) = burndown::explain_reasons(&f);
            (f, bucket, reasons)
        })
        .filter(|(_, bucket, _)| {
            bucket.advisor_seat() && seat_policy.seat_of(bucket.key()) == seats::Seat::Advisor
        })
        .collect();

    // DISTILL — under-specified specs the sweep would flag (minus live leases /
    // BUG-495 exclusions). Reuses the exact set `aida clarify` resolves.
    let distill = clarify_default_specs(backend).unwrap_or_default();

    // TRIAGE — findings still awaiting triage (draft + finding tag), distinct.
    // STORY-620: only when `triage` is assigned to the advisor (the default).
    let triage = if seat_policy.seat_of("triage") == seats::Seat::Advisor {
        collect_findings_by_spec(&store)
            .values()
            .flat_map(|ids| ids.iter())
            .collect::<std::collections::HashSet<_>>()
            .len()
    } else {
        0
    };

    // BLESS — approved, unblocked, unqueued, not-in-flight specs the advisor
    // could groom onto the queue (queueing IS the ADR-3 sign-off).
    let queued_ids = all_queued_requirement_ids(&project_root);
    let in_flight_scopes = in_flight_lease_scopes(&project_root);
    let bless: Vec<String> = store
        .requirements
        .iter()
        .filter(|r| !r.archived && !r.deferred)
        .filter(|r| matches!(r.status, RequirementStatus::Approved))
        .filter(|r| !queued_ids.contains(&r.id))
        .filter(|r| !findings::is_finding(&r.tags.iter().cloned().collect::<Vec<_>>()))
        .filter(|r| !aida_core::pickability::blocked_by_incomplete(r, &store))
        .filter(|r| {
            let live = !in_flight_scopes.is_empty()
                && [r.agreed_id.as_deref(), r.spec_id.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|s| in_flight_scopes.contains(&s.to_ascii_lowercase()));
            !live
        })
        .map(|r| r.display_id())
        .collect();

    // `--short`: bare IDs of the actionable groom/decompose/close specs, one
    // per line (mirrors `aida human --short`). The pointer buckets
    // (distill/triage/bless) are summaries, not part of the id stream.
    if short {
        for (f, _, _) in &advisor_rows {
            println!("{}", f.id);
        }
        for id in &bless {
            println!("{id}");
        }
        return Ok(());
    }

    println!(
        "{} the advisor's worklist",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    let nothing = advisor_rows.is_empty() && distill.is_empty() && triage == 0 && bless.is_empty();
    if nothing {
        println!(
            "  {}",
            "Nothing on the advisor's plate — no ungroomed drafts, no findings to \
             triage, no backlog to bless, nothing to close."
                .dimmed()
        );
        return Ok(());
    }

    // GROOM / DECOMPOSE / CLOSE buckets — full id+title lists (the real work),
    // most-actionable-first (CLOSE leads: a delivered epic is the cheapest win).
    use burndown::OpenBucket::*;
    let groups = [
        (
            ReadyToClose,
            "close",
            "mark the delivered epic Completed (`aida edit <id> --status completed`)",
        ),
        (
            Ungroomed,
            "groom",
            "weigh against the graph (overlap / staleness / scope), then dispose: \
             `aida edit <id> --status approved|rejected` · `aida defer <id> --until <trigger>` · or refine",
        ),
        (
            Umbrella,
            "decompose",
            "decompose into children or complete them",
        ),
    ];
    for (bucket, key, action) in groups {
        let rows: Vec<&(
            burndown::OpenFacts,
            burndown::OpenBucket,
            Vec<burndown::Reason>,
        )> = advisor_rows
            .iter()
            .filter(|(_, b, _)| *b == bucket)
            .collect();
        if rows.is_empty() {
            continue;
        }
        println!("\n{} {} — {}", "●".yellow(), key.bold(), action.dimmed());
        for (f, _, _) in rows {
            println!("    {}{}", f.id.cyan(), spec_title_cell(&titles, &f.id));
        }
    }

    // DISTILL / TRIAGE / BLESS — pointer + live count, each with its verb (the
    // accepted shape for "go act over there" buckets; full lists live in their
    // own commands). trace:STORY-618
    if !distill.is_empty() {
        println!(
            "\n{} {} — {}",
            "●".yellow(),
            "distill".bold(),
            // BUG-596: the hint must print a RUNNABLE command — `aida clarify`
            // does not exist (errors as an unrecognized subcommand). The real
            // verbs are `aida questions clarify` (interactive, defaults to the
            // swept set) and `aida decide <SPEC>` (smart-routes per spec).
            // trace:BUG-596 | ai:claude
            format!(
                "{} spec{} under-specified — author acceptance: `aida questions clarify` (or `aida decide <SPEC>`)",
                distill.len(),
                if distill.len() == 1 { "" } else { "s" }
            )
            .dimmed()
        );
    }
    if triage > 0 {
        println!(
            "\n{} {} — {}",
            "●".magenta(),
            "triage".bold(),
            format!(
                "{} finding{} awaiting triage — `aida findings list`",
                triage,
                if triage == 1 { "" } else { "s" }
            )
            .dimmed()
        );
    }
    if !bless.is_empty() {
        // STORY-622: LIST the bless candidates (not just a count) — these are the
        // approved-but-unqueued specs the advisor drives next, the actionable
        // backlog. A bare count hid the work and made the worklist read "empty"
        // next to `aida list open`. trace:STORY-622 | ai:claude
        println!(
            "\n{} {} — {}",
            "●".green(),
            "bless".bold(),
            "approved but not queued — `aida backlog groom --pickable` to queue (advisor sign-off)"
                .dimmed()
        );
        for id in &bless {
            println!("    {}{}", id.cyan(), spec_title_cell(&titles, id));
        }
    }

    println!(
        "\n  {} operator decisions / reviews live on `aida human` · full dashboard: `aida advisor status`",
        crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
    );
    Ok(())
}

/// STORY-563: build [`burndown::UnblockFacts`] for every open spec — the input
/// to the `aida human unblock` classifier. Mirrors [`collect_open_facts`] but
/// adds the three signals the unblock lens needs that the explain lens doesn't:
/// queue membership (advisor sign-off, ADR-3), acceptance-criteria presence,
/// and implementable-type. Read-only. trace:STORY-563 | ai:claude
pub(crate) fn collect_unblock_facts(
    store: &aida_core::RequirementsStore,
    in_flight_scopes: &std::collections::HashSet<String>,
    queued_ids: &std::collections::HashSet<uuid::Uuid>,
) -> Vec<burndown::UnblockFacts> {
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let mut facts = Vec::new();
    for req in &store.requirements {
        if req.archived {
            continue;
        }
        // "Open" = not yet terminal. Completed/Rejected are done with.
        if matches!(
            req.status,
            aida_core::RequirementStatus::Completed | aida_core::RequirementStatus::Rejected
        ) {
            continue;
        }
        let id = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| req.id.to_string());
        let in_flight = !in_flight_scopes.is_empty()
            && [req.agreed_id.as_deref(), req.spec_id.as_deref()]
                .into_iter()
                .flatten()
                .any(|s| in_flight_scopes.contains(&s.to_ascii_lowercase()));
        // Acceptance presence — same text scan the questions-sweep uses (BUG-495).
        let text = requirement_text(req).to_ascii_lowercase();
        let has_acceptance =
            contains_any(&text, &["acceptance", "acceptance criteria", "acceptance:"]);
        facts.push(burndown::UnblockFacts {
            id,
            title: req.title.clone(),
            req_type: format!("{:?}", req.req_type).to_ascii_lowercase(),
            status: norm(&format!("{:?}", req.status)),
            tags: req.tags.iter().cloned().collect(),
            has_unsatisfied_blocker: aida_core::pickability::blocked_by_incomplete(req, store),
            has_pending_decision: req
                .decision_request
                .as_ref()
                .map(|d| d.is_pending())
                .unwrap_or(false),
            in_flight,
            queued: queued_ids.contains(&req.id),
            has_acceptance,
            implementable: is_implementable_type(&req.req_type),
        });
    }
    facts
}

/// STORY-563: `aida human unblock` — the deterministic prompt-assembler that
/// ends the recurring "how do I get open items into the burndown?" question by
/// GENERATING the grooming question. Read-only + no LLM (the SPIKE-55
/// deterministic-slice pattern, like `aida ultraplan` / `aida goal`): classify
/// every open spec by what keeps it out of the burndown ready set, then assemble
/// a paste-ready advisor prompt that routes each to queue / clarify-first /
/// leave-parked. The advisor (the grooming skill / live session) is the actor
/// the prompt drives. trace:STORY-563 | ai:claude
pub(crate) fn handle_human_subcommand(cmd: &cli::HumanCommand) -> Result<()> {
    match cmd {
        // TASK-770: namespaced aliases over the existing top-level presence
        // verbs; no duplicate state path or output shape.
        cli::HumanCommand::Away => presence_cmd::handle_away_command(),
        cli::HumanCommand::Home => presence_cmd::handle_home_command(),
        cli::HumanCommand::Presence | cli::HumanCommand::Status => {
            presence_cmd::handle_presence_command()
        }
        cli::HumanCommand::Unblock {
            copy,
            stdout,
            json,
            interactive,
            then_drain,
        } => handle_human_unblock(*copy, *stdout, *json, *interactive, *then_drain),
        // STORY-768: fire the /aida-human-audit pass at the advisor — enqueue a
        // durable directive (default) and optionally tmux-inject it now.
        // trace:STORY-768 | ai:claude
        cli::HumanCommand::Audit { inject } => {
            let project_root =
                find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
            handle_human_audit(&project_root, *inject)
        }
        // STORY-611: the action aliases need a storage handle, so they are
        // dispatched in the main `run()` body (where `backend`/`store_path`
        // are in scope), not here. This arm is unreachable from the early
        // pre-storage dispatch, which gates on `human_subcommand_needs_no_storage`.
        // trace:STORY-611 | ai:claude
        cli::HumanCommand::Answer { .. }
        | cli::HumanCommand::Decide { .. }
        | cli::HumanCommand::Review { .. } => {
            unreachable!("STORY-611 action aliases are dispatched after store init")
        }
    }
}

/// STORY-768: `aida human audit` — fire the `/aida-human-audit` reconcile pass
/// at the advisor session. Default is a durable enqueue onto the worker-
/// directive channel that the polling advisor picks up; `--inject` additionally
/// `tmux send-keys` the slash command into the advisor's registered pane for an
/// immediate run, falling back to enqueue-only when no pane is registered.
// trace:STORY-768 | ai:claude
pub(crate) fn handle_human_audit(project_root: &std::path::Path, inject: bool) -> Result<()> {
    // Always enqueue the durable directive first — it is the reliable,
    // headless/cross-vendor path. The optional inject is a best-effort nudge on
    // top of it, so an idle advisor runs the pass without waiting a poll cycle.
    human_audit::post_directive_line_enqueue(project_root)?;
    println!(
        "{} Enqueued a human-audit request for the advisor.",
        glyph(crate::glyphs::Glyph::Check).green()
    );
    println!(
        "  {}",
        "The polling advisor picks it up next cycle (aida worker directives) and runs the pass."
            .dimmed()
    );

    if !inject {
        return Ok(());
    }

    // --inject: try to send the slash command straight into the advisor's
    // registered tmux pane. Never hard-fail — fall back to the enqueue already
    // done above and explain why.
    match human_audit::plan_inject(human_audit::read_pane(project_root)) {
        human_audit::InjectPlan::Inject(argv) => {
            let (program, rest) = argv.split_first().expect("send-keys argv is non-empty");
            match std::process::Command::new(program)
                .args(rest)
                .status_retrying_etxtbsy()
            {
                Ok(status) if status.success() => {
                    println!(
                        "{} Injected the audit command into the advisor's tmux pane.",
                        glyph(crate::glyphs::Glyph::Check).green()
                    );
                }
                Ok(status) => {
                    println!(
                        "{} tmux send-keys exited with {} — the enqueued request still stands.",
                        glyph(crate::glyphs::Glyph::Warning).yellow(),
                        status
                    );
                }
                Err(e) => {
                    println!(
                        "{} Could not run tmux ({}) — the enqueued request still stands.",
                        glyph(crate::glyphs::Glyph::Warning).yellow(),
                        e
                    );
                }
            }
        }
        human_audit::InjectPlan::Fallback => {
            println!(
                "{} No advisor tmux pane is registered (not in tmux, or the advisor session did \
                 not start under tmux) — kept the enqueued request only.",
                glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
    }
    Ok(())
}

/// STORY-611: which `aida human <sub>` verbs run WITHOUT a storage handle. The
/// presence + unblock verbs do; the action aliases (`answer`/`review`/`decide`)
/// delegate to backend-needing canonical verbs, so they fall through to the
/// main dispatch instead of the early pre-storage return.
/// trace:STORY-611 | ai:claude
pub(crate) fn human_subcommand_needs_no_storage(cmd: &cli::HumanCommand) -> bool {
    matches!(
        cmd,
        cli::HumanCommand::Away
            | cli::HumanCommand::Home
            | cli::HumanCommand::Presence
            | cli::HumanCommand::Status
            | cli::HumanCommand::Unblock { .. }
            | cli::HumanCommand::Audit { .. }
    )
}

// trace:STORY-611 | ai:claude
#[cfg(test)]
#[path = "tests/story_611_human_alias_tests.rs"]
mod story_611_human_alias_tests;

/// STORY-563: the `aida human unblock` body. trace:STORY-563 | ai:claude
pub(crate) fn handle_human_unblock(
    copy: bool,
    stdout: bool,
    json: bool,
    interactive: bool,
    then_drain: bool,
) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;
    let in_flight_scopes = in_flight_lease_scopes(&project_root);
    let queued_ids = all_queued_requirement_ids(&project_root);
    let facts = collect_unblock_facts(&store, &in_flight_scopes, &queued_ids);

    // Classify each open spec, dropping the ones already in the burndown ready
    // set (nothing for the human to do). Preserve store order. trace:STORY-563
    let lines: Vec<burndown::UnblockLine> = facts
        .iter()
        .filter_map(|f| {
            burndown::classify_unblock(f).map(|class| burndown::UnblockLine {
                id: f.id.clone(),
                title: f.title.clone(),
                class,
                reason: burndown::unblock_reason(class).to_string(),
            })
        })
        .collect();

    // STORY-750: --interactive walks the human-blocked set and resolves each
    // hurdle inline (self-invoking the existing `aida` verbs) instead of emitting
    // the paste-ready advisor prompt. trace:STORY-750 | ai:claude
    if interactive {
        return run_interactive_unblock_sweep(lines, then_drain);
    }

    // --json: the classification, for machine consumers / the TUI.
    if json {
        let payload = lines
            .iter()
            .map(|l| {
                let action = match l.class.action() {
                    burndown::UnblockAction::Queue => "queue",
                    burndown::UnblockAction::Clarify => "clarify",
                    // trace:BUG-502
                    burndown::UnblockAction::Review => "review",
                    burndown::UnblockAction::Leave => "leave",
                };
                serde_json::json!({
                    "spec": l.id,
                    "class": l.class.key(),
                    "action": action,
                    "reason": l.reason,
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let prompt = burndown::assemble_unblock_prompt(&lines);

    // --stdout: the bare prompt only, for piping / command substitution.
    if stdout {
        print!("{prompt}");
        return Ok(());
    }

    // --copy: prompt to the clipboard, falling back to stdout.
    if copy {
        if copy_to_clipboard(&prompt) {
            println!(
                "{} copied the grooming prompt to the clipboard — paste it to the advisor",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        } else {
            println!(
                "{} no clipboard tool found (wl-copy/xclip/xsel/pbcopy/clip) — printing instead",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
            println!();
            print!("{prompt}");
        }
        return Ok(());
    }

    // Default: framed terminal output. Headline the count + the three actions,
    // then the assembled prompt set apart so the operator knows exactly what to
    // paste. trace:STORY-563
    let counts = |want: burndown::UnblockAction| -> usize {
        lines.iter().filter(|l| l.class.action() == want).count()
    };
    let n_queue = counts(burndown::UnblockAction::Queue);
    let n_clarify = counts(burndown::UnblockAction::Clarify);
    let n_leave = counts(burndown::UnblockAction::Leave);

    println!(
        "{} grooming prompt for the advisor",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    if lines.is_empty() {
        println!(
            "  {}",
            "Nothing to groom — every open spec is either already in the burndown \
             or self-resolving."
                .dimmed()
        );
        return Ok(());
    }
    println!(
        "  {} {} to queue · {} to clarify first · {} to leave parked",
        "→".green(),
        n_queue,
        n_clarify,
        n_leave
    );
    println!(
        "  {}",
        "Paste the block below to the advisor (or --copy it / --stdout to pipe):".dimmed()
    );
    println!();
    for line in prompt.lines() {
        println!("  {}", line.dimmed());
    }
    Ok(())
}

/// STORY-750: the `aida human unblock --interactive` sweep — walk the
/// human-blocked specs CHEAPEST-first and resolve each inline. Impure (TTY
/// prompts + self-invoked `aida` verbs); the menus + walk order are the pure,
/// unit-tested `burndown::sweep_*` helpers.
pub(crate) fn run_interactive_unblock_sweep(
    lines: Vec<burndown::UnblockLine>,
    then_drain: bool,
) -> Result<()> {
    use burndown::SweepChoice;

    // Needs a human at the keyboard: `inquire` reads stdin. In a non-TTY / agent
    // context, degrade with clear guidance rather than hang or silently no-op.
    if non_interactive_confirm() {
        anyhow::bail!(
            "`aida human unblock --interactive` needs a terminal. Run `aida human unblock` \
             (no flag) for the paste-ready advisor prompt, or `--json` for the machine view."
        );
    }

    if lines.is_empty() {
        println!(
            "{} nothing blocked on you — every open spec is drive-ready or self-resolving.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }

    let ordered = burndown::sweep_walk_order(lines);
    // Partition into: advisor-groomable (bulk-delegate, not the human's call),
    // genuinely-human decisions (walk), and leave-parked info rows. trace:STORY-750
    let (groomable, rest): (Vec<_>, Vec<_>) = ordered
        .into_iter()
        .partition(|l| burndown::sweep_is_advisor_groomable(l.class));
    let (mut human, info): (Vec<_>, Vec<_>) = rest
        .into_iter()
        .partition(|l| burndown::sweep_is_actionable(l.class));

    // Everything the operator hands off (the bulk groomable set + per-spec
    // "Ask the advisor" picks) → one advisor-delegate prompt at the end.
    let mut delegated: Vec<burndown::UnblockLine> = Vec::new();

    // --- Bulk advisor-grooming step: don't dump 100 draft-approvals on the human.
    // "Hand them to the advisor" used to be one vague option that merely printed a
    // paste-prompt. TASK-1087: make it legible — offer the two concrete groom
    // actions as top-level picks (propose vs apply), keep the paste-prompt as an
    // explicit option, and add a re-prompting "?" item that explains each with
    // simple examples (inquire 0.9.1 has no native '?' keybinding).
    // trace:STORY-750 trace:TASK-1087 | ai:claude
    if !groomable.is_empty() {
        const GROOM_PROPOSE: &str =
            "Groom them — the advisor proposes approve/queue/reject; you review";
        const GROOM_APPLY: &str = "Groom + approve — the advisor decides AND applies now";
        const PASTE: &str = "Emit a paste-prompt for a live advisor session";
        const WALK: &str = "Walk them myself anyway";
        const SKIP: &str = "Skip them";
        const HELP: &str = "? — what do these mean? (examples)";

        let heading = format!(
            "{} spec(s) are advisor grooming (drafts to approve / approved to queue) — the advisor's call, not yours",
            groomable.len()
        );

        // A tiny action enum so the re-prompting menu (the '?' item loops back to
        // the same prompt) is decoupled from the code that consumes `groomable`.
        enum GroomAction {
            Propose,
            Apply,
            Paste,
            Walk,
            Skip,
        }

        let action = loop {
            let opts = vec![GROOM_PROPOSE, GROOM_APPLY, PASTE, WALK, SKIP, HELP];
            let pick = match inquire::Select::new(&heading, opts)
                .with_help_message("Up/Down to move, Enter to select, or pick '?' for examples")
                .prompt()
            {
                Ok(p) => p,
                Err(inquire::InquireError::OperationCanceled)
                | Err(inquire::InquireError::OperationInterrupted) => {
                    println!(
                        "  {} sweep stopped.",
                        crate::glyph(crate::glyphs::Glyph::Cross).yellow()
                    );
                    return Ok(());
                }
                Err(e) => anyhow::bail!("prompt failed: {e}"),
            };
            match pick {
                GROOM_PROPOSE => break GroomAction::Propose,
                GROOM_APPLY => break GroomAction::Apply,
                PASTE => break GroomAction::Paste,
                WALK => break GroomAction::Walk,
                SKIP => break GroomAction::Skip,
                HELP => {
                    print_groom_handoff_help();
                    continue;
                }
                _ => continue,
            }
        };

        match action {
            // "Groom them" / "Groom + approve" fire the advisor's own disposition
            // pass via the SAME `aida groom` verb the advisor runs by hand — no
            // re-implementation. Propose writes nothing; apply executes.
            GroomAction::Propose => {
                println!(
                    "  {} firing the advisor's groom pass (propose-only — writes nothing until you apply)…",
                    crate::glyph(crate::glyphs::Glyph::Robot).cyan()
                );
                self_invoke_aida(&["groom"])?;
            }
            GroomAction::Apply => {
                println!(
                    "  {} firing the advisor's groom pass with --apply — it will approve/queue/reject…",
                    crate::glyph(crate::glyphs::Glyph::Robot).cyan()
                );
                self_invoke_aida(&["groom", "--apply"])?;
            }
            // Relay to a warm advisor session as a paste-prompt (the old behavior).
            GroomAction::Paste => delegated.extend(groomable),
            GroomAction::Walk => {
                let mut merged = groomable;
                merged.extend(human);
                human = merged;
            }
            GroomAction::Skip => {}
        }
    }

    // --- Interactive human walk (cheapest-first; title shown so you can decide).
    println!();
    if human.is_empty() {
        println!(
            "{} no genuinely-human decisions to walk.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    } else {
        println!(
            "{} {} spec(s) need your judgment — walking cheapest-first. Ctrl-C to stop.",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
            human.len()
        );
        println!();
    }

    let (mut resolved, mut skipped) = (0usize, 0usize);
    'walk: for line in &human {
        // Per-spec loop so "Show details" re-prompts the same spec.
        loop {
            let menu = burndown::sweep_menu(line.class);
            let labels: Vec<&str> = menu.iter().map(|c| c.label()).collect();
            let heading = format!("{} — {} — {}", line.id, line.title, line.reason);
            let picked = match inquire::Select::new(&heading, labels).prompt() {
                Ok(label) => menu
                    .iter()
                    .copied()
                    .find(|c| c.label() == label)
                    .unwrap_or(SweepChoice::Skip),
                Err(inquire::InquireError::OperationCanceled)
                | Err(inquire::InquireError::OperationInterrupted) => {
                    println!(
                        "  {} sweep stopped.",
                        crate::glyph(crate::glyphs::Glyph::Cross).yellow()
                    );
                    break 'walk;
                }
                Err(e) => anyhow::bail!("prompt failed: {e}"),
            };
            match picked {
                // Show the full spec, then re-prompt the SAME one.
                SweepChoice::ShowDetails => {
                    let _ = self_invoke_aida(&["show", &line.id, "--no-git"]);
                    continue;
                }
                // Hand this one to the advisor instead of deciding it.
                SweepChoice::AskAdvisor => {
                    delegated.push(line.clone());
                    println!(
                        "    {} {} handed to the advisor.",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        line.id
                    );
                    skipped += 1;
                    break;
                }
                other => {
                    if apply_sweep_choice(&line.id, other)? {
                        resolved += 1;
                    } else {
                        skipped += 1;
                    }
                    break;
                }
            }
        }
    }

    // --- Leave-parked info rows: list, no prompt.
    if !info.is_empty() {
        println!();
        println!(
            "{} left parked (nothing to resolve inline):",
            crate::glyph(crate::glyphs::Glyph::Bullet).dimmed()
        );
        for line in &info {
            println!(
                "  {} {} — {}",
                crate::glyph(crate::glyphs::Glyph::Bullet).dimmed(),
                line.id.dimmed(),
                line.reason.dimmed()
            );
        }
    }

    println!();
    println!(
        "{} sweep done — {} resolved, {} skipped, {} handed to the advisor.",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        resolved,
        skipped,
        delegated.len()
    );

    // --- Emit ONE advisor-delegate prompt for everything handed off.
    if !delegated.is_empty() {
        let prompt = burndown::assemble_unblock_prompt(&delegated);
        println!();
        println!(
            "{} paste this to the advisor to groom the {} handed-off spec(s) (or `aida human unblock --copy`):",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
            delegated.len()
        );
        println!();
        for l in prompt.lines() {
            println!("  {}", l.dimmed());
        }
    }

    if then_drain {
        report_drain_readiness();
    }
    Ok(())
}

/// TASK-1087: the '?' item on the bulk advisor-grooming menu — explain each
/// choice in plain terms with concrete examples, then the caller re-prompts.
// trace:TASK-1087 | ai:claude
pub(crate) fn print_groom_handoff_help() {
    let arrow = crate::glyph(crate::glyphs::Glyph::Arrow);
    println!();
    println!("  {}", "What each choice does".bold());
    println!();
    println!("  {}", "Groom them (propose)".cyan());
    println!("    Fires the advisor — a fresh `claude -p` that reads the open drafts /");
    println!("    approved-unqueued specs and proposes a fate for each. Writes NOTHING");
    println!("    until you apply. For example:");
    println!("      DRAFT-812  {arrow} approve + queue   (clear, in scope)");
    println!("      DRAFT-820  {arrow} reject            (duplicate of an existing story)");
    println!("      SPIKE-9    {arrow} park              (needs a demand signal first)");
    println!("    You review, then run `aida groom --apply` to execute.");
    println!();
    println!("  {}", "Groom + approve (apply)".cyan());
    println!("    The same advisor pass, but it EXECUTES its calls immediately —");
    println!("    approvals get queued, rejects rejected, parks parked. For example:");
    println!("      \"11 approved & queued, 3 rejected, 2 parked\" — nothing left for you.");
    println!("    Use when you trust the advisor on these low-stakes grooming calls.");
    println!();
    println!("  {}", "Emit a paste-prompt for a live advisor".cyan());
    println!("    Changes nothing. Prints a ready-made prompt (grouped by action) for");
    println!("    you to paste into your WARM advisor session — richer context than a");
    println!("    cold-boot agent.");
    println!();
    println!("  {}", "Walk them myself".cyan());
    println!("    Adds them back to your one-by-one review; you decide each.");
    println!();
    println!("  {}", "Skip them".cyan());
    println!("    Leave them parked and untouched this pass.");
    println!();
    println!(
        "  {}",
        "Note: groom considers the advisor's full open-spec candidate set (these included), not only the rows shown here."
            .dimmed()
    );
    println!();
}

/// The parking tag the interactive sweep clears when the operator captures a
/// design decision on a spec — see [`tags_to_clear_on_note`].
// trace:TASK-1086 | ai:claude
pub(crate) const NEEDS_DESIGN_TAG: &str = "needs-design";

/// TASK-1086: the pure core of the sweep's "add a decision note" action — decide
/// which tags to clear given the operator's typed note and the spec's current
/// tags. Capturing a real decision on a `needs-design`-parked spec unparks it, so
/// `needs-design` (case-insensitive) is cleared and the spec becomes drive-ready
/// in the same action; a blank/cancelled note changes nothing. Every other tag is
/// preserved, and a spec that doesn't carry `needs-design` yields an empty list
/// (idempotent no-op). Pure, so exhaustively unit-testable; the runtime reuses the
/// `aida edit --remove-tag` write path for the actual clear.
// trace:TASK-1086 | ai:claude
pub(crate) fn tags_to_clear_on_note(note: &str, tags: &HashSet<String>) -> Vec<String> {
    if note.trim().is_empty() {
        return Vec::new();
    }
    tags.iter()
        .filter(|t| t.trim().eq_ignore_ascii_case(NEEDS_DESIGN_TAG))
        .cloned()
        .collect()
}

/// TASK-1086: best-effort read of a spec's current tag set for the interactive
/// sweep. Returns an empty set if the store can't be reached — the caller then
/// clears nothing, which is the safe (leave-parked) default.
// trace:TASK-1086 | ai:claude
pub(crate) fn load_spec_tags(id: &str) -> HashSet<String> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let Some(store) = load_store_for_lookup(&project_root) else {
        return HashSet::new();
    };
    store
        .get_requirement_by_spec_id(id.trim())
        .map(|r| r.tags.clone())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "tests/task_1086_unblock_clears_needs_design.rs"]
mod task_1086_unblock_clears_needs_design;

/// STORY-750: perform ONE sweep resolution by self-invoking the existing `aida`
/// verb — so the write path is the SAME one `aida edit` / `aida queue add` use,
/// never a re-implementation. Returns `true` if it mutated the spec toward
/// drive-ready, `false` if it only surfaced guidance / the operator skipped.
// trace:STORY-750 | ai:claude
pub(crate) fn apply_sweep_choice(id: &str, choice: burndown::SweepChoice) -> Result<bool> {
    use burndown::SweepChoice;
    match choice {
        SweepChoice::ApproveQueue => {
            self_invoke_aida(&["edit", id, "--status", "approved"])?;
            self_invoke_aida(&["queue", "add", id])?;
            println!(
                "    {} {id} approved & queued.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
            Ok(true)
        }
        SweepChoice::Reject => {
            self_invoke_aida(&["edit", id, "--status", "rejected"])?;
            println!(
                "    {} {id} rejected.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
            Ok(true)
        }
        SweepChoice::Queue => {
            self_invoke_aida(&["queue", "add", id])?;
            println!(
                "    {} {id} queued.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
            Ok(true)
        }
        SweepChoice::LaunchGuided => {
            // The operator chose to resolve the design NOW — hand off to the real
            // guided resolver (its own focused session), then return to the sweep.
            self_invoke_aida(&["queue", "work", id, "--guided"])?;
            Ok(true)
        }
        SweepChoice::AddNote => {
            let note = inquire::Text::new("Decision note:")
                .prompt()
                .unwrap_or_default();
            if note.trim().is_empty() {
                println!(
                    "    {} no note entered — left parked.",
                    crate::glyph(crate::glyphs::Glyph::Bullet).dimmed()
                );
                return Ok(false);
            }
            self_invoke_aida(&["comment", "add", id, note.trim()])?;
            // TASK-1086: capturing a design decision unparks the spec — clear the
            // `needs-design` tag so it becomes drive-ready in the SAME action,
            // instead of leaving the operator to strip the tag by hand. Reuses the
            // `aida edit --remove-tag` write path; idempotent (a spec without the
            // tag clears nothing). Only fires on a real capture — a blank note
            // returned above with the tag intact.
            let to_clear = tags_to_clear_on_note(&note, &load_spec_tags(id));
            for tag in &to_clear {
                self_invoke_aida(&["edit", id, "--remove-tag", tag])?;
            }
            if to_clear.is_empty() {
                println!(
                    "    {} note recorded on {id} (left parked).",
                    crate::glyph(crate::glyphs::Glyph::Check).green()
                );
                Ok(false)
            } else {
                println!(
                    "    {} decision recorded on {id} — cleared `{NEEDS_DESIGN_TAG}`, now drive-ready.",
                    crate::glyph(crate::glyphs::Glyph::Check).green()
                );
                Ok(true)
            }
        }
        SweepChoice::Clarify => {
            println!(
                "    {} clarify its acceptance, then it becomes queueable:",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan()
            );
            println!("      aida edit {id}   (add a `## Acceptance` section)");
            Ok(false)
        }
        SweepChoice::ShowReview => {
            println!(
                "    {} it's built — review it (don't re-queue): aida review {id}  (or reopen its draft PR)",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan()
            );
            Ok(false)
        }
        SweepChoice::ShowBlocker => {
            // `aida why` surfaces the dependency chain in detail; best-effort.
            let _ = self_invoke_aida(&["why", id]);
            Ok(false)
        }
        // Handled by the walk loop before reaching here (they re-prompt / delegate),
        // but the match must be exhaustive.
        SweepChoice::ShowDetails | SweepChoice::AskAdvisor | SweepChoice::Skip => Ok(false),
    }
}
