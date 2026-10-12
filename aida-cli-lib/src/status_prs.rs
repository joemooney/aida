//! status in-flight buckets, open-PR and PR head-state snapshots, unshipped-work detection (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// TASK-234: the three in-flight buckets, split out of
/// [`render_in_flight_grouped`] so the classification — which only ever
/// touches git (`log` / `merge-base` / `branch --contains`), never gh —
/// is unit-testable against a temp repo. trace:TASK-234 | ai:claude
pub(crate) struct InFlightBuckets {
    /// (spec_id, squash-merge PR number if any, relative time) — the
    /// referencing commit is already on main but the spec never bumped
    /// to Completed (auto-bump missed).
    pub(crate) stuck: Vec<(String, Option<u64>, String)>,
    /// branch → spec_ids — the commit is on a feature branch, awaiting
    /// merge.
    pub(crate) awaiting: std::collections::BTreeMap<String, Vec<String>>,
    /// Done specs with no commit referencing the id yet.
    pub(crate) no_commit: Vec<String>,
    /// Deliberately reopened after the merged evidence; replay is not recovery.
    // trace:TASK-1338 | ai:codex
    pub(crate) reopened: Vec<String>,
}

/// TASK-234: bucket Done specs by git state. gh-free by design — the
/// PR-state lookup happens later, in [`render_in_flight_grouped`]. A
/// referencing commit on main → `stuck`; on a feature branch →
/// `awaiting`; no commit at all → `no_commit`. trace:TASK-234 | ai:claude
pub(crate) fn classify_in_flight_specs(
    specs: &[&aida_core::Requirement],
    project_root: &std::path::Path,
) -> InFlightBuckets {
    use std::collections::BTreeMap;
    use std::process::Command as PCmd;

    let git = |args: &[&str]| -> Option<String> {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
    };
    // BUG-380: capture stderr via output() so an invalid `target` ref
    // cannot leak `fatal: Not a valid object name <X>` to the user's
    // terminal — paired with `resolve_default_branch_ref`, which
    // returns a refname known to exist (or None to skip the check).
    let is_ancestor = |sha: &str, target: &str| -> bool {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["merge-base", "--is-ancestor", sha, target])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    let main_ref = resolve_default_branch_ref(project_root);

    let mut stuck: Vec<(String, Option<u64>, String)> = Vec::new();
    let mut awaiting: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut no_commit: Vec<String> = Vec::new();
    let mut reopened = Vec::new();

    for req in specs {
        let id = req
            .agreed_id
            .as_deref()
            .or(req.spec_id.as_deref())
            .unwrap_or("???")
            .to_string();
        // Newest commit referencing the AIDA `(SPEC-ID)` format.
        let pattern = format!("({})", id);
        let line = git(&[
            "log",
            "--all",
            "-F",
            "-1",
            "--grep",
            pattern.as_str(),
            "--pretty=format:%H\u{1f}%s\u{1f}%cr",
        ])
        .unwrap_or_default();
        let mut parts = line.splitn(3, '\u{1f}');
        let (Some(full), subject, ago) = (
            parts.next().filter(|s| !s.is_empty()),
            parts.next().unwrap_or(""),
            parts.next().unwrap_or("recently"),
        ) else {
            no_commit.push(id);
            continue;
        };
        let on_default = main_ref
            .as_deref()
            .map(|m| is_ancestor(full, m))
            .unwrap_or(false);
        if on_default && auto_bump_evidence_is_stale(project_root, req, full) {
            // trace:TASK-1338 | ai:codex
            reopened.push(id);
        } else if on_default {
            stuck.push((id, parse_squash_pr_number(subject), ago.to_string()));
        } else {
            let contains = git(&[
                "branch",
                "--all",
                "--contains",
                full,
                "--format=%(refname:short)",
            ])
            .unwrap_or_default();
            let candidates: Vec<String> = contains
                .lines()
                .map(|b| b.trim().trim_start_matches("origin/"))
                .filter(|b| !b.is_empty() && *b != "HEAD" && *b != "main" && *b != "master")
                .map(|b| b.to_string())
                .collect();
            // BUG-229: `pr-N` (and `mr-N` / `github-N` / `gitlab-N`) are
            // reviewer-worktree *snapshot* branches — `aida` fetches them
            // via `git fetch origin pull/N/head:pr-N` for headless review.
            // The spec's actual PR-source branch is named after the spec
            // (e.g. `task-282`). When the referencing commit lands on both,
            // a plain alphabetical `.next()` picks `pr-N` (it sorts first),
            // and the later `gh pr list --head pr-N` finds nothing — the
            // row then mis-renders as "no PR opened yet" for a PR that is
            // open and under review. Prefer a non-review-session branch so
            // the PR lookup keys off the real head. trace:BUG-229 | ai:claude
            let branch = candidates
                .iter()
                .find(|b| !is_review_session_branch(b))
                .or_else(|| candidates.first())
                .cloned()
                .unwrap_or_else(|| "(branch unknown)".to_string());
            awaiting.entry(branch).or_default().push(id);
        }
    }

    InFlightBuckets {
        stuck,
        awaiting,
        no_commit,
        reopened,
    }
}

/// TASK-250: a PR's review sub-state, derived from AIDA-local signals
/// only — session leases and the `Review PR-N:` story status. The gh
/// `reviewDecision` probe that promotes a row to State 3 (Approved)
/// layers on top in the render path. trace:TASK-250 | ai:claude
#[derive(Debug, PartialEq)]
pub(crate) enum LocalReviewState {
    /// State 1: no reviewer lease, no In-Progress review story.
    NotStarted,
    /// State 2: a reviewer session lease owns this PR — carries the
    /// short session id + a humanized start time for the hint.
    InProgressLease { session_id: String, since: String },
    /// State 2 (no scoped session): the `Review PR-N:` story is
    /// In Progress — a reviewer picked the queue item up.
    InProgressStory { story_id: String },
}

/// TASK-250: classify a PR's review state from session leases + the
/// `Review PR-N:` story. gh-free by design — it only reads lease TOML
/// and the in-memory requirement set, so it is unit-testable against a
/// temp project root. The render path adds the gh `reviewDecision`
/// probe (State 3) on top. trace:TASK-250 | ai:claude
pub(crate) fn detect_local_review_state(
    project_root: &std::path::Path,
    all_reqs: &[aida_core::Requirement],
    pr_number: u64,
) -> LocalReviewState {
    // Leg 1 — an active session lease scoped to this PR. The strongest
    // signal: a reviewer shell is open on it right now. `list_leases`
    // returns oldest-first, so the last match is the most recent.
    let mut lease_hit: Option<(String, String)> = None;
    for lease in list_leases(project_root) {
        if let Some((ReviewForge::GitHub, n)) = parse_review_scope(&lease.scope) {
            if n == pr_number {
                lease_hit = Some((lease.id.clone(), humanize_relative(lease.started_at)));
            }
        }
    }
    if let Some((session_id, since)) = lease_hit {
        return LocalReviewState::InProgressLease { session_id, since };
    }
    // Leg 2 — an In-Progress `Review PR-N:` story: the reviewer picked
    // the queue item up but isn't in a PR-scoped session.
    for r in all_reqs {
        if r.status == RequirementStatus::InProgress
            && parse_review_story_pr_number(&r.title) == Some(pr_number)
        {
            let story_id = r
                .agreed_id
                .as_deref()
                .or(r.spec_id.as_deref())
                .unwrap_or("")
                .to_string();
            return LocalReviewState::InProgressStory { story_id };
        }
    }
    LocalReviewState::NotStarted
}

/// TASK-250: gh's `reviewDecision` for a PR plus the approving
/// reviewer's login — the State 3 ("reviewed + approved") signal.
/// trace:TASK-250 | ai:claude
pub(crate) struct PrReviewDecision {
    /// gh `reviewDecision`: "APPROVED" / "CHANGES_REQUESTED" /
    /// "REVIEW_REQUIRED" / "" (no reviews).
    pub(crate) decision: String,
    /// login of the most recent APPROVED review, when resolvable.
    pub(crate) approver: Option<String>,
}

/// TASK-250: parse `gh pr view --json reviewDecision,latestReviews`
/// output. Pure — unit-tested against captured gh JSON without
/// spawning gh. trace:TASK-250 | ai:claude
pub(crate) fn parse_review_decision_json(stdout: &str) -> Option<PrReviewDecision> {
    let json: serde_json::Value = serde_json::from_str(stdout).ok()?;
    let decision = json
        .get("reviewDecision")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let approver = json
        .get("latestReviews")
        .and_then(|v| v.as_array())
        .and_then(|reviews| {
            reviews
                .iter()
                .filter(|r| r.get("state").and_then(|s| s.as_str()) == Some("APPROVED"))
                .filter_map(|r| {
                    r.get("author")
                        .and_then(|a| a.get("login"))
                        .and_then(|l| l.as_str())
                })
                .next_back()
                .map(|s| s.to_string())
        });
    Some(PrReviewDecision { decision, approver })
}

/// TASK-250: probe gh for a PR's review decision. `None` when gh is
/// missing or the call fails — the caller degrades to the local review
/// state. trace:TASK-250 | ai:claude
pub(crate) fn detect_pr_review_decision(
    project_root: &std::path::Path,
    pr_number: u64,
) -> Option<PrReviewDecision> {
    // STORY-621 Slice 2: routed through Forge::change_reviews so the probe
    // works on GitLab (decision derived from the approvals endpoint). The
    // decision travels as the forge-neutral enum and is lowered back to gh's
    // token vocabulary here because this display path still compares strings.
    // trace:TASK-963 | ai:claude
    let reviews = crate::forge::forge_for(project_root)
        .change_reviews(pr_number, &mut network_retry::NoopSink)
        .ok()?;
    Some(PrReviewDecision {
        decision: reviews.decision.gh_token().to_string(),
        approver: reviews.approver,
    })
}

/// TASK-250: parse `gh pr list --state merged --json number,mergedAt`
/// output into (pr_number, humanized merge time). Pure — unit-tested
/// against captured gh JSON. trace:TASK-250 | ai:claude
pub(crate) fn parse_merged_pr_json(stdout: &str) -> Option<(u64, Option<String>)> {
    let json: serde_json::Value = serde_json::from_str(stdout).ok()?;
    let first = json.as_array()?.first()?;
    let number = first.get("number")?.as_u64()?;
    let merged = first
        .get("mergedAt")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| humanize_relative(dt.with_timezone(&chrono::Utc)));
    Some((number, merged))
}

/// TASK-250: State-4 probe — was `branch` the head of a now-MERGED PR,
/// and when did it merge? Distinct from `detect_merged_pr_for_branch`
/// because it also pulls `mergedAt` for the "Merged {time}" hint.
/// `None` when gh is missing / fails / reports no merged PR.
/// trace:TASK-250 | ai:claude
pub(crate) fn detect_merged_pr_with_time(
    project_root: &std::path::Path,
    branch: &str,
) -> Option<(u64, Option<String>)> {
    let gh_bin = resolve_gh_binary()?;
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "list",
            "--head",
            branch,
            "--state",
            "merged",
            "--limit",
            "1",
            "--json",
            "number,mergedAt",
        ])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_merged_pr_json(&String::from_utf8_lossy(&out.stdout))
}

/// TASK-250: unit coverage for the four-state classification behind
/// `aida queue list`'s "Done — awaiting merge" section. The lease /
/// review-story leg (`detect_local_review_state`) is exercised against
/// a temp project root; the gh-JSON parsers are exercised against
/// captured `gh` output. trace:TASK-250 | ai:claude
#[cfg(test)]
#[path = "tests/task_250_review_state_tests.rs"]
mod task_250_review_state_tests;

/// BUG-229: the in-flight display state for a branch with an open PR —
/// one variant per drain-pipeline stage. Derived purely from the three
/// signals the render path already gathers (`gh reviewDecision`, the
/// local review state, the CI probe) so the mapping is unit-testable
/// without spawning `gh`. trace:BUG-229 | ai:claude
#[derive(Debug, PartialEq)]
pub(crate) enum InFlightPrDisplay {
    /// gh `reviewDecision` == APPROVED — review done, awaiting merge.
    ReviewApproved { approver: Option<String> },
    /// A reviewer session lease owns the PR — review running now.
    UnderReviewLease { session_id: String, since: String },
    /// An In-Progress `Review PR-N:` story — a reviewer picked it up
    /// without a PR-scoped session.
    UnderReviewStory { story_id: String },
    /// PR open, no review started yet, CI still running — the
    /// orchestrator is in its CI-wait window; "start review" would be
    /// premature.
    CiRunning,
    /// PR open, no review started, CI not blocking — ready for a
    /// reviewer to pick up.
    AwaitingReview,
}

/// BUG-229: map the three already-probed signals onto an
/// [`InFlightPrDisplay`] stage. Pure — no `gh`, no `git` — so the full
/// drain-pipeline matrix is unit-testable from synthesized inputs.
///
/// Precedence mirrors the pipeline: an APPROVED `reviewDecision` is the
/// authoritative "safe to merge" signal and supersedes a lingering
/// reviewer lease; an active reviewer lease / story means review is
/// running; only when no review has started does the CI probe decide
/// between "CI still running" and "ready for review".
/// trace:BUG-229 | ai:claude
pub(crate) fn classify_open_pr_display(
    approved: Option<PrReviewDecision>,
    local_state: LocalReviewState,
    ci: &CiProbe,
) -> InFlightPrDisplay {
    if let Some(d) = approved {
        if d.decision == "APPROVED" {
            return InFlightPrDisplay::ReviewApproved {
                approver: d.approver,
            };
        }
    }
    match local_state {
        LocalReviewState::InProgressLease { session_id, since } => {
            InFlightPrDisplay::UnderReviewLease { session_id, since }
        }
        LocalReviewState::InProgressStory { story_id } => {
            InFlightPrDisplay::UnderReviewStory { story_id }
        }
        LocalReviewState::NotStarted => {
            if matches!(ci, CiProbe::InProgress { .. }) {
                InFlightPrDisplay::CiRunning
            } else {
                InFlightPrDisplay::AwaitingReview
            }
        }
    }
}

/// BUG-229: the open-PR display matrix — `classify_open_pr_display` maps
/// the three drain-pipeline signals onto one [`InFlightPrDisplay`] stage
/// per row of the spec's state machine. Synthesized inputs, no `gh` /
/// `git`. trace:BUG-229 | ai:claude
#[cfg(test)]
#[path = "tests/bug_229_display_matrix_tests.rs"]
mod bug_229_display_matrix_tests;

/// TASK-234: render the "Done — awaiting merge" body grouped by PR +
/// state instead of as a flat list. [`classify_in_flight_specs`] does
/// the git-only bucketing; this adds the `gh` PR-state lookups and
/// prints each bucket with a concrete `Next` action.
///
/// BUG-366: the "start review" hint must name a command that survives an
/// operator's reflexive flags. Bare `aida queue work PR-N` resolves to the
/// review story (TASK-85), but operators habitually append implementer-drain
/// flags (`--auto-complete --no-human`) — and the auto-complete preflight
/// can't resolve a PR number into a spec. Naming `--for reviewer` makes the
/// reviewer intent explicit so the implementer-drain flags don't get bolted
/// on to a review pickup. trace:BUG-366 | ai:claude
pub(crate) fn review_pickup_hint(pr_number: u64) -> String {
    format!("aida queue work PR-{pr_number} --for reviewer")
}

/// TASK-250: each open-PR branch is sub-classified into one of four
/// PR-lifecycle states — review not started / in progress / approved /
/// merged-awaiting-pull — so the next-action hint matches reality
/// instead of a blanket "merge PR-N". trace:TASK-234 TASK-250 | ai:claude
pub(crate) fn render_in_flight_grouped(
    specs: &[&aida_core::Requirement],
    all_reqs: &[aida_core::Requirement],
    project_root: &std::path::Path,
) {
    let InFlightBuckets {
        stuck,
        awaiting,
        no_commit,
        reopened,
    } = classify_in_flight_specs(specs, project_root);

    // trace:TASK-1338 | ai:codex
    if !reopened.is_empty() {
        println!("  Reopened after merged work: {}", reopened.join(", "));
        println!("    Next: finish the reopened work; old merge evidence will not close it.");
    }

    // ---- Awaiting merge, grouped by branch ----
    // TASK-250: an open-PR branch is sub-classified into State 1/2/3
    // (review not started / in progress / approved); a branch with no
    // open PR is checked for a merged PR (State 4) before falling back
    // to the "no PR opened yet" hint. trace:TASK-250 | ai:claude
    for (branch, ids) in &awaiting {
        let count = ids.len();
        let plural = if count == 1 { "" } else { "s" };
        let ids_line = ids
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("  ");

        // STORY-516: forge-routed; reconstruct OpenPrInfo so the Found block is
        // unchanged. The `_` arm already covers every non-Found state.
        // trace:STORY-516 | ai:claude
        match change_lookup_for_branch(project_root, branch) {
            crate::forge::ChangeLookup::Found(c) => {
                let pr = OpenPrInfo {
                    number: c.id,
                    title: c.title.unwrap_or_default(),
                    url: c.url,
                    head_branch: (!c.branch.is_empty()).then_some(c.branch),
                };
                println!(
                    "  {} {} — {} spec{}:",
                    format!("PR-{}", pr.number).cyan().bold(),
                    "(open)".dimmed(),
                    count,
                    plural
                );
                println!(
                    "    {} {}",
                    crate::glyph(crate::glyphs::Glyph::InFlight).bright_green(),
                    ids_line
                );

                // BUG-229: gather the three drain-pipeline signals, then
                // let `classify_open_pr_display` pick the stage. gh's
                // `reviewDecision` (State: approved) is the authoritative
                // "safe to merge" signal and supersedes a lingering
                // reviewer lease. The CI probe is only consulted when no
                // review has started — an extra `gh` round-trip we skip
                // once a lease / APPROVED decision already settles the
                // stage. trace:BUG-229 | ai:claude
                let approved = detect_pr_review_decision(project_root, pr.number)
                    .filter(|d| d.decision == "APPROVED");
                let local_state = detect_local_review_state(project_root, all_reqs, pr.number);
                let ci =
                    if approved.is_none() && matches!(local_state, LocalReviewState::NotStarted) {
                        // STORY-516: forge-routed. `project_root` is already in
                        // scope and feeds both review probes above; route the CI
                        // probe through the same root so all three signals come
                        // from one repository. trace:TASK-1273 | ai:claude
                        ci_probe_with_forge(
                            project_root,
                            crate::forge::resolve_forge_kind(project_root),
                            branch,
                        )
                    } else {
                        CiProbe::NoSignal(String::new())
                    };
                match classify_open_pr_display(approved, local_state, &ci) {
                    InFlightPrDisplay::ReviewApproved { approver } => {
                        let by = approver.map(|a| format!(" by {}", a)).unwrap_or_default();
                        println!(
                            "    {} Review complete{}, awaiting merge — Next: merge (`{}`)",
                            crate::glyph(crate::glyphs::Glyph::FlowActive).bold(),
                            by,
                            format!("gh pr merge {} --squash", pr.number).cyan()
                        );
                    }
                    InFlightPrDisplay::UnderReviewLease { session_id, since } => println!(
                        "    {} {}",
                        crate::glyph(crate::glyphs::Glyph::Hourglass).yellow(),
                        format!(
                            "Under review (reviewer session {} since {})",
                            session_id, since
                        )
                        .yellow()
                    ),
                    InFlightPrDisplay::UnderReviewStory { story_id } => println!(
                        "    {} {}",
                        crate::glyph(crate::glyphs::Glyph::Hourglass).yellow(),
                        format!("Under review (review story {})", story_id).yellow()
                    ),
                    InFlightPrDisplay::CiRunning => println!(
                        "    {} {}",
                        crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
                        "CI running — Next: wait for CI to finish, then review".yellow()
                    ),
                    InFlightPrDisplay::AwaitingReview => println!(
                        "    {} start review (`{}`)",
                        format!("{} Next:", crate::glyph(crate::glyphs::Glyph::FlowActive)).bold(),
                        review_pickup_hint(pr.number).cyan()
                    ),
                }
            }
            _ => {
                // No open PR for this branch. Either it merged and the
                // local clone hasn't pulled yet (State 4), or no PR was
                // ever opened.
                match detect_merged_pr_with_time(project_root, branch) {
                    Some((number, merged_at)) => {
                        println!(
                            "  {} {} — {} spec{}:",
                            format!("PR-{}", number).cyan().bold(),
                            "(merged)".dimmed(),
                            count,
                            plural
                        );
                        println!(
                            "    {} {}",
                            crate::glyph(crate::glyphs::Glyph::InFlight).bright_green(),
                            ids_line
                        );
                        let when = merged_at
                            .map(|t| format!("Merged {}", t))
                            .unwrap_or_else(|| "Merged".to_string());
                        println!(
                            "    {} {} — Next: `{}` (auto-bumps {} spec{} to Completed)",
                            crate::glyph(crate::glyphs::Glyph::FlowActive).bold(),
                            when,
                            "aida pull".cyan(),
                            count,
                            plural
                        );
                    }
                    None => {
                        println!(
                            "  {} {} — {} spec{}:",
                            branch.cyan().bold(),
                            "(no PR opened yet)".dimmed(),
                            count,
                            plural
                        );
                        println!(
                            "    {} {}",
                            crate::glyph(crate::glyphs::Glyph::InFlight).bright_green(),
                            ids_line
                        );
                        println!(
                            "    {} open a PR (`{}`), then merge → `{}`",
                            format!("{} Next:", crate::glyph(crate::glyphs::Glyph::FlowActive))
                                .bold(),
                            "/aida-pr".cyan(),
                            "aida pull".cyan()
                        );
                    }
                }
            }
        }
    }

    // ---- Stuck — auto-bump missed ----
    if !stuck.is_empty() {
        if !awaiting.is_empty() {
            println!();
        }
        println!(
            "  {} ({} spec{}):",
            "Stuck — PR merged, auto-bump missed".yellow().bold(),
            stuck.len(),
            if stuck.len() == 1 { "" } else { "s" }
        );
        for (id, pr, ago) in &stuck {
            let pr_note = pr
                .map(|n| format!("(#{}, {})", n, ago))
                .unwrap_or_else(|| format!("(merged {})", ago));
            println!(
                "    {} {}  {}",
                crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
                id.bold(),
                pr_note.dimmed()
            );
        }
        println!(
            "    {} `{}` replays the missed bumps to Completed",
            format!("{} Next:", crate::glyph(crate::glyphs::Glyph::FlowActive)).bold(),
            "aida db reconcile-status".cyan()
        );
    }

    // ---- Awaiting commit ----
    if !no_commit.is_empty() {
        if !awaiting.is_empty() || !stuck.is_empty() {
            println!();
        }
        println!(
            "  {} ({} spec{}):",
            "Awaiting commit — no commit references the spec yet"
                .dimmed()
                .bold(),
            no_commit.len(),
            if no_commit.len() == 1 { "" } else { "s" }
        );
        println!(
            "    {} {}",
            crate::glyph(crate::glyphs::Glyph::InFlight).dimmed(),
            no_commit.join("  ")
        );
        // TASK-671: name the real SPEC-ID (it's the payload the user must type
        // into the commit trailer — an allowed user-facing id use per TASK-268),
        // be remote-aware (no "open a PR" for a local-only project with no
        // origin), and show a literal commit example so the trailer isn't
        // dropped — the exact mistake that strands a spec in this state.
        // trace:TASK-671 | ai:claude
        let has_origin = aida_core::git_ops::has_remote(project_root, "origin");
        if no_commit.len() == 1 {
            let id = &no_commit[0];
            if has_origin {
                println!(
                    "    {} commit your change with `({})` in the message, then open a PR — the merge auto-completes it.",
                    format!("{} Next:", crate::glyph(crate::glyphs::Glyph::FlowActive)).bold(),
                    id.cyan()
                );
            } else {
                println!(
                    "    {} commit your change with `({})` in the message — that auto-completes it on the next `{}`.",
                    format!("{} Next:", crate::glyph(crate::glyphs::Glyph::FlowActive)).bold(),
                    id.cyan(),
                    "aida pull".cyan()
                );
            }
            println!(
                "            e.g.  {}",
                format!("git commit -m \"... ({})\"", id).dimmed()
            );
        } else {
            if has_origin {
                println!(
                    "    {} commit each with its own `({})` trailer in the message, then open a PR — the merge auto-completes each.",
                    format!("{} Next:", crate::glyph(crate::glyphs::Glyph::FlowActive)).bold(),
                    "SPEC-ID".cyan()
                );
            } else {
                println!(
                    "    {} commit each with its own `({})` trailer — that auto-completes each on the next `{}`.",
                    format!("{} Next:", crate::glyph(crate::glyphs::Glyph::FlowActive)).bold(),
                    "SPEC-ID".cyan(),
                    "aida pull".cyan()
                );
            }
            println!(
                "            e.g.  {}",
                "git commit -m \"... (SPEC-ID)\"".dimmed()
            );
        }
    }
}

/// One record from `git worktree list --porcelain`. The main worktree is
/// always the first record; linked worktrees follow. Detached worktrees
/// produce `None` for `branch`.
/// trace:STORY-385 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct WorktreeRecord {
    pub(crate) path: std::path::PathBuf,
    pub(crate) branch: Option<String>,
}

/// Parse `git worktree list --porcelain` output. One record per worktree,
/// separated by blank lines.
pub(crate) fn list_worktrees(project_root: &std::path::Path) -> Vec<WorktreeRecord> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "list", "--porcelain"])
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    let mut records = Vec::new();
    let mut cur_path: Option<std::path::PathBuf> = None;
    let mut cur_branch: Option<String> = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            // Flush previous record if any.
            if let Some(path) = cur_path.take() {
                records.push(WorktreeRecord {
                    path,
                    branch: cur_branch.take(),
                });
            }
            cur_path = Some(std::path::PathBuf::from(p));
        } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
            cur_branch = Some(b.to_string());
        }
    }
    if let Some(path) = cur_path {
        records.push(WorktreeRecord {
            path,
            branch: cur_branch,
        });
    }
    records
}

/// Open-PR snapshot keyed by head branch — one `gh` invocation feeds the
/// "open PRs" and "branches ahead with no PR" detectors. Empty when gh is
/// missing or the call fails (every dependent detector silently
/// degrades).
/// trace:STORY-385 | ai:claude
#[derive(Debug, Clone, Default)]
pub(crate) struct OpenPrSnapshot {
    pub(crate) by_branch: std::collections::HashMap<String, status_cleanup::OpenPrItem>,
}

pub(crate) fn collect_open_prs(project_root: &std::path::Path) -> OpenPrSnapshot {
    // BUG-613: one `aida status` resolves the open-PR snapshot from three
    // sections (the JSON `open_prs` block, the worktree-rows PR merge, and the
    // awaiting-you mergeable filter), each previously firing its own
    // `gh pr list` — ~0.4s of network latency apiece. PR state does not change
    // within a single short-lived `status` run, so memoize the snapshot for the
    // process lifetime, keyed by the canonicalized repo root (so the rare
    // cross-repo caller still gets the right answer). trace:BUG-613 | ai:claude
    use std::sync::Mutex;
    use std::sync::OnceLock;
    static CACHE: OnceLock<Mutex<std::collections::HashMap<std::path::PathBuf, OpenPrSnapshot>>> =
        OnceLock::new();
    let key = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(hit) = guard.get(&key) {
            return hit.clone();
        }
    }
    let snapshot = collect_open_prs_uncached(project_root);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, snapshot.clone());
    }
    snapshot
}

/// BUG-613: the uncached single-`gh pr list` snapshot fetch behind the
/// process-lifetime memo in [`collect_open_prs`]. trace:BUG-613 | ai:claude
pub(crate) fn collect_open_prs_uncached(project_root: &std::path::Path) -> OpenPrSnapshot {
    // TASK-833: mirror the `collect_pr_facts` forge-aware degrade (BUG-560) — on
    // a non-GitHub forge `gh` errors with a raw "not a known GitHub host" auth
    // message; skip the spawn entirely and degrade to an empty snapshot so every
    // dependent surface (status-cleanup detectors + the burndown-status open-PR
    // section) renders nothing rather than leaking that error.
    // trace:TASK-833 | ai:claude
    // BUG-1454 F3, DECIDED AND KEPT: this empty-on-unknown is the same SHAPE as
    // the `specs_with_open_prs` defect, but not the same consequence, so it is
    // deliberately left alone rather than widened into an Option.
    //
    // The distinction that matters is what a consumer DOES with emptiness, not
    // whether emptiness is ambiguous. Every consumer here degrades toward
    // silence in a surface that only informs: `active_reviewer_unmerged_pr`
    // (STORY-127) withholds a WARNING, the release check at the STORY-127
    // detector withholds a WARNING, and the `aida status` open-PR section
    // renders no rows. None of them change a spec's state. The defect in
    // `specs_with_open_prs` was that emptiness AUTHORISED an irreversible
    // Done -> Completed bump, hiding unfinished work.
    //
    // Note also that this default is already returned on four paths (non-GitHub
    // forge, gh missing, gh failure, unparseable output), so the forge guard
    // joins an existing conflation rather than creating one; typing "unknown"
    // here would mean typing all four and every consumer.
    // trace:BUG-1454 | ai:claude
    if forge::resolve_forge_kind(project_root) != forge::ForgeKind::GitHub {
        return OpenPrSnapshot::default();
    }
    let gh_bin = match resolve_gh_binary() {
        Some(p) => p,
        None => return OpenPrSnapshot::default(),
    };
    let mut cmd = std::process::Command::new(&gh_bin);
    cmd.current_dir(project_root).args([
        "pr",
        "list",
        "--state",
        "open",
        "--limit",
        "50",
        "--json",
        "number,title,headRefName,headRefOid,statusCheckRollup,mergeable,reviewDecision,labels,createdAt",
    ]);
    // trace:BUG-1288 | ai:claude — bounded like every other gh call this
    // machine-readable pipeline makes; see FORGE_CLI_CALL_TIMEOUT.
    let Some(out) = command_output_with_timeout(cmd, FORGE_CLI_CALL_TIMEOUT) else {
        return OpenPrSnapshot::default();
    };
    if !out.status.success() {
        return OpenPrSnapshot::default();
    }
    // BUG-1481: fetch the base branch's required status checks once per
    // snapshot and thread them through so `ci_rollup` can't read "pass" off
    // a head that is simply missing a required check's row entirely.
    // trace:BUG-1481 | ai:claude
    let required = required_status_checks(project_root);
    parse_open_pr_snapshot(&String::from_utf8_lossy(&out.stdout), required.as_deref())
}

/// BUG-1481: the base branch's required status-check names (branch
/// protection's `required_status_checks.contexts`), so `ci_rollup` can tell
/// "nothing is required" apart from "a required check never showed up on
/// this head". `None` means the set itself could not be determined — gh
/// missing, offline, or no permission to read protection — and callers must
/// treat that as *unknown*, never as "nothing required" (PRIN-5: absent
/// evidence is not good evidence). A branch that is genuinely unprotected
/// (the API's 404 "Branch not protected") maps to `Some(vec![])`, which is a
/// real, positive answer, not an unreadable one. Cached once per process per
/// project root, mirroring `collect_open_prs`'s BUG-613 memo, so one
/// `aida awaiting` run pays for this API call once regardless of PR count.
// trace:BUG-1481 | ai:claude
pub(crate) fn required_status_checks(project_root: &std::path::Path) -> Option<Vec<String>> {
    use std::sync::Mutex;
    use std::sync::OnceLock;
    static CACHE: OnceLock<
        Mutex<std::collections::HashMap<std::path::PathBuf, Option<Vec<String>>>>,
    > = OnceLock::new();
    let key = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(hit) = guard.get(&key) {
            return hit.clone();
        }
    }
    let result = required_status_checks_uncached(project_root);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, result.clone());
    }
    result
}

/// The uncached lookup behind [`required_status_checks`].
// trace:BUG-1481 | ai:claude
pub(crate) fn required_status_checks_uncached(
    project_root: &std::path::Path,
) -> Option<Vec<String>> {
    // Same forge guard as `collect_open_prs_uncached`: no gh-shaped
    // protection API to ask on a non-GitHub forge, so there is nothing to
    // treat as "required" — legitimately empty, not unknown.
    if forge::resolve_forge_kind(project_root) != forge::ForgeKind::GitHub {
        return Some(Vec::new());
    }
    let gh_bin = resolve_gh_binary()?;
    let default_branch = detect_default_branch_ref(project_root)
        .and_then(|r| r.rsplit('/').next().map(str::to_string))
        .unwrap_or_else(|| "main".to_string());
    let mut cmd = std::process::Command::new(&gh_bin);
    cmd.current_dir(project_root).args([
        "api",
        &format!("repos/{{owner}}/{{repo}}/branches/{default_branch}/protection"),
        "--jq",
        ".required_status_checks.contexts // []",
    ]);
    // BUG-1288: this specific call measured 10.5s of wall clock in this
    // repository — a single slow branch-protection lookup was enough to blow
    // the whole `aida awaiting --json` / `aida status --full` budget on its
    // own. `None` here degrades exactly like every other unreachable-forge
    // path this function already handles (PRIN-5: unknown, not "nothing
    // required"). trace:BUG-1288 | ai:claude
    let out = command_output_with_timeout(cmd, FORGE_CLI_CALL_TIMEOUT)?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return required_status_checks_outcome_from_stderr(&stderr);
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    Some(
        parsed
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::to_string)
            .collect(),
    )
}

/// BUG-1481: classify a failed `gh api .../protection` call's stderr into
/// "nothing is required" vs "unknown". GitHub returns HTTP 404 for BOTH a
/// genuinely unprotected branch (body `{"message":"Branch not protected",...}`)
/// AND a protected branch the caller lacks permission to read protection on
/// (body `{"message":"Not Found",...}`) — so a bare "404"/"not found"
/// substring match conflates the two and can silently report a protected
/// branch as having no required checks (the exact PR-2009 false-green shape).
/// Only the literal "Branch not protected" message is a real, positive
/// "nothing required" answer; everything else (permission denied, network
/// failure, the ambiguous plain "not found") is genuinely unknown and must
/// never be treated as "nothing required" (PRIN-5: absent evidence is not
/// good evidence).
// trace:BUG-1481 | ai:claude
pub(crate) fn required_status_checks_outcome_from_stderr(stderr: &str) -> Option<Vec<String>> {
    let stderr = stderr.to_ascii_lowercase();
    // prose-ok: matches gh's literal 'Branch not protected' answer (BUG-1481)
    if stderr.contains("branch not protected") {
        return Some(Vec::new());
    }
    None
}

/// BUG-1291: bounded safety net for runs killed before their normal reviewer
/// handoff. The scheduler tick calls this once over the forge's already-bounded
/// open-PR list. It only claims review work; it never reviews or merges.
// trace:BUG-1291 trace:TASK-1284 | ai:codex
pub(crate) fn sweep_orphaned_reviews(project_root: &std::path::Path) -> Vec<String> {
    let live_branches = live_owned_branches(project_root);

    let mut lines = Vec::new();
    for pr in collect_open_prs_uncached(project_root)
        .by_branch
        .into_values()
    {
        let clean = pr.mergeable.as_deref() == Some("MERGEABLE");
        let green = matches!(pr.ci_rollup.as_deref(), None | Some("pass"));
        let no_verdict =
            pr.review_decision.is_none() && !pr_has_local_verdict(project_root, pr.number);
        let no_hold = !pr_has_merge_hold(project_root, &pr);
        let unowned = !live_branches.contains(&pr.head_branch);
        if !orphaned_review_is_claimable(clean, green, no_verdict, no_hold, unowned) {
            continue;
        }

        let outcome = try_auto_queue_pr_review(
            project_root,
            &pr.head_branch,
            "orphan-sweep",
            AutoQueueOrigin::PrSkill,
        );
        if matches!(
            outcome.status,
            AutoQueueStatus::Filed | AutoQueueStatus::AlreadyExists
        ) {
            // Filed now means queue insertion succeeded; AlreadyExists means
            // reviewer_queue_story_ids verified an existing queue owner.
            lines.push(format!(
                "schedule tick: claimed orphaned PR #{} for reviewer",
                pr.number
            ));
        } else if matches!(outcome.status, AutoQueueStatus::SkippedNeedsAttention) {
            lines.push(format!(
                "schedule tick: orphaned PR #{} still unclaimed: {}",
                pr.number, outcome.summary
            ));
        }
    }
    lines
}

// TASK-192: shared ownership fact for the green orphan-review sweep and the
// red unowned-repair awaiting channel. A lease is ownership only while either
// recorded process identity is still alive.
// trace:TASK-192 | ai:codex
pub(crate) fn live_owned_branches(
    project_root: &std::path::Path,
) -> std::collections::HashSet<String> {
    list_leases(project_root)
        .into_iter()
        .filter(|lease| {
            lease.active_pid.is_some_and(|pid| {
                process_probe::process_identity_is_alive(
                    pid,
                    lease.active_pid_start_time.as_deref(),
                )
            }) || lease.creator_pid.is_some_and(|pid| {
                process_probe::process_identity_is_alive(
                    pid,
                    lease.creator_pid_start_time.as_deref(),
                )
            })
        })
        .map(|lease| lease.branch)
        .filter(|branch| !branch.is_empty())
        .collect()
}

// trace:BUG-1291 | ai:codex
pub(crate) fn orphaned_review_is_claimable(
    clean: bool,
    green: bool,
    no_verdict: bool,
    no_hold: bool,
    unowned: bool,
) -> bool {
    clean && green && no_verdict && no_hold && unowned
}

#[cfg(test)]
mod bug_1291_orphan_sweep_tests {
    use super::*;

    fn write_verdict(root: &std::path::Path, verdict: &str) {
        let dir = root.join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("PR-1970.json"),
            format!(r#"{{"verdict":"{verdict}"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn every_valid_local_verdict_blocks_historical_pr_1970_requeue() {
        for verdict in ["Approved", "RequestChanges", "Rejected"] {
            let root = tempfile::tempdir().unwrap();
            write_verdict(root.path(), verdict);
            assert!(
                pr_has_local_verdict(root.path(), 1970),
                "{verdict} must keep PR-1970 / STORY-1354 out of the orphan sweep"
            );
        }
    }

    #[test]
    fn sweep_predicate_requires_no_hold_and_no_live_owner() {
        assert!(orphaned_review_is_claimable(true, true, true, true, true));
        assert!(!orphaned_review_is_claimable(true, true, true, false, true));
        assert!(!orphaned_review_is_claimable(true, true, true, true, false));
    }

    #[test]
    fn malformed_or_missing_verdict_remains_sweep_eligible() {
        let root = tempfile::tempdir().unwrap();
        assert!(!pr_has_local_verdict(root.path(), 1970));
        let dir = root.path().join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("PR-1970.json"), "not-json").unwrap();
        assert!(!pr_has_local_verdict(root.path(), 1970));
    }

    // BUG-1549: end-to-end through the production readers — verdict files on
    // disk (spec-keyed for every title spec + PR-keyed) → `local_suppressed_prs`
    // → `classify_open_prs`, and the same PRs → `pr_review_rows`. The rule
    // itself is table-tested in `awaiting_you::tests::classify_pr_review_table`;
    // this pins the plumbing and the suppressed-iff-row invariant at the
    // report surface.
    // trace:BUG-1490 trace:BUG-1549 | ai:claude
    fn write_verdict_at(root: &std::path::Path, key: &str, verdict: &str, sha: &str, at: &str) {
        let dir = root.join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{key}.json")),
            format!(r#"{{"verdict":"{verdict}","reviewed_sha":"{sha}","recorded_at":"{at}"}}"#),
        )
        .unwrap();
    }

    fn open_pr(number: u64, title: &str, head_sha: Option<&str>) -> status_cleanup::OpenPrItem {
        status_cleanup::OpenPrItem {
            number,
            is_draft: false,
            title: title.to_string(),
            head_branch: format!("claude/pr-{number}"),
            ci_rollup: Some("pass".to_string()),
            mergeable: Some("MERGEABLE".to_string()),
            review_decision: None,
            head_sha: head_sha.map(str::to_string),
            labels: Vec::new(),
            created_at: None,
        }
    }

    #[test]
    fn pr_review_suppression_and_rows_agree_end_to_end() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        let head = "cafef00d0000000001";
        let old = "deadbeef0000000001";
        let (t1, t2) = ("2026-09-01T00:00:00+00:00", "2026-09-02T00:00:00+00:00");

        // 5001: RequestChanges at head (spec-keyed) → blocked.
        write_verdict_at(r, "BUG-5001", "request-changes", head, t1);
        // 5002: refusal moved past → mergeable, rework-ready row.
        write_verdict_at(r, "BUG-5002", "request-changes", old, t1);
        // 5003: spec-keyed refusal t1 superseded by PR-keyed approval at head t2.
        write_verdict_at(r, "BUG-5003", "request-changes", head, t1);
        write_verdict_at(r, "PR-5003", "approved", head, t2);
        // 5004: two title specs — first approves at head t1, SECOND refuses sha-less
        // at t2 (newer, so not superseded).
        write_verdict_at(r, "BUG-5004", "approved", head, t1);
        write_verdict_at(r, "BUG-5014", "request-changes", "", t2);
        // 5005: spec-keyed approval at head t1, PR-keyed sha-less approval t2.
        write_verdict_at(r, "BUG-5005", "approved", head, t1);
        write_verdict_at(r, "PR-5005", "approved", "", t2);
        // 5006: stale approval.
        write_verdict_at(r, "BUG-5006", "approved", old, t1);
        // 5007: approval at head → mergeable, no row.
        write_verdict_at(r, "BUG-5007", "approved", head, t1);
        // 5008: head-less PR with a sha'd approval → unverifiable.
        write_verdict_at(r, "BUG-5008", "approved", head, t1);
        // 5009: no verdict at all → mergeable, no row.

        let prs = vec![
            open_pr(5001, "fix: a (BUG-5001)", Some(head)),
            open_pr(5002, "fix: b (BUG-5002)", Some(head)),
            open_pr(5003, "fix: c (BUG-5003)", Some(head)),
            open_pr(5004, "fix: d (BUG-5004) (BUG-5014)", Some(head)),
            open_pr(5005, "fix: e (BUG-5005)", Some(head)),
            open_pr(5006, "fix: f (BUG-5006)", Some(head)),
            open_pr(5007, "fix: g (BUG-5007)", Some(head)),
            open_pr(5008, "fix: h (BUG-5008)", None),
            open_pr(5009, "fix: i (BUG-5009)", Some(head)),
        ];

        let suppressed = local_suppressed_prs(r, &prs);
        let mut want: Vec<u64> = vec![5001, 5004, 5005, 5006, 5008];
        let mut got: Vec<u64> = suppressed.iter().copied().collect();
        got.sort_unstable();
        want.sort_unstable();
        assert_eq!(got, want, "suppressed set");

        let mergeable: Vec<u64> = awaiting_you::classify_open_prs(&prs, &suppressed)
            .iter()
            .map(|m| m.number)
            .collect();
        assert_eq!(mergeable, vec![5002, 5003, 5007, 5009], "mergeable set");

        // Rows under a seat that recorded nothing: explaining rows are never
        // seat-filtered, so the invariant holds at the report surface.
        let rows = pr_review_rows(r, &prs, Some("nobody-in-particular"));
        let mut explained: Vec<u64> = rows
            .blocked_reviews
            .iter()
            .map(|b| b.pr)
            .chain(rows.stale_approvals.iter().map(|s| s.pr))
            .collect();
        explained.sort_unstable();
        assert_eq!(explained, want, "suppressed iff an explaining row exists");

        let blocked = |n: u64| {
            rows.blocked_reviews
                .iter()
                .find(|b| b.pr == n)
                .unwrap()
                .reason
        };
        assert_eq!(blocked(5001), awaiting_you::BlockedReason::AtHead);
        assert_eq!(blocked(5004), awaiting_you::BlockedReason::Unverifiable);
        let stale = |n: u64| {
            rows.stale_approvals
                .iter()
                .find(|s| s.pr == n)
                .unwrap()
                .reason
        };
        assert_eq!(stale(5005), awaiting_you::StaleApprovalReason::Unverifiable);
        assert_eq!(stale(5006), awaiting_you::StaleApprovalReason::Stale);
        assert_eq!(stale(5008), awaiting_you::StaleApprovalReason::Unverifiable);
        assert_eq!(
            rows.rework_ready.iter().map(|w| w.pr).collect::<Vec<_>>(),
            vec![5002],
            "a moved-only refusal keeps its rework-ready row"
        );
    }

    #[cfg(unix)]
    #[test]
    fn real_phase_driver_shelve_handoff_makes_story_1354_claimable() {
        let root = tempfile::tempdir().unwrap();
        let fake_aida = root.path().join("aida");
        let script = r#"#!/bin/sh
if [ "$1" = "list" ]; then
  printf '%s\n' '[{"spec_id":"STORY-1354","title":"Review PR-1970: BUG-1268 stale-base recovery","status":"Approved"}]'
  exit 0
fi
if [ "$1" = "queue" ] && [ "$2" = "add" ]; then
  printf '%s\n' "$3" > .claimed-review
  exit 0
fi
exit 1
"#;
        crate::test_exec::write_executable(&fake_aida, script);

        let mut driver = RealPhaseDriver::new(
            root.path().to_path_buf(),
            "BUG-1268".into(),
            "reviewer-test".into(),
            None,
            true,
            None,
            AutonomyMode::Default,
            "bug-1291-test".into(),
            false,
            false,
            false,
            false,
            auto_complete::LifecycleSkip::none(),
            auto_complete::AutoCompleteVariant::Full,
        );
        driver.aida_exe = fake_aida;
        driver.pr_number = Some(1970);

        auto_complete::PhaseDriver::handoff_open_pr_after_shelve(
            &mut driver,
            "BUG-1268",
            auto_complete::Phase::Reviewer,
            &auto_complete::PhaseFailure::new("reviewer phase shelved"),
        );

        assert_eq!(
            std::fs::read_to_string(root.path().join(".claimed-review"))
                .unwrap()
                .trim(),
            "STORY-1354",
            "the historical review story must be present in the reviewer queue after shelving"
        );
    }

    #[cfg(unix)]
    #[test]
    fn queue_add_failure_is_propagated() {
        let root = tempfile::tempdir().unwrap();
        let fake = root.path().join("failing-aida");
        crate::test_exec::write_executable(
            &fake,
            "#!/bin/sh\necho queue unavailable >&2\nexit 23\n",
        );
        let err = aida_subcmd_queue_add_for_reviewer_using(
            root.path(),
            "STORY-1354",
            "BUG-1291 test",
            &fake,
        )
        .unwrap_err();
        assert!(err.to_string().contains("queue unavailable"), "{err}");
    }

    /// AC3: the retry engages at *this* call site, not merely inside the helper's
    /// own unit test. A live writer descriptor on the freshly written fixture is a
    /// real `ETXTBSY` (the bare-spawn assertion below proves the window is open),
    /// which before BUG-1735 took the `Err(e)` arm and reported
    /// `could not invoke \`aida queue add\``. The retry must instead wait the
    /// window out and land on the `Ok(o)` non-zero-exit arm the caller cares
    /// about, so the assertion is on the *propagated stderr*, not on "no error".
    // trace:BUG-1735 | ai:claude
    #[cfg(target_os = "linux")]
    #[test]
    fn queue_add_waits_out_a_writer_descriptor_instead_of_reporting_could_not_invoke() {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;

        let body = "#!/bin/sh\necho queue unavailable >&2\nexit 23\n";
        let root = tempfile::tempdir().unwrap();
        let fake = root.path().join("failing-aida");
        crate::test_exec::write_executable(&fake, body);

        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(&fake)
            .unwrap();
        writer.write_all(body.as_bytes()).unwrap();
        writer.sync_all().unwrap();

        assert_eq!(
            std::process::Command::new(&fake)
                .spawn()
                .err()
                .and_then(|e| e.raw_os_error()),
            Some(libc::ETXTBSY),
            "the writer descriptor must make a bare spawn fail, or this test proves nothing"
        );

        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(80));
            drop(writer);
        });

        let err = aida_subcmd_queue_add_for_reviewer_using(
            root.path(),
            "STORY-1354",
            "BUG-1735 test",
            &fake,
        )
        .unwrap_err();
        release.join().unwrap();

        assert!(
            err.to_string().contains("queue unavailable"),
            "the spawn must be retried past ETXTBSY and reach the non-zero-exit arm, got: {err}"
        );
        assert!(
            !err.to_string().contains("could not invoke"),
            "a transient ETXTBSY must not surface as a spawn failure, got: {err}"
        );
    }
}

/// TASK-833: pure parse of a `gh pr list --json
/// number,title,headRefName,statusCheckRollup,mergeable,reviewDecision` payload
/// into an `OpenPrSnapshot`. Split out of `collect_open_prs` so it's
/// unit-testable without shelling out; malformed JSON / a missing `number`
/// field degrades silently (empty snapshot / skip the row).
///
/// `required_checks` (BUG-1481) is the base branch's required status-check
/// name set, from [`required_status_checks`]: `None` when it could not be
/// determined, `Some(list)` (possibly empty) when it's known. It only ever
/// pulls a `ci_rollup` of `"pass"` DOWN to `"missing"` (a required check's
/// row never showed up on this head) or `"unknown"` (the required set
/// itself couldn't be read) — it never turns a `"fail"`/`"pending"`/`"?"`
/// into something greener.
// trace:TASK-833 trace:BUG-1481 | ai:claude
pub(crate) fn parse_open_pr_snapshot(
    json: &str,
    required_checks: Option<&[String]>,
) -> OpenPrSnapshot {
    let parsed: serde_json::Value = match serde_json::from_str(json.trim()) {
        Ok(v) => v,
        Err(_) => return OpenPrSnapshot::default(),
    };
    let mut by_branch = std::collections::HashMap::new();
    for pr in parsed.as_array().cloned().unwrap_or_default() {
        let number = pr.get("number").and_then(|v| v.as_u64()).unwrap_or(0);
        if number == 0 {
            continue;
        }
        let title = pr
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let head_branch = pr
            .get("headRefName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let head_sha = pr
            .get("headRefOid")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let mergeable = pr
            .get("mergeable")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let review_decision = pr
            .get("reviewDecision")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let ci_rollup = pr
            .get("statusCheckRollup")
            .and_then(|v| v.as_array())
            .map(|arr| summarize_status_check_rollup_with_required(arr, required_checks));
        let labels = pr
            .get("labels")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|label| label.get("name").and_then(|v| v.as_str()))
            .map(str::to_owned)
            .collect();
        // BUG-1514: same `gh pr list` call, no extra request — drives the
        // unowned-failing-PR age gate. Malformed/missing → None (fail open).
        // trace:BUG-1514 | ai:claude
        let created_at = pr
            .get("createdAt")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));
        by_branch.insert(
            head_branch.clone(),
            status_cleanup::OpenPrItem {
                is_draft: false,
                number,
                title,
                head_branch,
                ci_rollup,
                mergeable,
                review_decision,
                head_sha,
                labels,
                created_at,
            },
        );
    }
    OpenPrSnapshot { by_branch }
}

/// Every branch that has ever been a PR head, across all states
/// (open / closed / merged). Used by the branches-ahead-no-PR detector
/// to distinguish "never PR'd" branches from squash-merged ones that
/// git still considers ahead. Empty when gh is missing or the call
/// fails. trace:STORY-385 | ai:claude
pub(crate) fn collect_all_pr_head_branches(
    project_root: &std::path::Path,
) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let gh_bin = match resolve_gh_binary() {
        Some(p) => p,
        None => return set,
    };
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "list",
            "--state",
            "all",
            "--limit",
            "500",
            "--json",
            "headRefName",
        ])
        .output_retrying_etxtbsy();
    let Ok(out) = out else { return set };
    if !out.status.success() {
        return set;
    }
    let parsed: serde_json::Value = match serde_json::from_slice(&out.stdout) {
        Ok(v) => v,
        Err(_) => return set,
    };
    for pr in parsed.as_array().cloned().unwrap_or_default() {
        if let Some(b) = pr.get("headRefName").and_then(|v| v.as_str()) {
            set.insert(b.to_string());
        }
    }
    set
}

#[derive(Debug, Clone)]
pub(crate) struct PrHeadEvidence {
    pub(crate) state: String,
    pub(crate) head_sha: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PrHeadStateSnapshot {
    pub(crate) by_branch: std::collections::HashMap<String, PrHeadEvidence>,
    pub(crate) open_heads_by_spec: std::collections::HashMap<String, Vec<String>>,
    pub(crate) merged_heads_by_spec: std::collections::HashMap<String, Vec<String>>,
    // BUG-1288: branches the per-branch `--head` query in
    // `collect_pr_head_state_snapshot_bounded` actually asked about. A branch
    // present here with no `by_branch` entry has an affirmatively CONFIRMED
    // absence of any PR ("absent"); a branch absent from both was never asked
    // (deadline-truncated) and must read "unknown", never "absent" — a
    // skipped check is not evidence of nothing (PRIN-5). trace:BUG-1288 | ai:claude
    pub(crate) queried_heads: std::collections::HashSet<String>,
}

// BUG-1576: query each bounded candidate by head name instead of sampling the
// first N PRs from repository history. This remains proportional to active
// work, while finding an arbitrarily old merged PR in a repository with more
// than 1,000 PRs. The recorded head SHA identifies the exact branch
// incarnation reviewed and merged; `git cherry` cannot prove an N-to-1 squash.
/// BUG-1288: the per-branch `gh pr list --head` loop below is one network
/// round trip per candidate, so a large candidate population can add several
/// real seconds even though each individual call is fast. `deadline`, when
/// set, stops issuing NEW per-branch queries once it passes; branches not yet
/// queried simply keep whatever `by_branch`/`open_heads_by_spec`/
/// `merged_heads_by_spec` state the initial batched "open" query already gave
/// them (state "unknown" downstream, never a false "absent" — PRIN-5). The
/// initial batched query always runs uncapped: it is one call regardless of
/// candidate count, so there is nothing to bound there. `deadline: None`
/// (every caller but the machine-readable awaiting/status paths) probes every
/// candidate exactly as before this change.
// trace:BUG-1288 | ai:claude
pub(crate) fn collect_pr_head_state_snapshot_bounded(
    project_root: &std::path::Path,
    candidate_branches: &[String],
    deadline: Option<std::time::Instant>,
) -> Option<PrHeadStateSnapshot> {
    let gh_bin = resolve_gh_binary()?;
    let query = |args: &[&str]| -> Option<PrHeadStateSnapshot> {
        let mut cmd = std::process::Command::new(&gh_bin);
        cmd.current_dir(project_root).args(args);
        // trace:BUG-1288 | ai:claude — bounded like every other gh call this
        // machine-readable pipeline makes; see FORGE_CLI_CALL_TIMEOUT.
        let out = command_output_with_timeout(cmd, FORGE_CLI_CALL_TIMEOUT)?;
        out.status
            .success()
            .then(|| parse_pr_head_state_snapshot(&String::from_utf8_lossy(&out.stdout)))?
    };
    let mut snapshot = query(&[
        "pr",
        "list",
        "--state",
        "open",
        "--limit",
        "1000",
        "--json",
        "state,title,headRefName,headRefOid",
    ])?;
    for branch in candidate_branches {
        if deadline.is_some_and(|dl| std::time::Instant::now() >= dl) {
            break;
        }
        // trace:BUG-1288 | ai:claude — record the attempt regardless of
        // outcome, same as the pre-BUG-1288 code implicitly did for every
        // branch (it always attempted every one); only a deadline-skipped
        // branch is now excluded from this set.
        snapshot.queried_heads.insert(branch.clone());
        if let Some(found) = query_pr_head_history(project_root, branch) {
            snapshot.merge(found);
        }
    }
    Some(snapshot)
}

/// One `gh pr list --head <branch> --state all` round trip: the full PR
/// history (open/closed/merged) recorded against this exact head name.
/// BUG-1756: split out of [`collect_pr_head_state_snapshot_bounded`]'s
/// per-candidate loop so the unshipped-work detector can issue it lazily —
/// only for candidates that survive the cheap local git classification —
/// instead of paying one network round trip per candidate up front.
// trace:BUG-1756 | ai:claude
pub(crate) fn query_pr_head_history(
    project_root: &std::path::Path,
    branch: &str,
) -> Option<PrHeadStateSnapshot> {
    let gh_bin = resolve_gh_binary()?;
    let mut cmd = std::process::Command::new(&gh_bin);
    cmd.current_dir(project_root).args([
        "pr",
        "list",
        "--head",
        branch,
        "--state",
        "all",
        "--limit",
        "100",
        "--json",
        "state,title,headRefName,headRefOid",
    ]);
    // trace:BUG-1288 | ai:claude — bounded like every other gh call this
    // machine-readable pipeline makes; see FORGE_CLI_CALL_TIMEOUT.
    let out = command_output_with_timeout(cmd, FORGE_CLI_CALL_TIMEOUT)?;
    out.status
        .success()
        .then(|| parse_pr_head_state_snapshot(&String::from_utf8_lossy(&out.stdout)))?
}

impl PrHeadStateSnapshot {
    pub(crate) fn merge(&mut self, other: Self) {
        for (branch, evidence) in other.by_branch {
            let replace = self.by_branch.get(&branch).is_none_or(|old| {
                (evidence.state == "open" && old.state != "open")
                    || (evidence.state == "merged" && old.state == "closed")
            });
            if replace {
                self.by_branch.insert(branch, evidence);
            }
        }
        for (spec, mut heads) in other.open_heads_by_spec {
            self.open_heads_by_spec
                .entry(spec)
                .or_default()
                .append(&mut heads);
        }
        for (spec, mut heads) in other.merged_heads_by_spec {
            self.merged_heads_by_spec
                .entry(spec)
                .or_default()
                .append(&mut heads);
        }
        for heads in self.open_heads_by_spec.values_mut() {
            heads.sort();
            heads.dedup();
        }
        for heads in self.merged_heads_by_spec.values_mut() {
            heads.sort();
            heads.dedup();
        }
    }
}

pub(crate) fn parse_pr_head_state_snapshot(json: &str) -> Option<PrHeadStateSnapshot> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    let mut snapshot = PrHeadStateSnapshot::default();
    for row in rows {
        let Some(branch) = row.get("headRefName").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(state) = row.get("state").and_then(|v| v.as_str()) else {
            continue;
        };
        let state = state.to_ascii_lowercase();
        if branch.is_empty() || !matches!(state.as_str(), "open" | "closed" | "merged") {
            continue;
        }
        let title = row.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let mut spec_ids = work_spec_id_from_branch(branch)
            .into_iter()
            .collect::<Vec<_>>();
        spec_ids.extend(extract_spec_ids_from_commit(title));
        spec_ids.sort();
        spec_ids.dedup();
        if state == "open" {
            for spec in &spec_ids {
                snapshot
                    .open_heads_by_spec
                    .entry(spec.to_ascii_uppercase())
                    .or_default()
                    .push(branch.to_string());
            }
        }
        let head_sha = row
            .get("headRefOid")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .map(str::to_string);
        if state == "merged" {
            if let Some(sha) = &head_sha {
                for spec in &spec_ids {
                    snapshot
                        .merged_heads_by_spec
                        .entry(spec.to_ascii_uppercase())
                        .or_default()
                        .push(sha.clone());
                }
            }
        }
        let evidence = PrHeadEvidence { state, head_sha };
        let replace = snapshot.by_branch.get(branch).is_none_or(|old| {
            (evidence.state == "open" && old.state != "open")
                || (evidence.state == "merged" && old.state == "closed")
        });
        if replace {
            snapshot.by_branch.insert(branch.to_string(), evidence);
        }
    }
    for heads in snapshot.open_heads_by_spec.values_mut() {
        heads.sort();
        heads.dedup();
    }
    for heads in snapshot.merged_heads_by_spec.values_mut() {
        heads.sort();
        heads.dedup();
    }
    Some(snapshot)
}

/// Roll up `statusCheckRollup` into one of `pass`, `fail`, `pending`, or
/// `?`. Counts FAILURE/CANCELLED/TIMED_OUT/ACTION_REQUIRED as fail,
/// IN_PROGRESS/QUEUED/PENDING as pending; SUCCESS only when every check
/// reports success.
pub(crate) fn summarize_status_check_rollup(checks: &[serde_json::Value]) -> String {
    if checks.is_empty() {
        return "?".to_string();
    }
    let mut has_fail = false;
    let mut has_pending = false;
    let mut total = 0usize;
    let mut passed = 0usize;
    for c in checks {
        total += 1;
        let conclusion = c
            .get("conclusion")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_uppercase();
        let status = c
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_uppercase();
        match conclusion.as_str() {
            "SUCCESS" => passed += 1,
            "FAILURE" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" => has_fail = true,
            _ => {
                if matches!(status.as_str(), "IN_PROGRESS" | "QUEUED" | "PENDING") {
                    has_pending = true;
                }
            }
        }
    }
    if has_fail {
        "fail".to_string()
    } else if has_pending {
        "pending".to_string()
    } else if passed == total {
        "pass".to_string()
    } else {
        "?".to_string()
    }
}

/// BUG-1481: [`summarize_status_check_rollup`], corrected for the checks
/// that never ran at all. `ci_rollup: "pass"` means "every check present is
/// green" — that's true of a head carrying one green trivial check and
/// missing the required build entirely, which is exactly the false-green
/// PR-2009 shape. This wrapper never lets a required check's *absence* read
/// as a pass:
///
/// - `required_checks` unresolved (`None`, e.g. branch protection unreadable)
///   downgrades a `"pass"` verdict to `"unknown"` — the required set itself
///   is absent evidence, not evidence of nothing required (PRIN-5).
/// - `required_checks` resolved but naming a check whose row is missing from
///   `checks` downgrades to `"missing"`.
/// - Otherwise the base rollup passes through unchanged, so a `"fail"` /
///   `"pending"` never gets *greener* just because the required set is
///   unknown or incomplete.
// trace:BUG-1481 | ai:claude
pub(crate) fn summarize_status_check_rollup_with_required(
    checks: &[serde_json::Value],
    required_checks: Option<&[String]>,
) -> String {
    let base = summarize_status_check_rollup(checks);
    if base != "pass" {
        return base;
    }
    let Some(required) = required_checks else {
        return "unknown".to_string();
    };
    if required.is_empty() {
        return base;
    }
    let present: std::collections::HashSet<&str> = checks
        .iter()
        .filter_map(|c| {
            c.get("name")
                .and_then(|v| v.as_str())
                .or_else(|| c.get("context").and_then(|v| v.as_str()))
        })
        .collect();
    if required.iter().any(|r| !present.contains(r.as_str())) {
        return "missing".to_string();
    }
    base
}

#[cfg(test)]
mod bug_1481_ci_rollup_required_checks_tests {
    use super::*;

    fn checks(json: &str) -> Vec<serde_json::Value> {
        serde_json::from_str(json).unwrap()
    }

    /// The exact PR-2009 shape from the bug report: one green non-required
    /// check (`merge-hold-gate`) and no row at all for the required `Build
    /// (ubuntu-latest)` check. Must NOT read as "pass".
    // trace:BUG-1481 | ai:claude
    #[test]
    fn missing_required_check_is_not_pass() {
        let arr =
            checks(r#"[{"name":"merge-hold-gate","status":"COMPLETED","conclusion":"SUCCESS"}]"#);
        let required = vec![
            "merge-hold-gate".to_string(),
            "Build (ubuntu-latest)".to_string(),
        ];
        assert_eq!(
            summarize_status_check_rollup_with_required(&arr, Some(&required)),
            "missing"
        );
    }

    /// Every required check present and green → pass, unchanged from today.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn all_required_present_and_green_is_pass() {
        let arr = checks(
            r#"[{"name":"merge-hold-gate","status":"COMPLETED","conclusion":"SUCCESS"},
                {"name":"Build (ubuntu-latest)","status":"COMPLETED","conclusion":"SUCCESS"}]"#,
        );
        let required = vec![
            "merge-hold-gate".to_string(),
            "Build (ubuntu-latest)".to_string(),
        ];
        assert_eq!(
            summarize_status_check_rollup_with_required(&arr, Some(&required)),
            "pass"
        );
    }

    /// Branch protection couldn't be read at all: never claim pass over an
    /// unknown required set.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn unreadable_protection_is_unknown_not_pass() {
        let arr =
            checks(r#"[{"name":"merge-hold-gate","status":"COMPLETED","conclusion":"SUCCESS"}]"#);
        assert_eq!(
            summarize_status_check_rollup_with_required(&arr, None),
            "unknown"
        );
    }

    /// A genuinely unprotected branch (`Some(vec![])`, e.g. the API's 404)
    /// keeps today's behavior — nothing is required, so all-green is pass.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn no_required_checks_configured_is_unchanged() {
        let arr =
            checks(r#"[{"name":"merge-hold-gate","status":"COMPLETED","conclusion":"SUCCESS"}]"#);
        assert_eq!(
            summarize_status_check_rollup_with_required(&arr, Some(&[])),
            "pass"
        );
    }

    /// A required set that can't be read never makes a failing/pending head
    /// look better than it is.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn unreadable_protection_does_not_upgrade_fail_or_pending() {
        let failing = checks(r#"[{"name":"x","status":"COMPLETED","conclusion":"FAILURE"}]"#);
        assert_eq!(
            summarize_status_check_rollup_with_required(&failing, None),
            "fail"
        );
        let pending = checks(r#"[{"name":"x","status":"IN_PROGRESS","conclusion":""}]"#);
        assert_eq!(
            summarize_status_check_rollup_with_required(&pending, None),
            "pending"
        );
    }

    /// The literal "Branch not protected" message is the ONLY real, positive
    /// "nothing required" answer.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn branch_not_protected_message_is_nothing_required() {
        assert_eq!(
            required_status_checks_outcome_from_stderr(
                "gh: Branch not protected (HTTP 404)\n{\"message\":\"Branch not protected\"}"
            ),
            Some(Vec::new())
        );
    }

    /// A bare 404 with a DIFFERENT message (e.g. a protected branch the
    /// caller lacks permission to read protection on) must NOT be read as
    /// "nothing required" — that is the exact PR-2009 false-green shape via
    /// the "404"/"not found" substring match this replaces.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn bare_404_with_other_message_is_unknown() {
        assert_eq!(
            required_status_checks_outcome_from_stderr(
                "gh: Not Found (HTTP 404)\n{\"message\":\"Not Found\"}"
            ),
            None
        );
    }

    /// Permission-denied / network failure: unknown, never "nothing
    /// required".
    // trace:BUG-1481 | ai:claude
    #[test]
    fn permission_denied_is_unknown() {
        assert_eq!(
            required_status_checks_outcome_from_stderr(
                "gh: Must have admin rights to Repository. (HTTP 403)"
            ),
            None
        );
    }
}

/// Walk recent default-branch commits and recover the `spec_id → sha`
/// pairs that would auto-bump a Done spec to Completed. Returns the
/// flips that *would* land — the caller filters them against current
/// store state. Cheaper than `auto_bump_done_to_completed` because it
/// doesn't require being checked out on main.
/// trace:STORY-385 | ai:claude
pub(crate) fn scan_default_branch_for_spec_landings(
    project_root: &std::path::Path,
    limit: u32,
) -> Vec<(String, String)> {
    let default = match detect_default_branch_ref(project_root) {
        Some(r) => r,
        None => return Vec::new(),
    };
    let log = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "log",
            "--pretty=format:%H%x09%s",
            &format!("--max-count={limit}"),
            &default,
        ])
        .output();
    let Ok(log) = log else { return Vec::new() };
    if !log.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&log.stdout);
    // Spec → first-seen sha (matches auto-bump "earliest landing wins").
    let mut seen: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for line in text.lines() {
        let mut parts = line.splitn(2, '\t');
        let sha = parts.next().unwrap_or("").trim();
        let subject = parts.next().unwrap_or("").trim();
        if sha.is_empty() || subject.is_empty() {
            continue;
        }
        for spec_id in extract_spec_ids_from_commit(subject) {
            seen.entry(spec_id).or_insert_with(|| sha.to_string());
        }
    }
    seen.into_iter().collect()
}

/// True when `branch` is ahead of `target_ref` (defaults to the project's
/// detected default ref).
pub(crate) fn branch_ahead_of(
    project_root: &std::path::Path,
    branch: &str,
    target_ref: &str,
) -> Option<u32> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-list",
            "--count",
            &format!("{}..{}", target_ref, branch),
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// TASK-1056: batch every local branch's commits-ahead-of-`target_ref` count
/// into ONE `git for-each-ref` (the `ahead-behind` field, git ≥ 2.41) instead
/// of a `git rev-list --count <target>..<branch>` per branch. On a fleet repo
/// the per-branch loop spawned a hundred-plus rev-list processes; this is one.
/// The map's value equals `branch_ahead_of(.., branch, target_ref)` for the
/// same branch, so callers that swap the lookup in render byte-identical
/// output. Returns an EMPTY map when the field is unsupported (older git makes
/// `for-each-ref` exit non-zero on the unknown atom) so callers transparently
/// fall back to the per-branch probe.
// trace:TASK-1056 | ai:claude
pub(crate) fn collect_branch_ahead_of(
    project_root: &std::path::Path,
    target_ref: &str,
) -> std::collections::HashMap<String, u32> {
    let mut map = std::collections::HashMap::new();
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "for-each-ref",
            &format!("--format=%(refname:short)%09%(ahead-behind:{target_ref})"),
            "refs/heads/",
        ])
        .output();
    let Ok(out) = out else { return map };
    if !out.status.success() {
        // Older git: the `ahead-behind` atom is unknown and the whole call
        // fails — leave the map empty so callers fall back per-branch.
        return map;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.splitn(2, '\t');
        let Some(name) = parts.next() else { continue };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        // The `ahead-behind` field renders "<ahead> <behind>"; we want ahead.
        // An empty field (e.g. target_ref unresolved for this ref) is skipped
        // so the caller falls back rather than recording a bogus 0.
        let Some(ahead) = parts
            .next()
            .and_then(|ab| ab.split_whitespace().next().map(|s| s.to_string()))
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        map.insert(name.to_string(), ahead);
    }
    map
}

/// `git for-each-ref refs/heads/` — just the branch names.
pub(crate) fn list_local_branches(project_root: &std::path::Path) -> Vec<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// One `git for-each-ref` over `refs/heads/` returning each local branch's name
/// paired with its tip-commit time (unix seconds). Collapses a per-branch
/// `git log -1 --format=%ct` fan-out (one process per branch — thousands on a
/// fleet repo) into a single git invocation that yields the same data.
// trace:TASK-1056 | ai:claude
pub(crate) fn collect_local_branch_commit_times(
    project_root: &std::path::Path,
) -> std::collections::HashMap<String, i64> {
    let mut map = std::collections::HashMap::new();
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "for-each-ref",
            "--format=%(refname:short)%09%(committerdate:unix)",
            "refs/heads/",
        ])
        .output();
    let Ok(out) = out else { return map };
    if !out.status.success() {
        return map;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.splitn(2, '\t');
        let Some(name) = parts.next() else { continue };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let ts = parts
            .next()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0);
        map.insert(name.to_string(), ts);
    }
    map
}

/// One `git for-each-ref` over `refs/remotes/origin` returning the set of
/// remote-tracking branch short names with the `origin/` prefix stripped (and
/// the symbolic `origin/HEAD` excluded). Collapses a per-branch
/// `git rev-parse --verify --quiet origin/<branch>` existence fan-out into a
/// single git invocation; membership in the set is equivalent to a successful
/// verify.
// trace:TASK-1056 | ai:claude
pub(crate) fn collect_remote_branch_name_set(
    project_root: &std::path::Path,
) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/remotes/origin",
        ])
        .output();
    let Ok(out) = out else { return set };
    if !out.status.success() {
        return set;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let name = line.trim();
        let short = name.strip_prefix("origin/").unwrap_or(name);
        if short.is_empty() || short == "HEAD" {
            continue;
        }
        set.insert(short.to_string());
    }
    set
}

/// One `git for-each-ref` over `refs/heads/` AND `refs/remotes/origin`
/// returning each ref's short name (exactly the display spelling the
/// unshipped-work candidate list uses: `bug-x` locally, `origin/bug-x` for
/// remote-tracking refs) paired with its tip-commit time (unix seconds).
/// BUG-1756: drives the newest-first probe order of the bounded scan.
// trace:BUG-1756 | ai:claude
pub(crate) fn collect_candidate_tip_times(
    project_root: &std::path::Path,
) -> std::collections::HashMap<String, i64> {
    let mut map = std::collections::HashMap::new();
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "for-each-ref",
            "--format=%(refname:short)%09%(committerdate:unix)",
            "refs/heads/",
            "refs/remotes/origin",
        ])
        .output();
    let Ok(out) = out else { return map };
    if !out.status.success() {
        return map;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.splitn(2, '\t');
        let Some(name) = parts.next() else { continue };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let ts = parts
            .next()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0);
        map.insert(name.to_string(), ts);
    }
    map
}

// trace:STORY-1043 | ai:codex
#[derive(Debug, Clone)]
pub(crate) struct UnshippedBranchCandidate {
    pub(crate) branch: String,
    pub(crate) refname: String,
    pub(crate) local_branch: String,
    pub(crate) spec_id: String,
    pub(crate) commits_ahead: u32,
    pub(crate) age: String,
    pub(crate) has_local: bool,
    // BUG-1531: the candidate's tip commit, used to bind a "do not ship"
    // refusal to the exact commit a reviewer looked at rather than to a PR
    // number or a branch-naming convention. `None` only when `git rev-parse`
    // itself fails, in which case the sha-bound checks are skipped rather
    // than guessed.
    // trace:BUG-1531 | ai:claude
    pub(crate) tip_sha: Option<String>,
    // BUG-1531: the branch's name has the `pr-N`/`mr-N` review-snapshot
    // shape, but the forge did NOT confirm it (no forge, lookup failure, or
    // a head sha that doesn't match this branch's tip) — so it is kept in
    // the report rather than silently excluded (PRIN-5: never hide on name
    // alone), labelled as unverified with no ship hint.
    // trace:BUG-1531 | ai:claude
    pub(crate) possible_review_snapshot: bool,
    // BUG-1756: whether a remote counterpart of this branch exists
    // (`origin/<name>`; trivially true for a remote-only row). `false` means
    // the commits exist on exactly one machine — the most strandable
    // unshipped state — and drives a push-first recovery hint instead of a
    // bare `aida pr ship`. trace:BUG-1756 | ai:claude
    pub(crate) pushed: bool,
}

/// BUG-1756: a candidate that survived the cheap local classification pass
/// and is waiting for (or has received) its per-head forge evidence. The
/// review-snapshot NAME detection happens in the local pass; the forge
/// CONFIRMATION (a network call) happens in the bounded evidence pass, so
/// `review_shape` carries the parsed shape between the two.
// trace:BUG-1756 | ai:claude
pub(crate) struct UnshippedSurvivor {
    pub(crate) cand: UnshippedBranchCandidate,
    pub(crate) review_shape: Option<(forge::ForgeKind, u64)>,
    /// The forge confirmed PR/MR N exists with a head sha equal to this
    /// branch's tip — the one condition BUG-1531 allows the row to be
    /// excluded on.
    pub(crate) confirmed_review_snapshot: bool,
}

// BUG-1288: a bounded candidate gate for the unshipped-work detector. Kept
// pure so a large stale-ref population can be pinned without timing-sensitive
// process tests. trace:BUG-1288 | ai:codex
pub(crate) fn branch_belongs_to_active_work(branch: &str, active_prefixes: &[String]) -> bool {
    active_prefixes.iter().any(|prefix| {
        branch == prefix
            || branch
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with('-'))
    })
}

// trace:BUG-1187 | ai:codex
pub(crate) fn all_requirement_summaries(
    project_root: &std::path::Path,
) -> Vec<aida_core::RequirementSummary> {
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Vec::new();
    };
    let Ok(dispenser) = load_dispenser(&store_path) else {
        return Vec::new();
    };
    let Ok(inner) = aida_core::GitBackend::new(&store_path).map(|b| b.with_dispenser(dispenser))
    else {
        return Vec::new();
    };
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let Ok(backend) = aida_core::CachedGitBackend::with_inner(inner, &cache_path) else {
        return Vec::new();
    };
    backend
        .list_summaries(&aida_core::ListFilter {
            archive: aida_core::ArchiveFilter::Both,
            defer: aida_core::DeferFilter::Both,
            ..Default::default()
        })
        .unwrap_or_default()
}

// trace:BUG-1187 | ai:codex
pub(crate) fn insert_summary_statuses(
    status_by_spec: &mut std::collections::HashMap<String, String>,
    summaries: &[aida_core::RequirementSummary],
) {
    for s in summaries {
        let Some(id) = s.agreed_id.clone().or_else(|| s.spec_id.clone()) else {
            continue;
        };
        status_by_spec.insert(id.to_ascii_uppercase(), s.status.to_ascii_lowercase());
    }
}

// trace:STORY-1043 | ai:codex
pub(crate) fn collect_unshipped_work_items(
    project_root: &std::path::Path,
    summaries: &[aida_core::RequirementSummary],
    no_forge: bool,
    emit_detected_events: bool,
) -> Vec<awaiting_you::UnshippedWorkItem> {
    collect_unshipped_work_items_bounded(
        project_root,
        summaries,
        no_forge,
        emit_detected_events,
        None,
    )
    .0
}

/// BUG-1288: time-boxed sibling of [`collect_unshipped_work_items`], used by
/// the machine-readable polling paths (`aida awaiting --json`, `aida status
/// --full`). The candidate-branch SET is computed exactly as before (PR
/// #1999 / STORY-1368's exclude-list, unchanged) — this only bounds how long
/// probing that set may run. `deadline == None` behaves identically to the
/// unbounded original (every other caller, including tests and
/// `session_reap`, which need the exhaustive answer regardless of cost).
///
/// Two independent probes can consume wall clock per candidate: the local
/// git commit/patch-equivalence walk and the per-branch `gh pr list --head`
/// network round trip ([`query_pr_head_history`]). Both consult the same
/// `deadline`, so the combined budget is shared rather than doubled — and
/// since BUG-1756 the CHEAP local walk runs first for every candidate, with
/// the network round trips paid only for the candidates that survive it.
/// (Before that reorder the up-front per-candidate network calls consumed
/// the whole budget on a working repository, so the bounded scan always
/// truncated at 0/N and the channel was effectively dead.) Once the deadline
/// passes, remaining candidate branches are simply not probed — the returned
/// [`awaiting_you::UnshippedScanStatus`] records `complete: false` plus how
/// many of the total candidates were actually scanned, so a truncated run is
/// never presented as an exhaustive one (PRIN-5). A survivor whose evidence
/// query the deadline skipped counts as UNSCANNED and is dropped rather than
/// reported on evidence that was never fetched. What IS returned is
/// unaffected: nothing already found is dropped, and nothing is hidden by
/// widening or narrowing which branches count as candidates.
// trace:BUG-1288 | ai:claude
pub(crate) fn collect_unshipped_work_items_bounded(
    project_root: &std::path::Path,
    summaries: &[aida_core::RequirementSummary],
    no_forge: bool,
    emit_detected_events: bool,
    deadline: Option<std::time::Instant>,
) -> (
    Vec<awaiting_you::UnshippedWorkItem>,
    awaiting_you::UnshippedScanStatus,
) {
    let Some(default_ref) = detect_default_branch_ref(project_root) else {
        return (
            Vec::new(),
            awaiting_you::UnshippedScanStatus {
                complete: true,
                scanned: 0,
                candidates: 0,
            },
        );
    };

    let mut status_by_spec = std::collections::HashMap::new();
    insert_summary_statuses(&mut status_by_spec, summaries);
    insert_summary_statuses(
        &mut status_by_spec,
        &all_requirement_summaries(project_root),
    );

    let live = process_probe::probe_live_claude_sessions();
    let now = chrono::Utc::now();
    let leases = list_leases(project_root);
    let live_leases: Vec<SessionLease> = leases
        .iter()
        .filter(|l| matches!(lease_state_for(l, &live, now), LeaseState::Live))
        .cloned()
        .collect();
    let live_scopes: std::collections::HashSet<String> = live_leases
        .iter()
        .map(|l| l.scope.to_ascii_uppercase())
        .collect();
    let live_branches: std::collections::HashSet<String> =
        live_leases.iter().map(|l| l.branch.clone()).collect();

    let mut seen = std::collections::HashSet::new();
    let local: std::collections::HashSet<String> =
        list_local_branches(project_root).into_iter().collect();
    let remote = collect_remote_branch_name_set(project_root);
    // BUG-1288: only run the expensive commit/patch-equivalence probes for
    // branches attributable to active requirement work or a recorded session.
    // Listing refs remains two batched git calls, but stale unrelated refs no
    // longer cause one or more subprocesses each. A requirement branch may
    // carry a suffix (`bug-1288-work`), so prefix matching deliberately keeps
    // those session-created variants while excluding another numeric id.
    //
    // PR #1999 rework: this was originally an ALLOW-list of only
    // "inprogress"/"in-progress"/"done", which silently dropped a lease-less
    // branch whose spec sat in NeedsAttention (shelved-but-not-abandoned),
    // Approved, Draft, or Planned — each a status a branch can legitimately
    // carry real unshipped commits under, before the commit/patch probe ever
    // ran. The bound this exists for is "don't probe every stale ref in the
    // repo" (301+ of them), not "only probe two statuses" — so the gate is
    // an EXCLUDE-list of the terminal statuses instead: everything that
    // isn't Completed/Rejected/Superseded is still eligible for the probe.
    // The exclusion set intentionally mirrors the terminal-status check
    // applied to the resolved branch spec_id further below in this
    // function, so a branch is never filtered here for a reason that
    // wouldn't also filter it there. trace:BUG-1288 | ai:claude
    let active_prefixes: Vec<String> = status_by_spec
        .iter()
        .filter(|(_, status)| !matches!(status.as_str(), "completed" | "rejected" | "superseded"))
        .map(|(spec, _)| spec.to_ascii_lowercase())
        .collect();
    let lease_branches: std::collections::HashSet<&str> =
        leases.iter().map(|lease| lease.branch.as_str()).collect();
    let is_candidate = |branch: &str| {
        lease_branches.contains(branch) || branch_belongs_to_active_work(branch, &active_prefixes)
    };
    let mut branches: Vec<(String, String, bool)> = local
        .iter()
        .filter(|branch| is_candidate(branch))
        .map(|b| (b.clone(), b.clone(), true))
        .chain(
            remote
                .iter()
                .filter(|b| !local.contains(*b) && is_candidate(b))
                .map(|b| (format!("origin/{b}"), format!("origin/{b}"), false)),
        )
        .collect();
    // BUG-1756: probe NEWEST tip first (name as the deterministic
    // tie-break), not alphabetically. The scan is wall-clock-bounded and a
    // single anciently-forked ref can cost tens of seconds in `git cherry`
    // (patch-id over every default-branch commit since the fork), so
    // whatever the budget cannot cover must be the OLD tail — never the
    // day-old branch a dead drain just stranded, which is the state this
    // channel exists to report. Tip times come from one batched
    // `for-each-ref`; a branch missing from it sorts last.
    // trace:BUG-1756 | ai:claude
    let tip_times = collect_candidate_tip_times(project_root);
    branches.sort_by(|a, b| {
        let ta = tip_times.get(&a.0).copied().unwrap_or(i64::MIN);
        let tb = tip_times.get(&b.0).copied().unwrap_or(i64::MIN);
        tb.cmp(&ta).then_with(|| a.0.cmp(&b.0))
    });

    // trace:BUG-1576 | ai:codex
    // BUG-1756: the snapshot starts BATCHED-ONLY — one `gh pr list --state
    // open` call regardless of candidate count. The per-candidate `--head`
    // history queries that used to run here up front moved into the evidence
    // pass below and run only for candidates that survive the cheap local
    // git classification. On a working repository the up-front per-candidate
    // network round trips consumed the entire wall-clock budget before a
    // single branch was locally probed, so the bounded scan always truncated
    // at 0/N and the channel reported nothing at all — which is exactly how
    // a local-only committed branch (the state that exists on one machine
    // and nowhere else) stayed invisible. Local evidence is cheap; it goes
    // first. trace:BUG-1756 | ai:claude
    let mut pr_head_states = if no_forge {
        None
    } else {
        collect_pr_head_state_snapshot_bounded(project_root, &[], deadline)
    };

    // BUG-1288: the total candidate-branch population identified above,
    // before the deadline can truncate how much of it actually gets probed
    // below. This is the denominator `UnshippedScanStatus::candidates`
    // reports, so "scan incomplete" always names how much was left unscanned
    // rather than just how much was scanned. trace:BUG-1288 | ai:claude
    let total_candidates = branches.len();
    let mut scanned = 0usize;
    let mut truncated = false;

    // BUG-1756: the most a single candidate's local diff+cherry probe may
    // spend when the scan is deadline-bounded. ~10× the loaded-host cost of
    // a normal candidate, so it only trips on genuine monsters (an
    // anciently-forked ref whose `git cherry` patch-ids thousands of
    // default-branch commits). trace:BUG-1756 | ai:claude
    const UNSHIPPED_PROBE_SLICE: std::time::Duration = std::time::Duration::from_millis(2000);

    // BUG-1756: resolve the patch-count base ref ONCE for the whole scan
    // instead of once per candidate inside
    // `branch_unshipped_patch_count_default` — the per-branch resolution was
    // 1–2 extra git subprocesses per candidate charged against the same
    // wall-clock budget. Resolution failure keeps the exact pre-existing
    // behavior: the candidate is skipped. trace:BUG-1756 | ai:claude
    let patch_count_base = resolve_default_branch_ref(project_root);

    // ── Pass 1: LOCAL classification — git only, no network ──────────────
    // Everything that can disqualify a candidate without per-branch forge
    // calls runs first, in the pre-existing predicate order: the batched
    // open-PR evidence, spec resolution, live leases, terminal status, and
    // the commit/patch-equivalence probe. Only the survivors — typically
    // zero to a handful — pay a per-head network query in the evidence pass
    // below, so the wall-clock budget is spent on work the local evidence
    // already says matters. trace:BUG-1756 | ai:claude
    let mut survivors: Vec<UnshippedSurvivor> = Vec::new();
    // A locally shipped ref can carry the merged-PR proof needed to suppress
    // a stale same-spec ancestor. Keep it for pass 2, but only query it when
    // that spec also has a survivor. trace:TASK-1575 | ai:codex
    let mut shipped_evidence_carriers: Vec<(String, String)> = Vec::new();
    for (display_branch, refname, has_local) in branches {
        if deadline.is_some_and(|dl| std::time::Instant::now() >= dl) {
            truncated = true;
            break;
        }
        scanned += 1;
        let short_branch = display_branch
            .strip_prefix("origin/")
            .unwrap_or(display_branch.as_str())
            .to_string();
        if matches!(
            short_branch.as_str(),
            "main" | "master" | "aida-store" | "HEAD"
        ) {
            continue;
        }
        // BUG-1531 criterion 1: a branch fetched from refs/pull/N/head (or its
        // GitLab mr-N twin) is the one shape `ReviewForge::local_branch_for`
        // creates, per TASK-1312's `parse_review_snapshot_branch` — but the
        // NAME alone is not proof (a real unpushed feature branch can happen
        // to be named `pr-123`). The NAME detection happens here; the forge
        // CONFIRMATION (`change_metadata`, a network call) happens in the
        // bounded evidence pass below. Only a confirmed match (head sha
        // equals this branch's tip) is excluded; anything less is kept and
        // labelled unverified rather than hidden on name alone (PRIN-5).
        // trace:BUG-1531 | ai:claude
        let review_shape = pr_cmd::parse_review_snapshot_branch(&short_branch);
        let pr_evidence = pr_head_states
            .as_ref()
            .and_then(|s| s.by_branch.get(&short_branch));
        if pr_evidence.is_some_and(|pr| pr.state == "open") {
            continue;
        }
        let Some(spec_id) = work_spec_id_from_branch(&short_branch)
            .or_else(|| first_spec_id_in_branch_commits(project_root, &default_ref, &refname))
        else {
            continue;
        };
        let spec_key = spec_id.to_ascii_uppercase();
        // A unique open PR's headRefName is authoritative for its spec. Never
        // suggest shipping an abandoned local branch as a duplicate PR.
        if pr_head_states
            .as_ref()
            .and_then(|s| s.open_heads_by_spec.get(&spec_key))
            .is_some_and(|heads| heads.len() == 1 && heads[0] != short_branch)
        {
            continue;
        }
        if live_scopes.contains(&spec_key) || live_branches.contains(&short_branch) {
            continue;
        }
        if status_by_spec
            .get(&spec_key)
            .map(|status| matches!(status.as_str(), "completed" | "rejected" | "superseded"))
            .unwrap_or(false)
        {
            continue;
        }
        // BUG-1756: one candidate gets at most UNSHIPPED_PROBE_SLICE (and
        // never more than the remaining budget) for its diff+cherry probe. A
        // candidate whose probe times out was never classified: it counts as
        // unscanned and marks the scan incomplete, instead of either eating
        // the whole budget (starving every candidate after it) or being
        // silently dispositioned as "nothing". trace:BUG-1756 | ai:claude
        let probe_slice = deadline.map(|dl| {
            dl.saturating_duration_since(std::time::Instant::now())
                .min(UNSHIPPED_PROBE_SLICE)
        });
        let probe = match patch_count_base.as_deref() {
            Some(base) => {
                branch_unshipped_patch_count_probe(project_root, base, &refname, probe_slice)
            }
            None => PatchCountProbe::NoSignal,
        };
        let commits_ahead = match probe {
            PatchCountProbe::Counted(n) if n > 0 => n,
            PatchCountProbe::Counted(0) => {
                shipped_evidence_carriers.push((spec_key, short_branch));
                continue;
            }
            PatchCountProbe::TimedOut => {
                scanned -= 1;
                truncated = true;
                continue;
            }
            PatchCountProbe::Counted(_) | PatchCountProbe::NoSignal => continue,
        };
        if !seen.insert(display_branch.clone()) {
            continue;
        }
        let tip_sha = git_output_checked(project_root, &["rev-parse", &refname])
            .ok()
            .map(|s| s.trim().to_string());
        // BUG-1756: a local branch with no `origin/<name>` counterpart exists
        // on exactly one machine. (A remote-only row is trivially pushed.)
        // Deliberately the EXISTENCE of the remote ref, not tip equality: a
        // stale-pushed branch still has a durable copy of most of its work,
        // and `aida pr ship` pushes the remainder. trace:BUG-1756 | ai:claude
        let pushed = !has_local || remote.contains(&short_branch);
        let age = branch_tip_age(project_root, &refname);
        survivors.push(UnshippedSurvivor {
            cand: UnshippedBranchCandidate {
                branch: display_branch,
                refname,
                local_branch: short_branch,
                spec_id,
                commits_ahead,
                age,
                has_local,
                tip_sha,
                possible_review_snapshot: false,
                pushed,
            },
            review_shape,
            confirmed_review_snapshot: false,
        });
    }

    // ── Pass 2: bounded per-survivor forge evidence ───────────────────────
    // BUG-1576's targeted `--head` history lookup and BUG-1531's review-
    // snapshot confirmation, one survivor at a time, still under the same
    // deadline. A survivor the budget cannot cover is counted as UNSCANNED
    // and dropped from the report — truncation stays a lower bound (PRIN-5),
    // never a row built on evidence that was never fetched.
    // trace:BUG-1756 | ai:claude
    // BUG-1756: evidence the LOCAL-ONLY survivors first (stable sort keeps
    // the newest-first order within each group). A pushed survivor the
    // budget cannot cover is durable and findable from any machine; a
    // local-only one is the state this channel exists to report, so
    // truncation must never be able to hide it behind pushed rows.
    // trace:BUG-1756 | ai:claude
    survivors.sort_by_key(|s| s.cand.pushed);
    let mut evidenced = survivors.len();
    if no_forge {
        for survivor in survivors.iter_mut() {
            if survivor.review_shape.is_some() {
                survivor.cand.possible_review_snapshot = true;
            }
        }
    } else {
        for (idx, survivor) in survivors.iter_mut().enumerate() {
            if deadline.is_some_and(|dl| std::time::Instant::now() >= dl) {
                truncated = true;
                evidenced = idx;
                break;
            }
            if let Some(states) = pr_head_states.as_mut() {
                states
                    .queried_heads
                    .insert(survivor.cand.local_branch.clone());
                if let Some(found) =
                    query_pr_head_history(project_root, &survivor.cand.local_branch)
                {
                    states.merge(found);
                }
            }
            if let Some((kind, n)) = survivor.review_shape {
                match forge::forge_for_kind(project_root, kind)
                    .change_metadata(n, &mut network_retry::NoopSink)
                {
                    Ok(meta) if !meta.head_sha.is_empty() => {
                        if survivor.cand.tip_sha.as_deref() == Some(meta.head_sha.as_str()) {
                            survivor.confirmed_review_snapshot = true;
                        } else {
                            survivor.cand.possible_review_snapshot = true;
                        }
                    }
                    _ => {
                        survivor.cand.possible_review_snapshot = true;
                    }
                }
            }
        }
        // Apply pass-2 truncation before carrier filtering. `evidenced` is a
        // prefix length; filtering first could shift a never-evidenced row
        // into that prefix and report it without forge evidence. trace:TASK-1575 | ai:codex
        if evidenced < survivors.len() {
            scanned -= survivors.len() - evidenced;
            survivors.truncate(evidenced);
        }
        // Query excluded same-spec carriers only after survivor queries. This
        // preserves lazy evidence while restoring merged-head representation
        // proof for the ancestor branch case. If the deadline prevents a
        // needed carrier query, suppress that spec's survivors rather than
        // report on incomplete representation evidence. trace:TASK-1575 | ai:codex
        let survivor_specs: std::collections::HashSet<String> = survivors
            .iter()
            .map(|s| s.cand.spec_id.to_ascii_uppercase())
            .collect();
        let mut incomplete_carrier_specs = std::collections::HashSet::new();
        for (spec_key, carrier) in shipped_evidence_carriers {
            if !survivor_specs.contains(&spec_key) {
                continue;
            }
            if deadline.is_some_and(|dl| std::time::Instant::now() >= dl) {
                truncated = true;
                incomplete_carrier_specs.insert(spec_key);
                continue;
            }
            if let Some(states) = pr_head_states.as_mut() {
                states.queried_heads.insert(carrier.clone());
                if let Some(found) = query_pr_head_history(project_root, &carrier) {
                    states.merge(found);
                } else {
                    truncated = true;
                    incomplete_carrier_specs.insert(spec_key);
                }
            } else {
                truncated = true;
                incomplete_carrier_specs.insert(spec_key);
            }
        }
        if !incomplete_carrier_specs.is_empty() {
            let before = survivors.len();
            survivors.retain(|s| {
                !incomplete_carrier_specs.contains(&s.cand.spec_id.to_ascii_uppercase())
            });
            let removed = before - survivors.len();
            scanned = scanned.saturating_sub(removed);
        }
    }
    // ── Final classification against the completed evidence ──────────────
    // The forge-dependent exclusions, in their pre-existing order, applied
    // once every surviving candidate's history query has been merged — so a
    // merged head discovered through one survivor can still represent (and
    // exclude) another survivor of the same spec, exactly as when every
    // candidate was queried up front. trace:BUG-1756 | ai:claude
    let mut candidates = Vec::new();
    for survivor in survivors {
        let UnshippedSurvivor {
            cand: c,
            confirmed_review_snapshot,
            ..
        } = survivor;
        // trace:BUG-1531 | ai:claude — the one exclusion a review-snapshot
        // NAME may earn: the forge confirmed the PR/MR and its head sha
        // equals this branch's tip.
        if confirmed_review_snapshot {
            continue;
        }
        let pr_evidence = pr_head_states
            .as_ref()
            .and_then(|s| s.by_branch.get(&c.local_branch));
        // Re-applied with the full evidence: a per-head history query above
        // can surface an open PR the batched open-PR snapshot did not.
        if pr_evidence.is_some_and(|pr| pr.state == "open") {
            continue;
        }
        let spec_key = c.spec_id.to_ascii_uppercase();
        if pr_head_states
            .as_ref()
            .and_then(|s| s.open_heads_by_spec.get(&spec_key))
            .is_some_and(|heads| heads.len() == 1 && heads[0] != c.local_branch)
        {
            continue;
        }
        if pr_evidence.is_some_and(|pr| {
            if pr.state != "merged" {
                return false;
            }
            let tip_matches = pr
                .head_sha
                .as_ref()
                .is_some_and(|sha| c.tip_sha.as_deref() == Some(sha.as_str()));
            tip_matches
                || doctor_cmd::branch_content_fully_landed(project_root, &default_ref, &c.refname)
        }) {
            continue;
        }
        // A rework ref may have a different name from the PR head while
        // pointing at that reviewed head (or one of its ancestors). This is
        // commit-representation proof, unlike branch age: a divergent branch
        // for the same spec is not an ancestor and remains visible.
        let represented_by_merged_head = pr_head_states
            .as_ref()
            .and_then(|s| s.merged_heads_by_spec.get(&spec_key))
            .is_some_and(|merged_heads| {
                merged_heads.iter().any(|head| {
                    let is_ancestor = std::process::Command::new("git")
                        .arg("-C")
                        .arg(project_root)
                        .args([
                            "merge-base",
                            "--is-ancestor",
                            git_arg_guard::END_OF_OPTIONS,
                            &c.refname,
                            head,
                        ]) // trace:BUG-1622 | ai:claude
                        .status()
                        .is_ok_and(|status| status.success());
                    is_ancestor
                        || doctor_cmd::branch_content_fully_landed(project_root, head, &c.refname)
                })
            });
        if represented_by_merged_head {
            continue;
        }
        // BUG-1531 criterion 2 (the safety floor, independent of criterion 1):
        // never suggest shipping a commit that is ALREADY the head of an open
        // PR, even when this branch's own name doesn't match that PR's
        // headRefName (a rework ref, a hand-fetched investigation branch,
        // …). Matched by sha across every open PR the snapshot knows about,
        // not just the by-name lookup above.
        // trace:BUG-1531 | ai:claude
        if let (Some(sha), Some(states)) = (c.tip_sha.as_deref(), pr_head_states.as_ref()) {
            let already_open_elsewhere = states.by_branch.values().any(|evidence| {
                evidence.state == "open" && evidence.head_sha.as_deref() == Some(sha)
            });
            if already_open_elsewhere {
                continue;
            }
        }
        candidates.push(c);
    }

    candidates.sort_by(|a, b| b.commits_ahead.cmp(&a.commits_ahead));
    let items: Vec<awaiting_you::UnshippedWorkItem> = candidates
        .into_iter()
        .map(|c| {
            let first_seen = if emit_detected_events {
                record_unshipped_work_detected(project_root, &c.spec_id, &c.branch)
            } else {
                String::new()
            };
            let pr_state = if no_forge || pr_head_states.is_none() {
                "unknown".to_string()
            } else {
                let states = pr_head_states.as_ref().expect("checked above");
                match states
                    .by_branch
                    .get(&c.local_branch)
                    .map(|pr| pr.state.as_str())
                {
                    Some("open") => "open",
                    Some("merged") => "merged",
                    // BUG-1288: only report "absent" (no PR ever existed for
                    // this head) when the per-branch query actually ran. A
                    // branch the deadline skipped was never asked, so it must
                    // read "unknown" rather than a false "absent" — PRIN-5.
                    // trace:BUG-1288 | ai:claude
                    _ if states.queried_heads.contains(&c.local_branch) => "absent",
                    _ => "unknown",
                }
                .to_string()
            };
            // BUG-1531 criterion 3 (the general form of the hole, independent
            // of both branch shape and PR number): a refusal binds to a
            // COMMIT. If this branch's tip is the exact sha an outstanding
            // (unclosed, not superseded by a Completed spec) RequestChanges
            // or Rejected verdict names, no surface may recommend shipping
            // it — regardless of which ref happens to reach that commit.
            // trace:BUG-1531 | ai:claude
            let outstanding_verdict = c.tip_sha.as_deref().and_then(|sha| {
                let verdict = review_verdict::read_recorded_verdict(project_root, &c.spec_id)?;
                let sha_matches = verdict
                    .reviewed_sha
                    .as_deref()
                    .is_some_and(|reviewed| review_verdict::same_reviewed_sha(reviewed, sha));
                let spec_completed = status_by_spec
                    .get(&c.spec_id.to_ascii_uppercase())
                    .is_some_and(|status| status.as_str() == "completed");
                (sha_matches && review_verdict::is_outstanding_refusal(&verdict, spec_completed))
                    .then_some(verdict)
            });
            let recovery = if let Some(verdict) = &outstanding_verdict {
                format!(
                    "do not ship — this commit carries an unresolved reviewer verdict ({}); resolve the review first",
                    verdict.raw
                )
            } else if c.possible_review_snapshot {
                // BUG-1531: name-shaped like a review snapshot but the forge
                // never confirmed it — kept visible, no ship hint (PRIN-5).
                format!(
                    "possible review snapshot ({}), unverified",
                    c.local_branch
                )
            } else if c.has_local && !c.pushed {
                // BUG-1756 AC3: a branch with NO remote ref must be pushed
                // before anything can ship it — the hint names the push
                // first, never a bare `aida pr ship` against a ref the forge
                // cannot see. trace:BUG-1756 | ai:claude
                format!(
                    "git push -u origin {b} && aida pr ship {b}",
                    b = &c.local_branch
                )
            } else if c.has_local {
                format!("aida pr ship {}", c.branch)
            } else {
                format!(
                    "git switch -c {} {} && aida pr ship {}",
                    &c.local_branch, c.refname, &c.local_branch
                )
            };
            let age = if first_seen.is_empty() {
                c.age
            } else {
                format!("{} (first seen {})", c.age, first_seen)
            };
            awaiting_you::UnshippedWorkItem {
                spec_id: c.spec_id,
                branch: c.branch,
                commits_ahead: c.commits_ahead,
                age,
                recovery,
                pr_state,
                pushed: c.pushed, // trace:BUG-1756 | ai:claude
            }
        })
        .collect();
    (
        items,
        awaiting_you::UnshippedScanStatus {
            complete: !truncated,
            scanned,
            candidates: total_candidates,
        },
    )
}

// trace:STORY-1043 | ai:codex
pub(crate) fn first_spec_id_in_branch_commits(
    project_root: &std::path::Path,
    default_ref: &str,
    refname: &str,
) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "log",
            "--format=%s",
            "-n",
            "25",
            &format!("{default_ref}..{refname}"),
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| extract_spec_ids_from_commit(line).into_iter().next())
}

// trace:STORY-1043 | ai:codex
pub(crate) fn branch_tip_age(project_root: &std::path::Path, refname: &str) -> String {
    let ts = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["log", "-1", "--format=%ct", refname])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<i64>()
                .ok()
        });
    let Some(ts) = ts else {
        return "unknown".to_string();
    };
    let age = chrono::Utc::now().timestamp().saturating_sub(ts);
    if age < 3600 {
        format!("{}m", (age / 60).max(1))
    } else if age < 86400 {
        format!("{}h", age / 3600)
    } else {
        format!("{}d", age / 86400)
    }
}

// trace:STORY-1043 | ai:codex
pub(crate) fn record_unshipped_work_detected(
    project_root: &std::path::Path,
    spec_id: &str,
    branch: &str,
) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    branch.hash(&mut hasher);
    let safe: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = format!("{safe}-{:016x}", hasher.finish());
    let path = project_root
        .join(".aida")
        .join("unshipped-work-seen")
        .join(safe);
    if let Ok(prev) = std::fs::read_to_string(&path) {
        return prev.trim().to_string();
    }
    let first_seen = chrono::Utc::now().to_rfc3339();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::write(&path, &first_seen).is_ok() {
        events::emit(
            project_root,
            &events::Event::new(
                Some(spec_id.to_string()),
                "",
                events::EventKind::UnshippedWorkDetected {
                    spec: spec_id.to_string(),
                    branch: branch.to_string(),
                    first_seen: first_seen.clone(),
                },
            ),
        );
    }
    first_seen
}

// trace:STORY-1043 | ai:codex
#[cfg(test)]
mod story_1043_unshipped_work_tests {
    use super::*;

    fn git(root: &std::path::Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn commit_file(root: &std::path::Path, path: &str, body: &str, subject: &str) {
        let full = root.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, body).unwrap();
        git(root, &["add", path]);
        git(root, &["commit", "-m", subject]);
    }

    fn branch_with_commit(root: &std::path::Path, branch: &str, spec: &str) {
        git(root, &["checkout", "-b", branch, "main"]);
        commit_file(
            root,
            &format!("{branch}.txt"),
            branch,
            &format!("[AI:codex] feat: branch work ({spec})"),
        );
        git(root, &["checkout", "main"]);
    }

    fn init_repo(root: &std::path::Path) {
        git(root, &["init"]);
        git(root, &["checkout", "-b", "main"]);
        git(root, &["config", "user.email", "codex@example.test"]);
        git(root, &["config", "user.name", "Codex"]);
        git(
            root,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/example/aida-fixture.git",
            ],
        );
        commit_file(root, "README.md", "fixture", "chore: init");
    }

    fn executable_fake_gh(root: &std::path::Path, body: &str) -> std::path::PathBuf {
        let fake_gh = root.join("fake-gh");
        crate::test_exec::write_executable(&fake_gh, body);
        fake_gh
    }

    fn summary(spec_id: &str, status: &str) -> aida_core::RequirementSummary {
        aida_core::RequirementSummary {
            id: uuid::Uuid::new_v4(),
            spec_id: Some(spec_id.to_string()),
            agreed_id: Some(spec_id.to_string()),
            title: format!("{spec_id} title"),
            description: String::new(),
            status: status.to_string(),
            priority: "high".to_string(),
            owner: String::new(),
            assignee: None,
            feature: String::new(),
            req_type: "Story".to_string(),
            tags: Vec::new(),
            created_at: String::new(),
            modified_at: chrono::Utc::now().to_rfc3339(),
            archived: false,
            archived_at: None,
            deferred: false,
            deferred_at: None,
            deferred_until: None,
            deferred_reason: None,
            in_degree: 0,
            out_degree: 0,
            heft: 0,
            blocked: false,
            has_pending_decision: false,
            execution_mode: None,
            weight: None,
            origin: None,
            completed_at: None, // trace:TASK-1474 | ai:claude
            yaml_path: String::new(),
        }
    }

    // BUG-1288: stale refs must not enlarge the expensive probe set. This is
    // a deterministic operation-count regression test rather than a flaky
    // wall-clock assertion. trace:BUG-1288 | ai:codex
    #[test]
    fn unshipped_candidate_gate_is_constant_with_stale_branch_count() {
        let active = vec!["bug-1288".to_string(), "story-42".to_string()];
        let mut refs: Vec<String> = (0..10_000).map(|n| format!("old-feature-{n}")).collect();
        refs.extend([
            "bug-1288".to_string(),
            "bug-1288-work".to_string(),
            "story-42-retry".to_string(),
            "story-420".to_string(),
        ]);

        let candidates: Vec<_> = refs
            .iter()
            .filter(|branch| branch_belongs_to_active_work(branch, &active))
            .cloned()
            .collect();
        assert_eq!(
            candidates,
            ["bug-1288", "bug-1288-work", "story-42-retry"],
            "only active spec branches reach per-branch git probes"
        );
    }

    fn write_live_lease(root: &std::path::Path, spec: &str, branch: &str) {
        let lease = SessionLease {
            id: "live1043".to_string(),
            scope: spec.to_string(),
            slug: slugify(spec),
            owner: "codex@example.test".to_string(),
            worktree_path: root.canonicalize().unwrap(),
            branch: branch.to_string(),
            started_at: chrono::Utc::now(),
            hostname: hostname(),
            role: Some("implementer".to_string()),
            creator_pid: None,
            creator_pid_start_time: None,
            active_pid: Some(std::process::id()),
            active_pid_start_time: process_probe::process_start_identity(std::process::id()),
            cargo_target_dir: None,
            parent_project_root: Some(root.to_path_buf()),
            pr_head_sha: None,
            pr_base_sha: None,
            pr_base_ref: None,
            zen_intent_token: None,
            escalated_to_human: None,
            parent_branch: None,
            parent_branch_sha: None,
            review_verb: false,
            claim_verb: false,
            manual_enter_at: None,
            interrupted_at: None,
            interrupted_reason: None,
            driver: None,
        };
        std::fs::create_dir_all(leases_dir(root)).unwrap();
        std::fs::write(
            lease_path(root, "live1043"),
            toml::to_string_pretty(&lease).unwrap(),
        )
        .unwrap();
    }

    // The fake `gh` used here is a `#!/usr/bin/env bash` script stubbed via
    // AIDA_TEST_GH_BINARY; Windows cannot execute it through a shebang, so the
    // open-PR branch is misread as unshipped. The detector itself is
    // platform-agnostic (real gh.exe works); only this harness is Unix-only.
    // trace:STORY-1043 | ai:claude
    #[cfg_attr(
        windows,
        ignore = "fake-gh harness is a bash script; gh cannot be stubbed via shebang on Windows"
    )]
    #[test]
    fn detector_lists_unshipped_work_and_skips_live_terminal_and_open_pr_branches() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        branch_with_commit(root, "story-1043-unshipped", "STORY-1043");
        branch_with_commit(root, "story-1044-live", "STORY-1044");
        branch_with_commit(root, "story-1045-done", "STORY-1045");
        branch_with_commit(root, "story-1046-open-pr", "STORY-1046");
        branch_with_commit(root, "story-1047-remote", "STORY-1047");
        git(
            root,
            &[
                "update-ref",
                "refs/remotes/origin/story-1047-remote",
                "refs/heads/story-1047-remote",
            ],
        );
        git(root, &["branch", "-D", "story-1047-remote"]);
        write_live_lease(root, "STORY-1044", "story-1044-live");

        let fake_gh = executable_fake_gh(
            root,
            r#"#!/usr/bin/env bash
if [[ "$*" == *"pr list"* ]]; then
  printf '[{"number":46,"title":"open","headRefName":"story-1046-open-pr","state":"OPEN","statusCheckRollup":[],"mergeable":"MERGEABLE","reviewDecision":""}]'
  exit 0
fi
exit 1
"#,
        );

        let _env = crate::test_env::EnvVarsGuard::set(&[(
            "AIDA_TEST_GH_BINARY",
            fake_gh.to_str().unwrap(),
        )]);
        let rows = collect_unshipped_work_items(
            root,
            &[
                summary("STORY-1043", "InProgress"),
                summary("STORY-1044", "InProgress"),
                summary("STORY-1045", "Completed"),
                summary("STORY-1046", "InProgress"),
                summary("STORY-1047", "InProgress"),
            ],
            false,
            false,
        );

        assert_eq!(rows.len(), 2);
        let local = rows
            .iter()
            .find(|row| row.branch == "story-1043-unshipped")
            .unwrap();
        assert_eq!(local.spec_id, "STORY-1043");
        assert_eq!(local.commits_ahead, 1);
        assert_eq!(local.pr_state, "absent");
        // BUG-1756: this fixture branch was never pushed (no
        // refs/remotes/origin counterpart), so it is the local-only state and
        // its hint must name the push first. trace:BUG-1756 | ai:claude
        assert!(!local.pushed);
        assert_eq!(
            local.recovery,
            "git push -u origin story-1043-unshipped && aida pr ship story-1043-unshipped"
        );

        let remote = rows
            .iter()
            .find(|row| row.branch == "origin/story-1047-remote")
            .unwrap();
        assert_eq!(remote.spec_id, "STORY-1047");
        assert_eq!(remote.commits_ahead, 1);
        assert!(remote.pushed); // trace:BUG-1756 | ai:claude
        assert_ne!(remote.age, "unknown");
        assert_eq!(
            remote.recovery,
            "git switch -c story-1047-remote origin/story-1047-remote && aida pr ship story-1047-remote"
        );
    }

    // BUG-1756 AC1 + AC3 + AC4: a LOCAL-ONLY committed branch (ahead of
    // main, ≥1 commit, no PR, never pushed) is reported as unshipped work,
    // with its state distinguished from a pushed-no-PR branch — the two need
    // different next actions (push then open a PR vs open a PR), and the
    // local-only hint must name the push first: a reader must never be told
    // to `aida pr ship` a ref the forge cannot see. The pushed branch is the
    // AC4 control — its classification must stay byte-identical to what the
    // detector reported before this fix. trace:BUG-1756 | ai:claude
    #[cfg_attr(
        windows,
        ignore = "fake-gh harness is a bash script; gh cannot be stubbed via shebang on Windows"
    )]
    #[test]
    fn detector_distinguishes_local_only_from_pushed_no_pr_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        branch_with_commit(root, "bug-1756-localonly", "BUG-1756");
        branch_with_commit(root, "bug-1757-pushed", "BUG-1757");
        // Simulate a pushed branch: the remote-tracking ref exists alongside
        // the local one (what a real `git push -u origin` leaves behind).
        git(
            root,
            &[
                "update-ref",
                "refs/remotes/origin/bug-1757-pushed",
                "refs/heads/bug-1757-pushed",
            ],
        );

        let fake_gh = executable_fake_gh(
            root,
            r#"#!/usr/bin/env bash
printf '[]'
"#,
        );
        let _env = crate::test_env::EnvVarsGuard::set(&[(
            "AIDA_TEST_GH_BINARY",
            fake_gh.to_str().unwrap(),
        )]);
        let rows = collect_unshipped_work_items(
            root,
            &[
                summary("BUG-1756", "InProgress"),
                summary("BUG-1757", "InProgress"),
            ],
            false,
            false,
        );

        assert_eq!(
            rows.len(),
            2,
            "both unshipped branches must report: {rows:?}"
        );
        let local_only = rows
            .iter()
            .find(|row| row.branch == "bug-1756-localonly")
            .expect("the never-pushed branch is the state the detector most needs to name");
        assert!(!local_only.pushed);
        assert_eq!(local_only.pr_state, "absent");
        assert_eq!(
            local_only.recovery,
            "git push -u origin bug-1756-localonly && aida pr ship bug-1756-localonly"
        );

        let pushed = rows
            .iter()
            .find(|row| row.branch == "bug-1757-pushed")
            .expect("a pushed-no-PR branch keeps reporting exactly as before");
        assert!(pushed.pushed);
        assert_eq!(pushed.pr_state, "absent");
        assert_eq!(pushed.recovery, "aida pr ship bug-1757-pushed");
    }

    // BUG-1756 (the observed incident): the per-candidate `--head` network
    // round trips used to run UP FRONT for every candidate branch, which on
    // a working repository consumed the entire wall-clock budget before a
    // single branch was locally probed — `aida awaiting` reported
    // `unshipped_work[0]` / `scanned: 0` while a finished local-only commit
    // sat on exactly one machine. Deterministic operation-count pin (no
    // wall-clock flakiness, same style as the BUG-1288 gate test): with
    // three candidates of which only one survives the local classification,
    // exactly ONE `--head` history query is issued, and it names the
    // survivor. trace:BUG-1756 | ai:claude
    #[cfg_attr(
        windows,
        ignore = "fake-gh harness is a bash script; gh cannot be stubbed via shebang on Windows"
    )]
    #[test]
    fn per_head_history_queries_run_only_for_surviving_candidates() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        branch_with_commit(root, "bug-9101-survivor", "BUG-9101");
        // A candidate the LOCAL pass excludes: live lease on its branch.
        branch_with_commit(root, "bug-9102-live", "BUG-9102");
        write_live_lease(root, "BUG-9102", "bug-9102-live");
        // A candidate the LOCAL pass excludes: no content beyond main.
        git(root, &["branch", "bug-9103-empty", "main"]);

        let call_log = root.join("gh-calls.log");
        let fake_gh = executable_fake_gh(
            root,
            &format!(
                r#"#!/usr/bin/env bash
echo "$@" >> "{}"
printf '[]'
"#,
                call_log.display()
            ),
        );
        let _env = crate::test_env::EnvVarsGuard::set(&[(
            "AIDA_TEST_GH_BINARY",
            fake_gh.to_str().unwrap(),
        )]);
        let rows = collect_unshipped_work_items(
            root,
            &[
                summary("BUG-9101", "InProgress"),
                summary("BUG-9102", "InProgress"),
                summary("BUG-9103", "InProgress"),
            ],
            false,
            false,
        );

        assert_eq!(rows.len(), 1, "only the survivor reports: {rows:?}");
        assert_eq!(rows[0].spec_id, "BUG-9101");

        let calls = std::fs::read_to_string(&call_log).unwrap_or_default();
        let head_queries: Vec<&str> = calls.lines().filter(|l| l.contains("--head")).collect();
        assert_eq!(
            head_queries.len(),
            1,
            "a per-head network query is paid only for candidates that survive \
             the local classification, never per candidate up front: {calls}"
        );
        assert!(
            head_queries[0].contains("bug-9101-survivor"),
            "the one --head query names the survivor: {calls}"
        );
    }

    // The merged PR can be on a sibling ref that pass 1 classifies as already
    // shipped (tree-identical to main), while the old ancestor still survives
    // git cherry. Its merged-head evidence must be fetched lazily for both
    // detector entry points. trace:TASK-1575 | ai:codex
    #[cfg_attr(windows, ignore = "fake-gh harness requires a Unix shebang")]
    #[test]
    fn shipped_sibling_carrier_represents_stale_ancestor_in_both_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        git(root, &["checkout", "-b", "task-1575"]);
        commit_file(
            root,
            "ancestor.txt",
            "ancestor\n",
            "fix: ancestor (TASK-1575)",
        );
        let ancestor = git_output_checked(root, &["rev-parse", "HEAD"]).unwrap();
        git(root, &["checkout", "-b", "task-1575-merged"]);
        commit_file(root, "carrier.txt", "carrier\n", "fix: carrier (TASK-1575)");
        let carrier = git_output_checked(root, &["rev-parse", "HEAD"]).unwrap();
        git(root, &["checkout", "main"]);
        std::fs::write(root.join("ancestor.txt"), "ancestor\n").unwrap();
        std::fs::write(root.join("carrier.txt"), "carrier\n").unwrap();
        git(root, &["add", "ancestor.txt", "carrier.txt"]);
        git(root, &["commit", "-m", "fix: squash (TASK-1575)"]);
        assert_eq!(
            git_output_checked(root, &["rev-parse", "main^{tree}"]).unwrap(),
            git_output_checked(root, &["rev-parse", "task-1575-merged^{tree}"]).unwrap()
        );

        let calls = root.join("gh-calls");
        let fake = executable_fake_gh(
            root,
            &format!(
                r#"#!/usr/bin/env bash
printf '%s\\n' "$*" >> '{}'
if [[ "$*" == *"--state open"* ]]; then printf '[]'; exit 0; fi
if [[ "$*" == *"--head task-1575-merged"* ]]; then
  printf '[{{"state":"MERGED","title":"fix (TASK-1575)","headRefName":"task-1575-merged","headRefOid":"{}"}}]'
else
  printf '[]'
fi
"#,
                calls.display(),
                carrier.trim()
            ),
        );
        let _env =
            crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake.to_str().unwrap())]);
        let summaries = [summary("TASK-1575", "InProgress")];

        let rows = collect_unshipped_work_items(root, &summaries, false, false);
        assert!(
            rows.iter().all(|row| row.branch != "task-1575"),
            "ancestor is represented: {rows:?}; calls: {}",
            std::fs::read_to_string(&calls).unwrap_or_default()
        );

        let (rows, scan) = collect_unshipped_work_items_bounded(
            root,
            &summaries,
            false,
            false,
            Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
        );
        assert!(scan.complete, "generous deadline should finish: {scan:?}");
        assert!(
            rows.iter().all(|row| row.branch != "task-1575"),
            "ancestor is represented: {rows:?}"
        );
        assert_eq!(ancestor.trim().len(), 40);
        let calls = std::fs::read_to_string(calls).unwrap();
        assert!(
            calls.contains("--head task-1575-merged --state all"),
            "carrier history queried: {calls}"
        );
    }

    // When pass 2 truncates after an evidenced local-only survivor, an
    // incomplete carrier must not filter that survivor and shift a later,
    // never-evidenced pushed survivor into the count-based kept prefix.
    // trace:TASK-1575 | ai:codex
    #[cfg_attr(windows, ignore = "fake-gh harness requires a Unix shebang")]
    #[test]
    fn incomplete_carrier_cannot_promote_never_evidenced_survivor() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        git(root, &["checkout", "-b", "task-1575"]);
        commit_file(
            root,
            "ancestor.txt",
            "ancestor\n",
            "fix: ancestor (TASK-1575)",
        );
        git(root, &["checkout", "-b", "task-1575-carrier"]);
        commit_file(root, "carrier.txt", "carrier\n", "fix: carrier (TASK-1575)");
        let carrier = git_output_checked(root, &["rev-parse", "HEAD"]).unwrap();
        git(root, &["checkout", "main"]);
        std::fs::write(root.join("ancestor.txt"), "ancestor\n").unwrap();
        std::fs::write(root.join("carrier.txt"), "carrier\n").unwrap();
        git(root, &["add", "ancestor.txt", "carrier.txt"]);
        git(root, &["commit", "-m", "fix: squash (TASK-1575)"]);
        assert_eq!(
            git_output_checked(root, &["rev-parse", "main^{tree}"]).unwrap(),
            git_output_checked(root, &["rev-parse", "task-1575-carrier^{tree}"]).unwrap()
        );
        branch_with_commit(root, "task-1582-pushed", "TASK-1582");
        let pushed = git_output_checked(root, &["rev-parse", "task-1582-pushed"]).unwrap();
        git(
            root,
            &[
                "update-ref",
                "refs/remotes/origin/task-1582-pushed",
                pushed.trim(),
            ],
        );

        let calls = root.join("gh-calls");
        let fake = executable_fake_gh(
            root,
            &format!(
                r#"#!/usr/bin/env bash
printf '%s\\n' "$*" >> '{}'
if [[ "$*" == *"--state open"* ]]; then printf '[]'; exit 0; fi
if [[ "$*" == *"--head task-1575"* ]]; then sleep 2.5; fi
printf '[]'
"#,
                calls.display()
            ),
        );
        let _env =
            crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake.to_str().unwrap())]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let (rows, scan) = collect_unshipped_work_items_bounded(
            root,
            &[
                summary("TASK-1575", "InProgress"),
                summary("TASK-1582", "InProgress"),
            ],
            false,
            false,
            Some(deadline),
        );

        assert!(
            !scan.complete,
            "deadline and incomplete carrier evidence must be reported: {scan:?}"
        );
        assert!(
            rows.iter().all(|row| row.branch != "task-1582-pushed"),
            "the later survivor never received --head evidence and must not be reported: {rows:?}"
        );
        assert_eq!(carrier.trim().len(), 40);
        let calls = std::fs::read_to_string(calls).unwrap();
        assert!(
            calls.contains("--head task-1575 --state all"),
            "first survivor queried: {calls}"
        );
        assert!(
            !calls.contains("--head task-1582-pushed"),
            "later survivor skipped: {calls}"
        );
    }

    // Failure of the batched forge snapshot also makes carrier evidence
    // incomplete; dropping same-spec survivors must mark the scan incomplete.
    // trace:TASK-1575 | ai:codex
    #[cfg_attr(windows, ignore = "fake-gh harness requires a Unix shebang")]
    #[test]
    fn missing_forge_snapshot_marks_carrier_scan_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        git(root, &["checkout", "-b", "task-1575"]);
        commit_file(
            root,
            "ancestor.txt",
            "ancestor\n",
            "fix: ancestor (TASK-1575)",
        );
        git(root, &["checkout", "-b", "task-1575-carrier"]);
        commit_file(root, "carrier.txt", "carrier\n", "fix: carrier (TASK-1575)");
        git(root, &["checkout", "main"]);
        std::fs::write(root.join("ancestor.txt"), "ancestor\n").unwrap();
        std::fs::write(root.join("carrier.txt"), "carrier\n").unwrap();
        git(root, &["add", "ancestor.txt", "carrier.txt"]);
        git(root, &["commit", "-m", "fix: squash (TASK-1575)"]);

        let fake = executable_fake_gh(
            root,
            "#!/usr/bin/env bash\nif [[ \"$*\" == *\"--state open\"* ]]; then exit 1; fi\nprintf '[]'\n",
        );
        let _env =
            crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake.to_str().unwrap())]);
        let (rows, scan) = collect_unshipped_work_items_bounded(
            root,
            &[summary("TASK-1575", "InProgress")],
            false,
            false,
            Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
        );

        assert!(
            rows.is_empty(),
            "missing representation evidence suppresses the spec: {rows:?}"
        );
        assert!(
            !scan.complete,
            "a missing forge snapshot must make the scan honestly incomplete: {scan:?}"
        );
    }

    // BUG-1756: a single anciently-forked candidate can cost `git cherry`
    // tens of seconds — several times the whole scan budget — and before the
    // per-candidate slice it starved every candidate after it (observed live:
    // the scan stuck at the same truncation point under 5s, 10s and 15s
    // budgets because one review-snapshot ref cost ~30s). The monster must
    // burn at most its slice, count as UNSCANNED (incomplete scan, PRIN-5),
    // and the normal candidates around it must still classify and report.
    // trace:BUG-1756 | ai:claude
    #[cfg_attr(
        windows,
        ignore = "fake-git harness is a bash script; git cannot be shimmed via shebang on Windows"
    )]
    #[test]
    fn one_expensive_candidate_cannot_starve_the_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        branch_with_commit(root, "bug-9105-monster", "BUG-9105");
        branch_with_commit(root, "bug-9106-normal", "BUG-9106");

        // A PATH-shim git that stalls ONLY the monster's `cherry` probe and
        // execs the real git for everything else.
        let real_git = std::env::var_os("PATH")
            .and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|dir| dir.join("git"))
                    .find(|p| p.is_file())
            })
            .expect("git on PATH");
        let real_git = real_git.display();
        let shim_dir = root.join("git-shim");
        std::fs::create_dir_all(&shim_dir).unwrap();
        crate::test_exec::write_executable(
            &shim_dir.join("git"),
            &format!(
                r#"#!/usr/bin/env bash
is_cherry=""
is_monster=""
for a in "$@"; do
  [ "$a" = "cherry" ] && is_cherry=1
  [ "$a" = "bug-9105-monster" ] && is_monster=1
done
if [ -n "$is_cherry" ] && [ -n "$is_monster" ]; then
  sleep 30
fi
exec "{real_git}" "$@"
"#
            ),
        );
        let path = format!(
            "{}:{}",
            shim_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let _env = crate::test_env::EnvVarsGuard::set(&[("PATH", path.as_str())]);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        let (rows, scan) = collect_unshipped_work_items_bounded(
            root,
            &[
                summary("BUG-9105", "InProgress"),
                summary("BUG-9106", "InProgress"),
            ],
            true, // no_forge — this pin is about the LOCAL pass
            false,
            Some(deadline),
        );

        assert!(
            rows.iter().any(|r| r.branch == "bug-9106-normal"),
            "a normal candidate must still classify and report next to the monster: {rows:?}"
        );
        assert!(
            rows.iter().all(|r| r.branch != "bug-9105-monster"),
            "the timed-out candidate was never classified and must not be reported: {rows:?}"
        );
        assert!(
            !scan.complete,
            "a probe-capped candidate means the scan did NOT cover everything"
        );
        assert_eq!(
            scan.scanned,
            scan.candidates - 1,
            "exactly the monster counts as unscanned: {scan:?}"
        );
    }

    // BUG-1288: `collect_unshipped_work_items_bounded`'s deadline must be
    // honest about a truncated scan, never collapse it into an empty/zero
    // result. An already-elapsed deadline is the deterministic way to pin
    // this — no wall-clock race, no flakiness — and stands in for what a
    // slow, candidate-heavy repo does to the real 2s production budget.
    // trace:BUG-1288 | ai:claude
    #[test]
    fn detector_reports_an_incomplete_scan_honestly_instead_of_hiding_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        branch_with_commit(root, "story-9001-first", "STORY-9001");
        branch_with_commit(root, "story-9002-second", "STORY-9002");

        // Already in the past — the bounded scan must not probe a single
        // candidate branch.
        let deadline = std::time::Instant::now();
        let (rows, scan) = collect_unshipped_work_items_bounded(
            root,
            &[
                summary("STORY-9001", "InProgress"),
                summary("STORY-9002", "InProgress"),
            ],
            true, // no_forge — no gh dependency for this assertion
            false,
            Some(deadline),
        );

        assert!(
            rows.is_empty(),
            "an elapsed-before-start deadline must probe nothing: {rows:?}"
        );
        assert!(
            !scan.complete,
            "a truncated scan must say so, not read as an exhaustive empty result"
        );
        assert_eq!(scan.scanned, 0);
        assert_eq!(
            scan.candidates, 2,
            "the candidate COUNT must still reflect the full eligible set, even though \
             none of it was actually probed — narrowing what's reported is not the same \
             as narrowing what's eligible"
        );
    }

    #[cfg_attr(
        windows,
        ignore = "fake-gh harness is a bash script; gh cannot be stubbed via shebang on Windows"
    )]
    #[test]
    fn detector_skips_merged_pr_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        branch_with_commit(root, "story-1187-merged-pr", "STORY-1187");
        let head = git_output_checked(root, &["rev-parse", "story-1187-merged-pr"]).unwrap();

        let fake_gh = executable_fake_gh(
            root,
            &format!(
                r#"#!/usr/bin/env bash
if [[ "$*" == *"pr list"* ]]; then
  printf '[{{"number":1877,"title":"merged","headRefName":"story-1187-merged-pr","headRefOid":"{}","state":"MERGED","statusCheckRollup":[],"mergeable":"UNKNOWN","reviewDecision":""}}]'
  exit 0
fi
exit 1
"#,
                head.trim()
            ),
        );

        let _env = crate::test_env::EnvVarsGuard::set(&[(
            "AIDA_TEST_GH_BINARY",
            fake_gh.to_str().unwrap(),
        )]);
        let rows = collect_unshipped_work_items(
            root,
            &[summary("STORY-1187", "InProgress")],
            false,
            false,
        );

        assert!(
            rows.iter().all(|row| row.branch != "story-1187-merged-pr"),
            "merged PR heads must not be reported as unshipped: {rows:?}"
        );
    }

    #[test]
    fn detector_skips_patch_equivalent_squash_merged_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        git(root, &["checkout", "-b", "story-1187-squash", "main"]);
        commit_file(
            root,
            "squash.txt",
            "same patch\n",
            "[AI:codex] feat: branch work (STORY-1187)",
        );
        git(root, &["checkout", "main"]);
        commit_file(
            root,
            "squash.txt",
            "same patch\n",
            "[AI:codex] feat: landed equivalent patch (STORY-1187)",
        );

        assert_eq!(
            branch_ahead_of(root, "story-1187-squash", "main"),
            Some(1),
            "raw rev-list still sees the old branch as ahead"
        );
        assert_eq!(
            branch_unshipped_patch_count_default(root, "story-1187-squash"),
            Some(0),
            "patch-equivalence count must see it as shipped"
        );

        let rows =
            collect_unshipped_work_items(root, &[summary("STORY-1187", "InProgress")], true, false);

        assert!(
            rows.iter().all(|row| row.branch != "story-1187-squash"),
            "patch-equivalent branches must not be reported as unshipped: {rows:?}"
        );
    }

    // BUG-1531 criterion 1 + 4: a local `pr-<digits>` review-snapshot branch
    // (the one shape `ReviewForge::local_branch_for` creates, per TASK-1312's
    // parser) is a PUBLISHED snapshot and must never be reported as unshipped
    // work — regardless of naming collisions with other branches. A
    // genuinely unshipped, non-snapshot branch for a different spec must
    // still report, so the fix does not suppress the whole channel. The
    // "PR-2035" summary/branch is a self-contained fixture (no real PR
    // needed) matching the advisor's note that this shape should be
    // constructed, not depended on an accidental branch.
    // trace:BUG-1531 | ai:claude
    #[cfg_attr(
        windows,
        ignore = "fake-gh harness is a bash script; gh cannot be stubbed via shebang on Windows"
    )]
    #[test]
    fn detector_excludes_review_snapshot_branch_but_still_reports_genuine_unshipped_work() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        // The snapshot: a spec whose own id happens to be "PR-2035" so the
        // branch `pr-2035` clears the active-work candidate gate on its own,
        // exactly the way a real snapshot's spec id clears it via commit
        // trailers.
        branch_with_commit(root, "pr-2035", "PR-2035");
        // A genuinely unshipped, unrelated branch that must keep reporting.
        branch_with_commit(root, "story-1531-unshipped", "STORY-1531");
        let pr_2035_tip = git_output_checked(root, &["rev-parse", "pr-2035"]).unwrap();

        // The forge CONFIRMS PR 2035 exists and its head sha equals the
        // branch tip — the only condition BUG-1531's rework allows the
        // name-shaped `pr-2035` branch to be excluded on.
        let fake_gh = executable_fake_gh(
            root,
            &format!(
                r#"#!/usr/bin/env bash
if [[ "$*" == *"pr list"* ]]; then
  printf '[]'
  exit 0
fi
if [[ "$*" == *"pr view 2035"* ]]; then
  printf '{{"state":"MERGED","title":"t","mergedAt":null,"baseRefName":"main","headRefName":"pr-2035","headRefOid":"{}","isCrossRepository":false,"headRepository":null,"isDraft":false}}'
  exit 0
fi
exit 1
"#,
                pr_2035_tip.trim()
            ),
        );
        let _env = crate::test_env::EnvVarsGuard::set(&[(
            "AIDA_TEST_GH_BINARY",
            fake_gh.to_str().unwrap(),
        )]);
        let rows = collect_unshipped_work_items(
            root,
            &[
                summary("PR-2035", "InProgress"),
                summary("STORY-1531", "InProgress"),
            ],
            false,
            false,
        );

        assert!(
            rows.iter().all(|row| row.branch != "pr-2035"),
            "a forge-confirmed review snapshot (matching head sha) must never be reported as unshipped: {rows:?}"
        );
        let genuine = rows
            .iter()
            .find(|row| row.branch == "story-1531-unshipped")
            .expect("a genuinely unshipped branch must still report");
        assert_eq!(genuine.spec_id, "STORY-1531");
        // BUG-1756: never pushed in this fixture → push-first hint.
        assert_eq!(
            genuine.recovery,
            "git push -u origin story-1531-unshipped && aida pr ship story-1531-unshipped"
        );
    }

    // BUG-1531 PROXY DECISION: with NO forge available (`no_forge = true`),
    // a name-shaped `pr-N` branch cannot be confirmed, so it must be KEPT —
    // never hidden on name alone (PRIN-5) — and labelled unverified with no
    // "aida pr ship" hint.
    // trace:BUG-1531 | ai:claude
    #[test]
    fn detector_keeps_review_snapshot_named_branch_unverified_with_no_forge() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        branch_with_commit(root, "pr-2036", "PR-2036");

        let rows = collect_unshipped_work_items(
            root,
            &[summary("PR-2036", "InProgress")],
            true, // no_forge
            false,
        );

        let row = rows
            .iter()
            .find(|row| row.branch == "pr-2036")
            .expect("with no forge, a name-shaped pr-N branch must be kept, not hidden");
        assert_eq!(row.spec_id, "PR-2036");
        assert_eq!(
            row.recovery,
            "possible review snapshot (pr-2036), unverified"
        );
        assert!(
            !row.recovery.contains("aida pr ship"),
            "an unverified row must carry no ship hint: {row:?}"
        );
    }

    // BUG-1531 PROXY DECISION: the forge resolves PR 2037, but its recorded
    // head sha does NOT match this branch's tip (e.g. the change moved since
    // the fetch, or the name is a coincidence). The branch must be KEPT and
    // labelled unverified rather than excluded.
    // trace:BUG-1531 | ai:claude
    #[cfg_attr(
        windows,
        ignore = "fake-gh harness is a bash script; gh cannot be stubbed via shebang on Windows"
    )]
    #[test]
    fn detector_keeps_review_snapshot_named_branch_unverified_when_forge_head_differs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        branch_with_commit(root, "pr-2037", "PR-2037");

        let fake_gh = executable_fake_gh(
            root,
            r#"#!/usr/bin/env bash
if [[ "$*" == *"pr list"* ]]; then
  printf '[]'
  exit 0
fi
if [[ "$*" == *"pr view 2037"* ]]; then
  printf '{"state":"OPEN","title":"t","mergedAt":null,"baseRefName":"main","headRefName":"pr-2037","headRefOid":"0000000000000000000000000000000000dead","isCrossRepository":false,"headRepository":null,"isDraft":false}'
  exit 0
fi
exit 1
"#,
        );
        let _env = crate::test_env::EnvVarsGuard::set(&[(
            "AIDA_TEST_GH_BINARY",
            fake_gh.to_str().unwrap(),
        )]);
        let rows =
            collect_unshipped_work_items(root, &[summary("PR-2037", "InProgress")], false, false);

        let row = rows.iter().find(|row| row.branch == "pr-2037").expect(
            "a forge-resolved PR whose head sha differs from the branch tip must be kept, not excluded",
        );
        assert_eq!(row.spec_id, "PR-2037");
        assert_eq!(
            row.recovery,
            "possible review snapshot (pr-2037), unverified"
        );
        assert!(
            !row.recovery.contains("aida pr ship"),
            "an unverified row must carry no ship hint: {row:?}"
        );
    }

    // BUG-1531 criterion 3 + 6: a refusal binds to a COMMIT, not to a PR
    // number or a branch-naming convention. A branch whose tip is the exact
    // sha an outstanding (unclosed) RequestChanges verdict names must never
    // carry a "ship it" recommendation — driven from a verdict fixture
    // rather than from any particular PR number, so the check generalizes
    // past the specific pr2035-review incident that surfaced it.
    // trace:BUG-1531 | ai:claude
    #[test]
    fn detector_omits_ship_hint_for_commit_carrying_unresolved_review_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        branch_with_commit(root, "bug-1470-work", "BUG-1470");
        let tip = git_output_checked(root, &["rev-parse", "bug-1470-work"]).unwrap();

        let verdict_dir = root.join(".aida").join("review-verdicts");
        std::fs::create_dir_all(&verdict_dir).unwrap();
        std::fs::write(
            verdict_dir.join("BUG-1470.json"),
            format!(
                r#"{{"verdict":"request-changes","reviewed_sha":"{}","reviewed_branch":"bug-1470-work","recorded_at":"2026-09-21T05:00:00Z","summary":"blocking defects found"}}"#,
                tip.trim()
            ),
        )
        .unwrap();

        let rows =
            collect_unshipped_work_items(root, &[summary("BUG-1470", "InProgress")], true, false);

        for row in &rows {
            assert!(
                !row.recovery.contains("aida pr ship"),
                "a commit carrying an unresolved review verdict must never get a ship hint: {row:?}"
            );
        }
        assert!(
            rows.iter().any(|row| row.branch == "bug-1470-work"),
            "the refused branch should still surface as unshipped, just without a ship hint: {rows:?}"
        );
    }

    // PR #1999 rework: the reviewer's CHANGES REQUESTED finding on BUG-1288
    // was that `active_prefixes` (the bounded-probe candidate gate) was built
    // from only "inprogress"/"in-progress"/"done" statuses, so a branch whose
    // spec sat in NeedsAttention, Approved, or Draft — with genuine unshipped
    // commits and NO lease — was filtered out before the commit/patch probe
    // ever ran, and the detector under-reported. This end-to-end fixture pins
    // exactly that shape for all three previously-invisible statuses.
    // trace:BUG-1288 | ai:claude
    #[test]
    fn detector_lists_unshipped_work_on_leaseless_nonterminal_branches() {
        for status in ["NeedsAttention", "Approved", "Draft"] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            init_repo(root);

            let branch = format!("bug-9001-{}", status.to_ascii_lowercase());
            branch_with_commit(root, &branch, "BUG-9001");
            // Deliberately no lease and no live session: this is exactly the
            // "shelved but has real unshipped work" shape from the finding.

            let rows = collect_unshipped_work_items(
                root,
                &[summary("BUG-9001", status)],
                true, // no_forge: isolate from gh entirely
                false,
            );

            assert_eq!(
                rows.len(),
                1,
                "status={status}: a lease-less {status} branch with unmerged \
                 commits must be reported as unshipped, got: {rows:?}"
            );
            assert_eq!(rows[0].spec_id, "BUG-9001");
            assert_eq!(rows[0].branch, branch);
        }
    }

    // trace:BUG-1576 | ai:codex
    #[cfg_attr(windows, ignore = "fake-gh harness requires a Unix shebang")]
    #[test]
    fn detector_suppresses_multi_commit_squash_merge_from_recorded_pr_head() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        git(root, &["checkout", "-b", "bug-1576-squashed", "main"]);
        commit_file(root, "one.txt", "one\n", "fix: first half (BUG-1576)");
        commit_file(root, "two.txt", "two\n", "fix: second half (BUG-1576)");
        let head = git_output_checked(root, &["rev-parse", "HEAD"]).unwrap();
        git(root, &["checkout", "main"]);
        std::fs::write(root.join("one.txt"), "one\n").unwrap();
        std::fs::write(root.join("two.txt"), "two\n").unwrap();
        git(root, &["add", "one.txt", "two.txt"]);
        git(root, &["commit", "-m", "fix: squash landing (BUG-1576)"]);
        std::fs::write(root.join("one.txt"), "one\nlater main edit\n").unwrap();
        git(root, &["add", "one.txt"]);
        git(root, &["commit", "-m", "chore: advance main"]);

        assert_eq!(
            branch_unshipped_patch_count_default(root, "bug-1576-squashed"),
            Some(2)
        );
        let body = format!(
            r#"#!/usr/bin/env bash
printf '[{{"state":"MERGED","title":"fix (BUG-1576)","headRefName":"bug-1576-squashed","headRefOid":"{}"}}]'
"#,
            head.trim()
        );
        let fake = executable_fake_gh(root, &body);
        let _env =
            crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake.to_str().unwrap())]);
        let rows =
            collect_unshipped_work_items(root, &[summary("BUG-1576", "InProgress")], false, false);
        assert!(
            rows.is_empty(),
            "squash-merged work must not produce a ship hint: {rows:?}"
        );
    }

    // trace:BUG-1576 | ai:codex
    #[cfg_attr(windows, ignore = "fake-gh harness requires a Unix shebang")]
    #[test]
    fn detector_handles_regular_merge_and_keeps_open_or_unknown_work_safe() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        branch_with_commit(root, "bug-1577-regular", "BUG-1577");
        git(
            root,
            &[
                "merge",
                "--no-ff",
                "bug-1577-regular",
                "-m",
                "merge regular",
            ],
        );
        branch_with_commit(root, "bug-1578-abandoned", "BUG-1578");
        branch_with_commit(root, "bug-1578-real-head", "BUG-1578");
        branch_with_commit(root, "bug-1579-unknown", "BUG-1579");

        let fake = executable_fake_gh(
            root,
            r#"#!/usr/bin/env bash
printf '[{"state":"OPEN","title":"fix (BUG-1578)","headRefName":"bug-1578-real-head","headRefOid":"abc"}]'
"#,
        );
        let _env =
            crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake.to_str().unwrap())]);
        let rows = collect_unshipped_work_items(
            root,
            &[
                summary("BUG-1577", "InProgress"),
                summary("BUG-1578", "InProgress"),
                summary("BUG-1579", "InProgress"),
            ],
            false,
            false,
        );
        assert!(
            rows.iter().all(|r| r.spec_id != "BUG-1577"),
            "regular merge is shipped"
        );
        assert!(
            rows.iter().all(|r| r.spec_id != "BUG-1578"),
            "only the open PR head is authoritative and it is already open"
        );
        assert!(
            rows.iter().any(|r| r.spec_id == "BUG-1579"),
            "absent forge evidence must retain genuine work"
        );

        let ambiguous = parse_pr_head_state_snapshot(
            r#"[
          {"state":"OPEN","title":"one (BUG-1580)","headRefName":"bug-1580-one","headRefOid":"1"},
          {"state":"OPEN","title":"two (BUG-1580)","headRefName":"bug-1580-two","headRefOid":"2"}
        ]"#,
        )
        .unwrap();
        assert_eq!(
            ambiguous.open_heads_by_spec["BUG-1580"].len(),
            2,
            "ambiguity must remain explicit rather than selecting a head"
        );
    }

    // trace:BUG-1576 | ai:codex
    #[cfg_attr(windows, ignore = "fake-gh harness requires a Unix shebang")]
    #[test]
    fn merged_same_spec_does_not_hide_an_unrepresented_divergent_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        branch_with_commit(root, "bug-1576-divergent", "BUG-1576");
        branch_with_commit(root, "bug-1576-shipped", "BUG-1576");
        let shipped = git_output_checked(root, &["rev-parse", "bug-1576-shipped"]).unwrap();
        let body = format!(
            r#"#!/usr/bin/env bash
if [[ "$*" == *"--state open"* ]]; then printf '[]'; exit 0; fi
if [[ "$*" == *"--head bug-1576-shipped"* ]]; then
  printf '[{{"state":"MERGED","title":"fix (BUG-1576)","headRefName":"bug-1576-shipped","headRefOid":"{}"}}]'
else
  printf '[]'
fi
"#,
            shipped.trim()
        );
        let fake = executable_fake_gh(root, &body);
        let _env =
            crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake.to_str().unwrap())]);
        let rows =
            collect_unshipped_work_items(root, &[summary("BUG-1576", "InProgress")], false, false);
        assert!(rows.iter().any(|r| r.branch == "bug-1576-divergent"));
        assert!(rows.iter().all(|r| r.branch != "bug-1576-shipped"));
    }

    // trace:BUG-1576 | ai:codex
    #[cfg_attr(windows, ignore = "fake-gh harness requires a Unix shebang")]
    #[test]
    fn merged_head_lookup_is_targeted_not_limited_by_repository_history_page() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        branch_with_commit(root, "bug-1576-ancient", "BUG-1576");
        let head = git_output_checked(root, &["rev-parse", "bug-1576-ancient"]).unwrap();
        let calls = root.join("gh-calls");
        let body = format!(
            r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> '{}'
if [[ "$*" == *"--state open"* ]]; then printf '[]'; exit 0; fi
if [[ "$*" == *"--head bug-1576-ancient"* ]]; then
  printf '[{{"state":"MERGED","title":"ancient (BUG-1576)","headRefName":"bug-1576-ancient","headRefOid":"{}"}}]'
else
  printf '[]'
fi
"#,
            calls.display(),
            head.trim()
        );
        let fake = executable_fake_gh(root, &body);
        let _env =
            crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake.to_str().unwrap())]);
        let rows =
            collect_unshipped_work_items(root, &[summary("BUG-1576", "InProgress")], false, false);
        assert!(rows.is_empty());
        let calls = std::fs::read_to_string(calls).unwrap();
        assert!(
            calls.contains("--head bug-1576-ancient --state all"),
            "{calls}"
        );
        assert!(!calls.contains("--state all --limit 1000"), "{calls}");
    }
}

/// Look up a PR's merge state via `gh pr view <N>`. Returns `Some(true)`
/// when merged, `Some(false)` when open/closed-without-merge, `None` when
/// gh is missing or the call fails.
pub(crate) fn gh_pr_is_merged(project_root: &std::path::Path, n: u64) -> Option<bool> {
    let gh_bin = resolve_gh_binary()?;
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "view",
            &n.to_string(),
            "--json",
            "state",
            "-q",
            ".state",
        ])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout)
        .trim()
        .to_ascii_lowercase();
    Some(s == "merged")
}

/// Scan `~/.claude/projects/` for project dirs whose recorded cwd no
/// longer exists. Re-derived from the same detection logic as
/// `session_prune_orphans` so the cleanup surface stays consistent with
/// the cleanup command.
/// trace:STORY-385 | ai:claude
pub(crate) fn collect_orphan_project_dirs() -> Vec<status_cleanup::OrphanProjectDirItem> {
    let Some(home) = crate::home_dir() else {
        return Vec::new();
    };
    let projects = home.join(".claude/projects");
    if !projects.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(&projects) else {
        return out;
    };
    for entry in read.filter_map(|e| e.ok()) {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let mut found_cwd: Option<String> = None;
        let mut jsonl_count = 0;
        if let Ok(read) = std::fs::read_dir(&dir) {
            for jsonl in read.filter_map(|e| e.ok()) {
                if jsonl.path().extension().and_then(|s| s.to_str()) != Some("jsonl") {
                    continue;
                }
                jsonl_count += 1;
                if found_cwd.is_none() {
                    if let Ok(text) = std::fs::read_to_string(jsonl.path()) {
                        for line in text.lines().take(20) {
                            if let Some(idx) = line.find("\"cwd\":\"") {
                                let after = &line[idx + 7..];
                                if let Some(end) = after.find('"') {
                                    found_cwd = Some(after[..end].to_string());
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        let cwd_str = found_cwd.unwrap_or_else(|| {
            let name = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            name.replace('-', "/")
        });
        if cwd_str.is_empty() {
            continue;
        }
        if std::path::Path::new(&cwd_str).is_dir() {
            continue;
        }
        out.push(status_cleanup::OrphanProjectDirItem {
            path: dir,
            decoded_cwd: cwd_str,
            jsonl_count,
        });
    }
    out
}

/// Parse a lease scope of the form `PR-<n>` (or `MR-<n>`) into its PR
/// number. Used to cross-reference reviewer leases against `gh pr view`
/// for the stale-on-merged detector.
pub(crate) fn parse_pr_scope(scope: &str) -> Option<u64> {
    let rest = scope
        .strip_prefix("PR-")
        .or_else(|| scope.strip_prefix("MR-"))?;
    rest.parse().ok()
}

/// True when `branch` matches the AIDA *work-branch* naming convention:
/// `<type>-<n>` for spec types that drive an implementer session
/// (`task`, `story`, `bug`, `epic`, `spike`, `spec`, `fr`, `nfr`,
/// `user`, `sr`), optionally with a `-suffix` or `.suffix`. PR/MR
/// branches (reviewer worktrees) are excluded — they surface via the
/// stale-reviewer-on-merged detector.
/// trace:STORY-385 | ai:claude
pub(crate) fn is_work_spec_branch_name(branch: &str) -> bool {
    work_spec_id_from_branch(branch).is_some()
}

// trace:BUG-888 | ai:codex
pub(crate) fn work_spec_id_from_branch(branch: &str) -> Option<String> {
    const PREFIXES: &[&str] = &[
        "task-", "story-", "bug-", "epic-", "spike-", "spec-", "fr-", "nfr-", "user-", "sr-",
    ];
    let lower = branch.to_ascii_lowercase();
    for p in PREFIXES {
        if let Some(rest) = lower.strip_prefix(p) {
            let segments: Vec<&str> = rest.split(['-', '.']).filter(|s| !s.is_empty()).collect();
            if segments.is_empty() {
                continue;
            }
            if !segments
                .iter()
                .all(|s| s.chars().all(|c| c.is_ascii_alphanumeric()))
            {
                continue;
            }
            let spec_segment_count = if segments[0].chars().all(|c| c.is_ascii_digit()) {
                if segments
                    .get(1)
                    .is_some_and(|s| s.chars().all(|c| c.is_ascii_digit()))
                {
                    2
                } else {
                    1
                }
            } else {
                segments
                    .iter()
                    .position(|s| s.chars().all(|c| c.is_ascii_digit()))
                    .map(|idx| idx + 1)
                    .unwrap_or(0)
            };
            if spec_segment_count == 0 {
                continue;
            }
            let kind = p.trim_end_matches('-').to_ascii_uppercase();
            return Some(format!(
                "{}-{}",
                kind,
                segments[..spec_segment_count]
                    .iter()
                    .map(|s| s.to_ascii_uppercase())
                    .collect::<Vec<_>>()
                    .join("-")
            ));
        }
    }
    None
}

#[cfg(test)]
#[path = "tests/is_work_spec_branch_name_tests.rs"]
mod is_work_spec_branch_name_tests;
