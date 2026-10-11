//! Cleanup and awaiting reports, status hygiene/identity, fast status and dev-context CI lines (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// Build the full `CleanupReport` from the live project state. Each
/// detector is independent — when one fails (gh missing, git invocation
/// broken, store unreadable) it returns an empty vec rather than
/// aborting the whole report. trace:STORY-385 | ai:claude
pub(crate) fn collect_cleanup_report(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
) -> status_cleanup::CleanupReport {
    let leases = list_leases(project_root);
    let now = chrono::Utc::now();
    let live_sessions = process_probe::probe_live_claude_sessions();

    // Per-lease state classification — reused by sticky-in-progress,
    // dormant, and stale-reviewer detectors.
    let lease_states: Vec<(SessionLease, LeaseState)> = leases
        .iter()
        // BUG-511: review-verb leases classify by creator PID, not worktree.
        .map(|l| (l.clone(), lease_state_for(l, &live_sessions, now)))
        .collect();

    // TASK-1056: batch the per-branch git probes the detectors below need into
    // two `git for-each-ref` calls — local branch tip-times and the remote
    // branch name set — instead of spawning a `git log -1 --format=%ct` per
    // local branch and a `git rev-parse --verify origin/<branch>` per branch.
    // On a fleet repo (hundreds of work branches) that collapses thousands of
    // git subprocesses into two. The derived values are identical, so the
    // rendered report is unchanged. trace:TASK-1056 | ai:claude
    let local_branch_times = collect_local_branch_commit_times(project_root);
    let remote_branches = collect_remote_branch_name_set(project_root);

    // ── Detector 1: Uncommitted WIP across every worktree ──
    // Skip the orphan-store worktree (`.aida-store`) — it's AIDA-managed,
    // dirty state there is the normal mid-write moment between
    // `aida add`/`aida edit` and the next `aida push`, not lost work.
    // trace:STORY-385 | ai:claude
    let worktrees = list_worktrees(project_root);
    let mut uncommitted_wip = Vec::new();
    for wt in &worktrees {
        if !wt.path.exists() {
            continue;
        }
        if wt.branch.as_deref() == Some("aida-store") {
            continue;
        }
        let dirty = worktree_dirty_entries(&wt.path);
        if dirty.is_empty() {
            continue;
        }
        let canon_path = wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone());
        let scope = lease_states
            .iter()
            .find(|(l, _)| {
                let lp = l
                    .worktree_path
                    .canonicalize()
                    .unwrap_or_else(|_| l.worktree_path.clone());
                lp == canon_path
            })
            .map(|(l, _)| l.scope.clone());
        let branch = wt.branch.clone().unwrap_or_else(|| "(detached)".into());
        let age_hours = lease_states
            .iter()
            .find(|(l, _)| l.worktree_path == canon_path || l.worktree_path == wt.path)
            .map(|(l, _)| now.signed_duration_since(l.started_at).num_hours())
            .unwrap_or(0);
        uncommitted_wip.push(status_cleanup::UncommittedWipItem {
            worktree_path: wt.path.clone(),
            branch,
            scope,
            modified_files: dirty.len(),
            age_hours,
        });
    }
    uncommitted_wip.sort_by(|a, b| b.age_hours.cmp(&a.age_hours));

    // ── Detector 2: Sticky In-Progress specs (no Live/Dormant lease) ──
    let active_scopes: std::collections::HashSet<String> = lease_states
        .iter()
        .filter(|(_, s)| matches!(s, LeaseState::Live | LeaseState::Dormant))
        .map(|(l, _)| l.scope.clone())
        .collect();
    let mut sticky_in_progress = Vec::new();
    for req in &store.requirements {
        if !matches!(req.status, RequirementStatus::InProgress) {
            continue;
        }
        let Some(spec_id) = req.spec_id.as_deref() else {
            continue;
        };
        if active_scopes.contains(spec_id) {
            continue;
        }
        // STORY-385: parent EPICs / Visions / Folders flip to In Progress
        // when their children are working — the parent itself has no
        // branch and isn't "stuck". Skip these so the category surfaces
        // only specs that actually need a recovery action.
        // trace:STORY-385 | ai:claude
        if matches!(
            req.req_type,
            aida_core::RequirementType::Epic
                | aida_core::RequirementType::Vision
                | aida_core::RequirementType::Folder
                | aida_core::RequirementType::Meta
                | aida_core::RequirementType::Sprint
        ) {
            continue;
        }
        // STORY-385: derive the spec branch by convention (kebab-cased
        // spec id) and check whether it carries unpushed work. Branches
        // are typed `<type>-<num>` (e.g. `task-281`). Missing branches
        // are reported with `unpushed_commits == 0`.
        // trace:STORY-385 | ai:claude
        let branch_name = spec_id.to_ascii_lowercase();
        // TASK-1056: batched-map lookups replace per-spec `git rev-parse
        // --verify` probes. trace:TASK-1056 | ai:claude
        let local_exists = local_branch_times.contains_key(&branch_name);
        let (unpushed, pushed) = if local_exists {
            let upstream = format!("origin/{}", branch_name);
            let upstream_exists = remote_branches.contains(&branch_name);
            if upstream_exists {
                let ahead_upstream =
                    branch_ahead_of(project_root, &branch_name, &upstream).unwrap_or(0);
                let pushed_count = std::process::Command::new("git")
                    .arg("-C")
                    .arg(project_root)
                    .args(["rev-list", "--count", &format!("origin/main..{}", upstream)])
                    .output()
                    .ok()
                    .and_then(|o| {
                        if o.status.success() {
                            String::from_utf8_lossy(&o.stdout).trim().parse().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
                (ahead_upstream, pushed_count)
            } else {
                let ahead = branch_ahead_of(project_root, &branch_name, "origin/main")
                    .or_else(|| branch_ahead_of(project_root, &branch_name, "main"))
                    .unwrap_or(0);
                (ahead, 0)
            }
        } else {
            (0, 0)
        };
        sticky_in_progress.push(status_cleanup::StickyInProgressItem {
            spec_id: spec_id.to_string(),
            title: req.title.clone(),
            branch: if local_exists {
                Some(branch_name)
            } else {
                None
            },
            unpushed_commits: unpushed,
            pushed_commits: pushed,
            age_hours: None,
        });
    }

    // ── Detector 3 + 5: Open PRs + branches-ahead-no-PR (gh calls) ──
    let open_prs_snapshot = collect_open_prs(project_root);
    let open_pr_branches: std::collections::HashSet<String> =
        open_prs_snapshot.by_branch.keys().cloned().collect();
    // Every branch that has EVER had a PR — used to filter out
    // squash-merged branches whose local tip looks "ahead" of main but
    // already shipped. trace:STORY-385 | ai:claude
    let ever_pr_branches = collect_all_pr_head_branches(project_root);

    // STORY-385: branches-ahead-no-PR scans all local branches, which
    // becomes noisy in long-lived repos that accumulate hundreds of
    // historical branches. Filter to branches with a commit in the
    // recent window (default 30 days) AND skip the orphan store
    // branch — those carry archived history, not in-flight work.
    // trace:STORY-385 | ai:claude
    let recent_branch_window_secs: i64 = 30 * 24 * 3600;
    let now_secs = chrono::Utc::now().timestamp();
    let mut branches_ahead_no_pr = Vec::new();
    let default_ref = detect_default_branch_ref(project_root);
    if let Some(default_ref) = default_ref.as_deref() {
        // TASK-1056: one batched ahead-count map for every local branch,
        // replacing a `git rev-list --count <default>..<branch>` per surviving
        // branch in the loop below. Falls back per-branch for anything the
        // batch missed. trace:TASK-1056 | ai:claude
        let ahead_by_branch = collect_branch_ahead_of(project_root, default_ref);
        for branch in list_local_branches(project_root) {
            // Skip the default branch itself + the orphan store branch.
            if branch == "main" || branch == "master" || branch == "aida-store" {
                continue;
            }
            // Skip branches already in an open PR.
            if open_pr_branches.contains(&branch) {
                continue;
            }
            // Skip branches that have EVER had a PR (closed or merged).
            // A squash-merged branch's local tip remains "ahead" of main
            // because the squash gave its commits a different SHA; the
            // ever-PR set is the most reliable "this already shipped"
            // signal.
            if ever_pr_branches.contains(&branch) {
                continue;
            }
            // STORY-385: only flag branches whose name matches the
            // SPEC-ID convention for *work* branches — `task-<n>`,
            // `story-<n>`, `bug-<n>`, `epic-<n>`, `spike-<n>`, `spec-<n>`,
            // optionally with a `-suffix` like `-batch7`. The canonical
            // motivating example in the spec is `task-281`; long-lived
            // feature branches and ad-hoc agent-tool branches (e.g.
            // `claude/plan-...`) are not what this category is for.
            // `pr-<n>` / `mr-<n>` branches are reviewer-worktree state
            // and surface elsewhere (stale-reviewer-on-merged), so
            // exclude them here to avoid double-counting.
            // trace:STORY-385 | ai:claude
            if !is_work_spec_branch_name(&branch) {
                continue;
            }
            // Skip branches with no recent activity — the cleanup section
            // is for in-flight work, not historical archaeology.
            // TASK-1056: read the tip-commit time from the batched
            // for-each-ref map instead of a `git log -1 --format=%ct` per
            // branch. trace:TASK-1056 | ai:claude
            let last_commit_ts = local_branch_times.get(&branch).copied().unwrap_or(0);
            if now_secs - last_commit_ts > recent_branch_window_secs {
                continue;
            }
            // TASK-1056: batched-map ahead count, falling back to the
            // per-branch probe when the batch didn't cover this branch.
            // trace:TASK-1056 | ai:claude
            let ahead = match ahead_by_branch
                .get(&branch)
                .copied()
                .or_else(|| branch_ahead_of(project_root, &branch, default_ref))
            {
                Some(n) if n > 0 => n,
                _ => continue,
            };
            // Skip branches already surfaced as sticky-in-progress —
            // those are visible in their own category and double-listing
            // them is noise.
            let already_sticky = sticky_in_progress
                .iter()
                .any(|s| s.branch.as_deref() == Some(branch.as_str()));
            if already_sticky {
                continue;
            }
            // TASK-1056: batched-map membership replaces a per-branch
            // `git rev-parse --verify origin/<branch>`. trace:TASK-1056
            let upstream_exists = remote_branches.contains(&branch);
            branches_ahead_no_pr.push(status_cleanup::BranchAheadItem {
                branch,
                commits_ahead: ahead,
                has_upstream: upstream_exists,
            });
        }
    }
    branches_ahead_no_pr.sort_by(|a, b| b.commits_ahead.cmp(&a.commits_ahead));

    let mut open_prs: Vec<status_cleanup::OpenPrItem> =
        open_prs_snapshot.by_branch.values().cloned().collect();
    open_prs.sort_by_key(|p| p.number);

    // ── Detector 4: Missed auto-bump ──
    let landings = scan_default_branch_for_spec_landings(project_root, 200);
    let mut missed_auto_bump = Vec::new();
    for (spec_id, sha) in landings {
        let Some(req) = store.get_requirement_by_spec_id(&spec_id) else {
            continue;
        };
        if !matches!(req.status, RequirementStatus::Done) {
            continue;
        }
        missed_auto_bump.push(status_cleanup::MissedAutoBumpItem {
            spec_id,
            title: req.title.clone(),
            landing_sha: sha,
        });
    }

    // ── Detector 7: Stale reviewer leases on merged PRs ──
    // (Detected first so we can dedupe these out of the Dormant category
    // below — a reviewer lease on a merged PR is recoverable but the
    // merged-PR signal is more actionable.)
    // trace:STORY-385 | ai:claude
    let mut stale_reviewer_leases = Vec::new();
    let mut stale_reviewer_lease_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for (l, _state) in &lease_states {
        if l.role.as_deref() != Some("reviewer") {
            continue;
        }
        let Some(pr_n) = parse_pr_scope(&l.scope) else {
            continue;
        };
        // Skip if the PR is still open — those show up under "Open PRs".
        if open_prs_snapshot
            .by_branch
            .values()
            .any(|p| p.number == pr_n)
        {
            continue;
        }
        let merged = gh_pr_is_merged(project_root, pr_n).unwrap_or(false);
        if !merged {
            continue;
        }
        let age_hours = now.signed_duration_since(l.started_at).num_hours();
        stale_reviewer_lease_ids.insert(l.id.clone());
        stale_reviewer_leases.push(status_cleanup::StaleReviewerLeaseItem {
            lease_id: l.id.clone(),
            pr_number: pr_n,
            worktree_path: l.worktree_path.clone(),
            age_hours,
        });
    }
    stale_reviewer_leases.sort_by(|a, b| b.age_hours.cmp(&a.age_hours));

    // ── Detector 6: Dormant leases ──
    let mut dormant_leases = Vec::new();
    for (l, state) in &lease_states {
        if !matches!(state, LeaseState::Dormant) {
            continue;
        }
        // STORY-385: stale-reviewer-on-merged is a strict refinement of
        // dormant — show the more actionable signal, suppress here.
        if stale_reviewer_lease_ids.contains(&l.id) {
            continue;
        }
        let age_hours = now.signed_duration_since(l.started_at).num_hours();
        // BUG-376: detect the "lingering implementer with done queue"
        // subcategory by looking up the lease's scope as a spec ID and
        // checking whether it has already reached Done / Completed.
        // Informational annotation only — the recovery verb is the same
        // as a plain dormant lease (`aida session end`). Cap the lookup
        // to implementer-role leases so reviewer / triage / advisor
        // dormancy doesn't get mis-tagged (their lifecycle relative to
        // the spec's status is different — a reviewer can be dormant on
        // a Done spec entirely legitimately while waiting for merge).
        // trace:BUG-376 | ai:claude
        let spec_done = if matches!(l.role.as_deref(), Some("implementer")) {
            store.requirements.iter().any(|req| {
                req.spec_id.as_deref() == Some(l.scope.as_str())
                    && matches!(
                        req.status,
                        RequirementStatus::Done | RequirementStatus::Completed
                    )
            })
        } else {
            false
        };
        dormant_leases.push(status_cleanup::DormantLeaseItem {
            lease_id: l.id.clone(),
            scope: l.scope.clone(),
            role: l.role.clone(),
            worktree_path: l.worktree_path.clone(),
            age_hours,
            spec_done,
        });
    }
    dormant_leases.sort_by(|a, b| b.age_hours.cmp(&a.age_hours));

    // ── Detector 8: Orphan Claude Code project dirs ──
    let orphan_project_dirs = collect_orphan_project_dirs();

    // ── Detector 9 (STORY-469 Guard 3): claimed-Done-vs-substrate divergence ──
    // A spec whose status is Done/Completed but whose local reality contradicts
    // the claim — an agent's "I shipped" the substrate doesn't corroborate. Two
    // signals fire (see `detect_claimed_done_divergence`): (1) an active lease +
    // a dirty worktree (uncommitted work despite Done), (2) no commit references
    // the spec AND no PR exists. We build a filesystem-derived input row per
    // Done/Completed spec, then the pure detector decides. trace:STORY-469
    // BUG-606: full-history, body-aware corroboration set. The prior call here
    // capped at 200 commits AND read subjects only, so every Done/Completed spec
    // older than ~200 commits — or referenced only in a squash-merge BODY — was
    // falsely reported as "no commit references it" (~1464 false positives on
    // this repo). The missed-auto-bump detector above keeps the recent-window
    // landing scan (it needs the landing sha and only cares about recent Done
    // specs). trace:BUG-606 | ai:claude
    let landed_spec_ids = referenced_spec_ids_on_default_branch(project_root);
    // Map a spec's active-lease worktree (Live/Dormant) to its dirty count.
    let mut claimed_done_inputs = Vec::new();
    for req in &store.requirements {
        if !matches!(
            req.status,
            RequirementStatus::Done | RequirementStatus::Completed
        ) {
            continue;
        }
        let Some(spec_id) = req.spec_id.as_deref() else {
            continue;
        };
        let active_lease = lease_states.iter().find(|(l, s)| {
            l.scope == spec_id && matches!(s, LeaseState::Live | LeaseState::Dormant)
        });
        let (has_active_lease, branch, modified_files, age_hours) = match active_lease {
            Some((l, _)) => {
                let dirty = if l.worktree_path.exists() {
                    worktree_dirty_entries(&l.worktree_path).len()
                } else {
                    0
                };
                (
                    true,
                    Some(l.branch.clone()),
                    dirty,
                    now.signed_duration_since(l.started_at).num_hours(),
                )
            }
            None => (false, None, 0, 0),
        };
        let branch_name = spec_id.to_ascii_lowercase();
        // BUG-606: corroborate by spec_id OR agreed_id — a commit may reference
        // the short display id (TASK-216) while the spec is stored under its
        // node-aware id, or vice versa.
        let has_commit = landed_spec_ids.contains(&spec_id.to_ascii_uppercase())
            || req
                .agreed_id
                .as_deref()
                .map(|a| landed_spec_ids.contains(&a.to_ascii_uppercase()))
                .unwrap_or(false);
        let has_pr =
            open_pr_branches.contains(&branch_name) || ever_pr_branches.contains(&branch_name);
        claimed_done_inputs.push(status_cleanup::ClaimedDoneInput {
            spec_id: spec_id.to_string(),
            title: req.title.clone(),
            status: format!("{:?}", req.status),
            has_active_lease,
            branch,
            modified_files,
            age_hours,
            has_commit,
            has_pr,
        });
    }
    let claimed_done_diverged =
        status_cleanup::detect_claimed_done_divergence(&claimed_done_inputs);

    status_cleanup::CleanupReport {
        uncommitted_wip,
        sticky_in_progress,
        branches_ahead_no_pr,
        missed_auto_bump,
        open_prs,
        dormant_leases,
        stale_reviewer_leases,
        orphan_project_dirs,
        claimed_done_diverged,
        // STORY-508/TASK-651: resolve the active forge so the open-change hint
        // names the right CLI (gh/glab) or none (pure-git).
        forge_kind: Some(crate::forge::resolve_forge_kind(project_root)),
    }
}

/// Gather every "human-gate" item — mergeable PRs the operator still
/// needs to merge, briefs filed for the running agent, findings awaiting
/// triage, reviewer-queue verdicts, `NeedsAttention` escalations, unread
/// mail, and pending worker directives —
/// into the structured report rendered as the "Awaiting you" section.
/// Empty (and so hidden) on a quiet day; that absence is the signal.
/// trace:STORY-465 | ai:claude
pub(crate) fn collect_awaiting_report(
    project_root: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
    ctx: &UserStatusContext,
    no_ci: bool,
) -> awaiting_you::AwaitingReport {
    collect_awaiting_report_inner(project_root, backend, ctx, no_ci, false)
}

pub(crate) fn collect_awaiting_report_inner(
    project_root: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
    ctx: &UserStatusContext,
    no_ci: bool,
    notice_fast: bool,
) -> awaiting_you::AwaitingReport {
    // BUG-1288: the nightly channel performs independent forge reads. Start it
    // while the PR/store/local channels are collected so network latency is
    // paid once rather than serially on the machine-readable path.
    // trace:BUG-1288 | ai:codex
    let nightly_handle = if no_ci {
        None
    } else {
        let root = project_root.to_path_buf();
        Some(std::thread::spawn(move || cached_nightly_red_status(&root)))
    };
    // Mergeable PRs — reuse the same `gh pr list` snapshot that the cleanup
    // report consumes, then filter via the awaiting-you classifier.
    // STORY-1397: every active hold is read once, READ-ONLY — this render
    // (and the per-turn `--notice` hook) never rewrites a marker, writes a
    // brief, or probes process liveness; `aida merge-hold list --fix` owns
    // that. A held PR is never offered as ready-to-merge, and every held PR
    // stays visible with its typed hold reason. trace:STORY-1397 | ai:claude
    let hold_records: Vec<merge_hold::MergeHoldRecord> = merge_hold::list_holds(project_root)
        .into_iter()
        .filter_map(|(pr, _)| merge_hold::read_hold_record(project_root, pr))
        .collect();
    let held_numbers: std::collections::HashSet<u64> =
        hold_records.iter().map(|record| record.pr).collect();
    let open_prs: Option<Vec<status_cleanup::OpenPrItem>> = (!no_ci).then(|| {
        collect_open_prs(project_root)
            .by_branch
            .into_values()
            .collect()
    });
    let mergeable_prs = match &open_prs {
        None => Vec::new(),
        Some(all_prs) => {
            let prs: Vec<_> = all_prs
                .iter()
                .filter(|pr| !held_numbers.contains(&pr.number))
                .cloned()
                .collect();
            let local_suppressed = local_suppressed_prs(project_root, &prs);
            let mut items = awaiting_you::classify_open_prs(&prs, &local_suppressed);
            // STORY-1405: a mergeable PR a reviewer is mid-way through says so.
            // Local file reads only; the head comes from the same snapshot.
            // trace:STORY-1405 | ai:claude
            let marker_root = main_worktree_root_from(project_root);
            for item in &mut items {
                let head = prs
                    .iter()
                    .find(|p| p.number == item.number)
                    .and_then(|p| p.head_sha.as_deref());
                item.under_review =
                    review_marker::live_description(&marker_root, item.number, head);
            }
            items
        }
    };
    // Non-recusal holds (supervision / rework / decision / unreadable) on
    // OPEN PRs: shown with their reason and whose action it is. Needs the PR
    // snapshot to tell an open PR from a stale marker, so the notice path
    // (no snapshot) skips it exactly as it skips `mergeable_prs`.
    let mut held_prs: Vec<awaiting_you::HeldPrItem> = match &open_prs {
        None => Vec::new(),
        Some(all_prs) => hold_records
            .iter()
            .filter(|record| record.reason_kind != merge_hold::HoldReasonKind::Recusal)
            .filter_map(|record| {
                all_prs.iter().find(|pr| pr.number == record.pr).map(|pr| {
                    // trace:BUG-1788 | ai:antigravity
                    let approved_at_head = pr.head_sha.as_deref().filter(|head| {
                        crate::review_verdict::verdicts_for_sha(project_root, head)
                            .into_iter()
                            .any(|v| !v.is_closed() && v.kind.approves())
                    });
                    awaiting_you::project_held_pr(record, &pr.title, approved_at_head, pr.is_draft)
                })
            })
            .collect(),
    };

    // STORY-1397: typed recusal holds are local/offline coordination state, so
    // they remain visible even on the fast notice path. A recused principal is
    // explicitly told this is awaiting somebody else, never offered a merge.
    // A moved head is PROJECTED as stale in memory; nothing is written here.
    // The principal comes from the self-declared AIDA_AGENT_ID and only
    // personalizes this read-only view (see `merge_hold::current_principal`).
    let current_principal = merge_hold::current_principal()
        .map(|p| p.key())
        .unwrap_or_default();
    let live_pr_heads: std::collections::HashMap<u64, &str> = open_prs
        .iter()
        .flatten()
        .filter_map(|pr| pr.head_sha.as_deref().map(|sha| (pr.number, sha)))
        .collect();
    let recusal_holds = hold_records
        .iter()
        .filter(|record| record.reason_kind == merge_hold::HoldReasonKind::Recusal)
        .map(|record| merge_hold::project_live_head(record, live_pr_heads.get(&record.pr).copied()))
        .filter_map(|record| awaiting_you::project_recusal_hold(&record, &current_principal))
        .collect();

    // Pending briefs — prefer narrowing to the running agent so we
    // don't spam the operator with hand-offs filed for a different
    // agent. `detect_agent_type` returns "other" when no agent env is
    // present (raw shell), in which case we surface every unacked brief
    // so the operator orchestrating multiple agents still sees the
    // backlog of hand-offs.
    let agent_type = agent_registry::detect_agent_type();
    let brief_filter = match agent_type.as_str() {
        "claude" | "codex" | "antigravity" => Some(agent_type.as_str()),
        _ => None,
    };
    // BUG-569: bare agent-type filter for the status surface — stay silent on
    // type-class ambiguity.
    let pending_briefs = collect_agent_briefs_inner(project_root, brief_filter, false, false)
        .ok()
        .unwrap_or_default()
        .into_iter()
        .map(|e| awaiting_you::PendingBriefItem {
            agent: e.agent,
            spec_id: e.spec_id,
            path: e.path,
        })
        .collect();

    // Escalations need the full summary list; findings need a draft-only view.
    // trace:TASK-1526 | ai:codex
    // Advisory reads use a zero-wait labelled snapshot. Compatible full-rebuild
    // cases defer, preserving the no-full-scan notice contract.
    let summaries = if notice_fast {
        backend.list_summaries_with_budget(
            &aida_core::ListFilter::default(),
            aida_core::db::cache_refresh::ReadBudget(std::time::Duration::ZERO),
        )
    } else {
        backend.list_summaries(&aida_core::ListFilter::default())
    }
    .unwrap_or_default();
    // BUG-1773: a rework hold DERIVED from the verdict corpus, for an open PR
    // with no marker of its own. Only `handle_review_record_at` writes a
    // `.aida/merge-holds/PR-<n>` marker, so a refusal recorded by any other
    // producer (the reviewer skill's direct write, `stamp_pr_review_verdict`,
    // a hand-edited file) was invisible here — BUG-1705 measured exactly that
    // on PR #2242, which sat CLEAN and unlisted with `request-changes` at its
    // exact head. The predicate is the POSITIVE form (an outstanding refusal AT
    // the current head), not `local_verdict_blocks_merge`, which is the
    // fail-closed merge test and would list every PR carrying a stale verdict.
    //
    // STORY-1397's contract is preserved: this reads the corpus and writes
    // nothing — no marker, no label. Arming the gate is BUG-1774.
    // trace:BUG-1773 | ai:claude
    if let Some(all_prs) = open_prs.as_ref() {
        let mut status_by_spec = std::collections::HashMap::new();
        insert_summary_statuses(&mut status_by_spec, &summaries);
        let candidates: Vec<awaiting_you::CorpusHoldCandidate> = all_prs
            .iter()
            .map(|pr| {
                let spec = worktree_lease::spec_id_from_branch(&pr.head_branch);
                let mut keys = vec![format!("PR-{}", pr.number)];
                keys.extend(spec.clone());
                let bodies = keys
                    .iter()
                    .map(|key| review_verdict::verdict_path(project_root, key))
                    .filter_map(|path| std::fs::read_to_string(path).ok())
                    .collect();
                let spec_completed = spec.as_deref().is_some_and(|id| {
                    status_by_spec
                        .get(&id.to_ascii_uppercase())
                        .is_some_and(|status| status == "completed")
                });
                awaiting_you::CorpusHoldCandidate {
                    pr: pr.number,
                    title: pr.title.clone(),
                    head_sha: pr.head_sha.clone(),
                    bodies,
                    spec_completed,
                    has_marker_hold: held_numbers.contains(&pr.number),
                    is_draft: pr.is_draft,
                }
            })
            .collect();
        held_prs.extend(awaiting_you::corpus_held_prs(&candidates));
    }
    // BUG-472: the findings breadcrumb must mirror `aida findings list` — DRAFT
    // specs carrying a from-* tag only. Building it from the unfiltered
    // `summaries` also counts completed/rejected specs that still carry their
    // origin from-review/from-implementer tag, overcounting the count (status
    // said "35" while `aida findings list` showed 0). Filter to draft like
    // print_status_findings_section does. trace:BUG-472 | ai:claude
    let findings_total = {
        let filter = aida_core::ListFilter {
            status: Some("draft".to_string()),
            ..Default::default()
        };
        let draft = if notice_fast {
            backend.list_summaries_with_budget(
                &filter,
                aida_core::db::cache_refresh::ReadBudget(std::time::Duration::ZERO),
            )
        } else {
            backend.list_summaries(&filter)
        }
        .unwrap_or_default();
        findings::count_findings(&findings::build_findings_view(
            &draft,
            &findings::FindingsFilter::default(),
        ))
    };
    // trace:STORY-1023 | ai:codex
    let mut shelved_total = 0usize;
    let mut escalations: Vec<awaiting_you::EscalationItem> = Vec::new();
    for s in summaries
        .iter()
        .filter(|s| s.status.eq_ignore_ascii_case("NeedsAttention"))
    {
        let spec_id = s
            .agreed_id
            .clone()
            .or_else(|| s.spec_id.clone())
            .unwrap_or_else(|| "?".to_string());
        match backend.get_requirement_by_spec_id(&spec_id) {
            Ok(Some(req)) if req.failure_reason.is_some() => {
                shelved_total += 1;
            }
            _ => escalations.push(awaiting_you::EscalationItem {
                spec_id,
                title: s.title.clone(),
            }),
        }
    }

    // Reviewer-queue items — surface queue entries where the verdict is
    // the operator's only when the active role IS reviewer. Otherwise
    // these would just duplicate the Queue section below.
    //
    // BUG-1508 AC1/AC2/AC3/AC8: each routed row is annotated with its
    // actionability, resolved entirely locally (verdict file + git refs
    // via `reviewer_row_actionability` -- the same helper `aida queue
    // list --for reviewer` uses, and no forge call). Rows are never
    // dropped for being already-reviewed (AC2); the depth figure `aida
    // awaiting` leads with ("actionable N of M") is built from these
    // states in `render`/`compact_line`/`to_json` below.
    // trace:BUG-1508 | ai:claude
    let reviewer_queue_items: Vec<awaiting_you::ReviewerQueueItem> =
        if matches!(ctx.role.as_deref(), Some("reviewer")) {
            let leases = list_leases(project_root);
            ctx.queue_head
                .iter()
                .filter(|r| !r.in_progress)
                .map(|r| {
                    let state = backend
                        .get_requirement_by_spec_id(&r.spec_id)
                        .ok()
                        .flatten()
                        .map(|req| {
                            reviewer_row_actionability(project_root, &req, &leases, |uuid| {
                                backend
                                    .get_requirement(&uuid)
                                    .ok()
                                    .flatten()
                                    .and_then(|r| r.agreed_id.or(r.spec_id))
                            })
                        })
                        .unwrap_or(review_verdict::ReviewActionability::NeedsReview);
                    awaiting_you::ReviewerQueueItem {
                        spec_id: r.spec_id.clone(),
                        title: r.title.clone(),
                        state,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

    // Unread mail — folded into the awaiting-you report so the coordination
    // inbox is ONE surface (STORY-741). Reads only the local + canonical
    // mailbox files (never a network call), so this stays cheap enough to ride
    // the per-turn `aida awaiting --notice` path. Every read degrades to empty
    // on error, never bubbling up.
    //
    // BUG-767: the headline number is the OPERATOR's OWN inbox — the handle
    // this session is (`AIDA_USER`/`USER`), and nothing else. The rest of the
    // ambient identity set (`inbox_identities()` also unions the session ROLE
    // and the AGENT TYPE) are SHARED inboxes carrying agent-to-agent traffic;
    // counting them as "your unread mail" made the `awaiting` number a function
    // of env vars rather than of your inbox, so an operator who emptied their
    // own inbox saw the count jump to the shared backlog instead of 0. They are
    // still surfaced — as a separate, labelled `shared_unread` line — so the
    // fleet-wide view survives without leaking into the operator-gated count.
    // trace:STORY-741 | ai:claude
    // trace:BUG-767 | ai:claude
    let mail = {
        let store_root = project_root.join(".aida-store");
        let local = mailbox_store::read_local_messages(project_root).unwrap_or_default();
        let canonical = mailbox_store::read_canonical_messages(&store_root).unwrap_or_default();
        let merged = aida_core::mailbox::merge_dedup(&local, &canonical);
        let watermarks = mailbox_store::read_all_watermarks(project_root).unwrap_or_default();
        let operator = current_user_id(None);
        let mut shared: Vec<String> = Vec::new();
        if let Some(raw) = ctx
            .role
            .clone()
            .or_else(|| std::env::var("AIDA_SESSION_ROLE").ok())
            .filter(|s| !s.trim().is_empty())
        {
            let (role, _is_default) = resolve_effective_role(Some(raw.as_str()));
            if role != operator {
                shared.push(role);
            }
        }
        awaiting_you::split_mail_scopes(&operator, &shared, &merged, &watermarks)
    };

    // Pending worker directives — the enqueue channel (`aida human audit`,
    // MCP directive posters, hand-written overnight plans) lands in the local
    // directive file, and until now only surfaced via the worker poll view.
    // Folding it in makes the unified inbox the one place the advisor looks.
    // A single local file read (absent file → empty), never a network call,
    // so it rides the per-turn notice path too. trace:TASK-1146 | ai:claude
    let worker_directives = {
        let directives = worker::parse_directives(&worker::worker_cmd_path(project_root));
        awaiting_you::DirectivesChannel {
            pending: directives.len(),
            next: directives.first().map(|d| d.summary()),
        }
    };

    // Due seat jobs from the `[schedule]` registry — the per-seat delivery
    // channel (STORY-1226). Scoped to the session's seat when one is known;
    // every seat otherwise (the operator orchestrating several seats sees
    // them all, like briefs). File-only: a TOML parse plus the store ledger /
    // local state files — never git, never the network — so it rides the
    // per-turn notice. Substrate jobs never surface here; the tick runs them.
    // trace:STORY-1226 | ai:claude
    let cron = {
        let seat = ctx
            .role
            .clone()
            .or_else(|| std::env::var("AIDA_SESSION_ROLE").ok())
            .filter(|s| !s.trim().is_empty())
            .map(|r| canonical_role_name(&r));
        let due = maintenance_schedule::due_seat_jobs(project_root, seat.as_deref());
        awaiting_you::CronChannel {
            due: due.len(),
            next: due.first().map(|j| j.line(chrono::Utc::now())),
        }
    };

    // The per-turn notice has a hard latency contract. Branch divergence walks
    // spawn git processes and can exceed that budget in a busy repository; the
    // full awaiting/status views retain this channel. trace:BUG-1239 | ai:codex
    let (unshipped_work, unshipped_work_scan) = if notice_fast {
        (Vec::new(), None)
    } else {
        // BUG-1288: bound the wall clock this scan may spend, not which
        // branches are eligible for it — the candidate set is exactly the
        // one PR #1999 (STORY-1368) widened it to. A repo whose candidate
        // population has since grown large enough to blow the budget gets a
        // truncated-but-honest scan (`unshipped_work_scan.complete: false`)
        // instead of a multi-minute block; see
        // `collect_unshipped_work_items_bounded`. trace:BUG-1288 | ai:claude
        let deadline = std::time::Instant::now() + unshipped_work_scan_budget();
        let (items, status) = collect_unshipped_work_items_bounded(
            project_root,
            &summaries,
            no_ci,
            !no_ci,
            Some(deadline),
        );
        (items, Some(status))
    };
    // trace:STORY-1043 | ai:codex
    let nightly_red = nightly_handle.and_then(|handle| handle.join().ok().flatten());

    // TASK-192: red PRs must not enter the green orphan-review sweep, but a
    // definitively failing PR with no verdict, hold, reviewer route, or live
    // branch owner still needs an operator-visible repair route. This is a
    // full-report channel only because it depends on the forge snapshot.
    // trace:TASK-192 | ai:codex
    let unowned_failing_prs = if notice_fast || no_ci {
        Vec::new()
    } else {
        let routed_prs = reviewer_queue_story_ids(project_root).map(|queued_story_ids| {
            let queued_story_ids: std::collections::HashSet<String> = queued_story_ids
                .into_iter()
                .map(|id| id.to_ascii_uppercase())
                .collect();
            summaries
                .iter()
                .filter(|summary| {
                    summary
                        .agreed_id
                        .as_deref()
                        .or(summary.spec_id.as_deref())
                        .is_some_and(|id| queued_story_ids.contains(&id.to_ascii_uppercase()))
                })
                .filter_map(|summary| parse_review_story_pr_number(&summary.title))
                .collect::<std::collections::HashSet<u64>>()
        });
        let live_branches = live_owned_branches(project_root);
        // BUG-1514: the general form of the two-way inconsistency (acceptance
        // #1) — a spec already marked Done whose PR title names it is red
        // regardless of who owns/reviews the branch. Built from the spec ids
        // in the PR title (the same trailer convention `aida pr` writes), so
        // it costs no extra `gh` calls beyond the snapshot already fetched.
        // trace:BUG-1514 | ai:claude
        let done_spec_ids: std::collections::HashSet<String> = summaries
            .iter()
            .filter(|s| s.status.eq_ignore_ascii_case("done"))
            .flat_map(|s| [s.spec_id.clone(), s.agreed_id.clone()])
            .flatten()
            .map(|id| id.to_ascii_uppercase())
            .collect();
        let candidates: Vec<awaiting_you::UnownedFailingPrCandidate> =
            collect_open_prs(project_root)
                .by_branch
                .into_values()
                .map(|pr| {
                    let done_spec = pr_ship::extract_spec_ids_from_text(&pr.title)
                        .into_iter()
                        .find(|id| done_spec_ids.contains(id));
                    awaiting_you::UnownedFailingPrCandidate {
                        has_local_verdict: pr_has_local_verdict(project_root, pr.number),
                        held: pr_has_merge_hold(project_root, &pr),
                        route: reviewer_route_for_pr(routed_prs.as_ref(), pr.number),
                        actively_owned: live_branches.contains(&pr.head_branch),
                        done_spec,
                        pr,
                    }
                })
                .collect();
        awaiting_you::classify_unowned_failing_prs(&candidates, chrono::Utc::now())
    };

    // STORY-1419: PRs whose rework has landed on a refusal this seat recorded.
    // Needs the PR snapshot for current heads, so it is skipped on the
    // notice-fast path and when CI/forge lookups are off — the same contract as
    // mergeable_prs. Reuses the MEMOIZED snapshot, so this adds no request.
    // trace:STORY-1419 | ai:claude
    // trace:BUG-1549 | ai:claude — shares the candidate build with rework_ready
    // below (same PR snapshot, same seat scoping); only the direction differs.
    let (rework_ready, stale_approvals, blocked_reviews) = if notice_fast || no_ci {
        (Vec::new(), Vec::new(), Vec::new())
    } else {
        let snapshot = collect_open_prs(project_root);
        let seat = std::env::var("AIDA_USER")
            .ok()
            .filter(|s| !s.trim().is_empty());
        // trace:STORY-1420 | ai:claude — an exited recorder's refusal is
        // shown to every seat instead of routed to nobody. Liveness is
        // registry + pid only.
        let reader = awaiting_you::ReworkReader {
            identity: seat.as_deref(),
        };
        let rows = pr_review_rows_routed(project_root, snapshot.by_branch.values(), reader);
        (
            rows.rework_ready,
            rows.stale_approvals,
            rows.blocked_reviews,
        )
    };

    // TASK-1445 (containment for BUG-1510 AC5): does a live drain's
    // lease-based PR attribution agree with what the PR's own commits
    // credit? Local-only (drain-state file + a git log per in-flight
    // member) — no network — so it's skipped on the notice-fast path for
    // the same latency reason as `unshipped_work` above.
    // trace:TASK-1445 | ai:claude
    let pr_attribution_disagreements = if notice_fast {
        Vec::new()
    } else {
        collect_pr_attribution_disagreements(project_root)
    };

    // BUG-1564: In-Progress specs with no live session/lease/process behind
    // the flag — reuses `gather_running_work`'s orphan pass verbatim (the
    // same verdict `aida ps` computes), so this is not a second
    // implementation of the detection. Needs a lease scan + a live-process
    // probe, the same "heavier local probe" tier as `unshipped_work` /
    // `pr_attribution_disagreements` above, so it is skipped on the
    // notice-fast path to keep the per-turn hook local and fast (no
    // full-store load, no network).
    // trace:BUG-1564 | ai:claude
    let orphaned_in_progress = if notice_fast {
        Vec::new()
    } else {
        let (_rows, orphans) = gather_running_work(project_root);
        orphaned_in_progress_items(orphans, |spec| {
            summaries
                .iter()
                .find(|s| {
                    s.agreed_id.as_deref() == Some(spec) || s.spec_id.as_deref() == Some(spec)
                })
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s.modified_at).ok())
                .map(|t| {
                    let secs = chrono::Utc::now()
                        .signed_duration_since(t.with_timezone(&chrono::Utc))
                        .num_seconds()
                        .max(0) as u64;
                    format!("last touched {} ago", humanize_duration_secs(secs))
                })
                .unwrap_or_else(|| "last-touched time unknown".to_string())
        })
    };

    // TASK-1454: cheap on every path (see doc comment) — computed
    // unconditionally, including on the notice-fast path, unlike
    // `orphaned_in_progress` above.
    let blocked_seats = collect_blocked_seat_items(project_root);

    awaiting_you::AwaitingReport {
        mergeable_prs,
        recusal_holds,
        held_prs,
        unowned_failing_prs,
        rework_ready,
        stale_approvals,
        blocked_reviews,
        pending_briefs,
        findings_total,
        reviewer_queue_items,
        escalations,
        mail,
        worker_directives,
        cron,
        shelved_total,
        unshipped_work,
        unshipped_work_scan,
        nightly_red,
        pr_attribution_disagreements,
        orphaned_in_progress,
        blocked_seats,
        // trace:BUG-1530 | ai:claude — the seat this session reads as, so the
        // headline (render) and per-turn compact line can scope themselves
        // to channels this seat can act on.
        role: ctx.role.clone(),
    }
}

/// STORY-741: render the unified "Awaiting you" report as a first-class
/// command. Two shapes:
///   - `--notice`: the COMPACT one-line per-turn signal the `UserPromptSubmit`
///     hook injects. Silent when nothing awaits. Cache/local-backed and
///     network-free — it forces the `no_ci` path (so the gh-backed PR probe
///     never runs) and builds a lightweight context (role from env, no
///     full-store load), so a hook firing every prompt stays cheap and
///     fail-open.
///   - default: the full multi-channel report (text / `--json`), including the
///     gh-backed PR channel unless `--no-ci`. Mirrors `aida status --awaiting`.
// trace:STORY-741 | ai:claude
/// STORY-769: emit the always-on "Current date/time + Timing" leading line for
/// `aida awaiting --notice`, and stamp the per-session last-human-input oracle.
/// Called EARLY in dispatch (before store init) so the line survives even when
/// the store/backend can't be resolved — the per-turn hook always gets its time
/// context and the escalation cascade always gets a fresh presence stamp.
///
/// The UserPromptSubmit / SessionStart hook relay passes its JSON payload in
/// `AIDA_HOOK_PAYLOAD` (`session_id`, `hook_event_name`). The command itself
/// must never read stdin: scripted callers may attach an open pipe whose writer
/// outlives this process. Missing or malformed hook metadata simply renders the
/// time line with no session key. Fail-open throughout.
pub(crate) fn emit_notice_time_line() {
    // trace:BUG-1239 | ai:codex
    let (session_id, is_session_start) = std::env::var("AIDA_HOOK_PAYLOAD")
        .ok()
        .map(|payload| presence::parse_hook_payload(&payload))
        .unwrap_or((None, false));
    if is_session_start {
        let project_root = std::env::current_dir()
            .ok()
            .and_then(|cwd| find_aida_project_root_from(&cwd).ok())
            .map(|root| main_worktree_root_from(&root));
        if let Some(project_root) = project_root {
            let binary = agent_registry::AgentBinaryIdentity::new(
                env!("CARGO_PKG_VERSION").to_string(),
                env!("AIDA_BUILD_GIT_SHA").to_string(),
            );
            let _ = agent_registry::touch_session_start_agent(
                &project_root,
                session_id.as_deref(),
                &binary,
            );
        }
    }
    let now = chrono::Local::now();
    let label = presence::stamp_turn_clock(
        session_id.as_deref(),
        is_session_start,
        now.with_timezone(&chrono::Utc),
    );
    // Local time, matching the trial hook's `%A %Y-%m-%d %H:%M %Z` shape.
    let when = now.format("%A %Y-%m-%d %H:%M %Z").to_string();
    println!("{}", format_notice_time_line(&when, &label));
    // STORY-1464: seat rotation signal — when this session's latest model call
    // crossed the context ceiling, tell the seat to hand off and restart.
    // Tail-read of the hook's transcript only; silent below the ceiling.
    // trace:STORY-1464 | ai:claude
    if let Ok(payload) = std::env::var("AIDA_HOOK_PAYLOAD") {
        let seat = std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| canonical_role_name(&s))
            .unwrap_or_else(|| "<seat>".to_string());
        if let Some(line) = seat_rotation::notice_from_hook_payload(&payload, &seat) {
            println!("{line}");
        }
    }
}

/// Enforce the per-turn notice's fail-open latency contract across the whole
/// dispatch path. Most notice reads are deliberately cheap, but cache refresh,
/// lease discovery, or an unusually large protocol store can still stall after
/// this early dispatch point. A detached watchdog bounds all of those paths and
/// exits successfully because the notice is advisory. Arm this before the
/// time-line/session bookkeeping so that work is covered by the same bound.
// trace:BUG-1239 | ai:codex
// trace:TASK-1274 | ai:claude
pub(crate) fn arm_notice_deadline() {
    let Some(deadline) = notice_deadline() else {
        return;
    };
    std::thread::spawn(move || {
        std::thread::sleep(deadline);
        std::process::exit(0);
    });
}

/// The per-turn notice's fail-open bound. TASK-1274 traced a flaky leased-spec
/// reminder to its full-store protocol lookup racing this watchdog. BUG-1569
/// replaces that scan with targeted/cache-indexed reads, allowing the product
/// bound to remain meaningfully sub-second. The override exists only so
/// black-box tests can assert tighter latency budgets in their subprocesses.
pub(crate) fn notice_deadline() -> Option<std::time::Duration> {
    // Test-only escape hatch; a production caller never sets this. Zero
    // disables the watchdog so lifecycle/content tests can synchronize on the
    // command's completion instead of racing a wall-clock deadline under CI
    // load. Latency behavior remains covered by its dedicated black-box test.
    // trace:BUG-1567 | ai:codex
    if let Ok(ms) = std::env::var("AIDA_TEST_NOTICE_DEADLINE_MS") {
        if let Ok(ms) = ms.parse::<u64>() {
            return (ms != 0).then(|| std::time::Duration::from_millis(ms));
        }
    }
    const PRODUCT_NOTICE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(750);
    Some(PRODUCT_NOTICE_DEADLINE)
}

/// BUG-1288: the wall-clock budget [`collect_unshipped_work_items_bounded`]
/// (and the `gh` probe it drives, [`collect_pr_head_state_snapshot_bounded`])
/// may spend probing candidate branches from `aida awaiting --json` / `aida
/// status --full`. Same escape-hatch shape as [`notice_deadline`]: a
/// production caller never sets the override.
// trace:BUG-1288 | ai:claude
pub(crate) fn unshipped_work_scan_budget() -> std::time::Duration {
    if let Ok(ms) = std::env::var("AIDA_TEST_UNSHIPPED_SCAN_BUDGET_MS") {
        if let Ok(ms) = ms.parse::<u64>() {
            return std::time::Duration::from_millis(ms);
        }
    }
    // BUG-1756: raised from BUG-1288's 2000ms. That number was sized when
    // the scan's dominant cost was one `gh pr list --head` network round
    // trip per candidate, and it protected poll latency by truncating —
    // which on a working repository (24 candidates, ~0.5s per gh call)
    // truncated at 0/N on every single run, leaving the channel effectively
    // dead. With the per-candidate network calls now lazy (survivors only),
    // the budget's job is to cover the CHEAP local pass (~0.2s of git per
    // candidate on this class of host, measured under its normal multi-agent
    // build load) plus a handful of survivor queries; 10s covers the
    // observed 26-candidate population at roughly 2× headroom, while still
    // bounding a pathological repo. The per-turn notice path skips this
    // channel entirely, so this budget never touches per-turn latency, and
    // the full `awaiting` report it rides already spends ~30s on its other
    // forge-backed channels. trace:BUG-1756 | ai:claude
    const PRODUCT_UNSHIPPED_SCAN_BUDGET: std::time::Duration =
        std::time::Duration::from_millis(10_000);
    PRODUCT_UNSHIPPED_SCAN_BUDGET
}

/// PURE: the notice's always-on leading line. Separated so the exact contract
/// shape is unit-testable without a clock or stdin.
pub(crate) fn format_notice_time_line(when: &str, label: &str) -> String {
    format!("Current date/time: {when}. Timing: {label}.")
}

pub(crate) fn handle_awaiting_command(
    notice: bool,
    json: bool,
    verbose: bool,
    no_ci: bool,
    backend: &aida_core::CachedGitBackend,
) -> Result<()> {
    let project_root = std::env::current_dir()?;

    if notice {
        // CHEAP per-turn path: NO full-store load, NO gh/network. Mail, briefs,
        // findings, worker directives and escalations are all cache/local
        // reads; PRs (gh) and the
        // reviewer-verdict channel (which needs the queue snapshot off the
        // loaded store) are deliberately omitted here — run bare `aida awaiting`
        // for those. The lightweight context carries only the role (from env),
        // so `collect_awaiting_report` sees an empty queue head and skips the
        // reviewer channel. Fail-open: a caught-up inbox prints nothing.
        let role = std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let ctx = UserStatusContext {
            session: None,
            role,
            branch: None,
            pr: None,
            queue_head: Vec::new(),
            queue_total: 0,
            agents: Vec::new(),
        };
        let report = collect_awaiting_report_inner(&project_root, backend, &ctx, true, true);
        if let Some(line) = report.compact_line() {
            println!("{line}");
        }
        // Re-teach a compacted session its type contract while this worktree
        // still holds the spec lease. `queue done` removes/releases that lease,
        // so the reminder naturally disappears. Fail-open on every read.
        // trace:STORY-1221 | ai:codex
        let canonical_cwd = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.clone());
        let lease_root = find_main_worktree_root().unwrap_or_else(|_| project_root.clone());
        if let Some(scope) = list_leases(&lease_root)
            .into_iter()
            .find(|lease| canonical_cwd.starts_with(&lease.worktree_path))
            .map(|lease| lease.scope)
        {
            match protocol_cmd::targeted_notice_line_for_scope(backend, &scope) {
                Ok(Some(line)) => {
                    println!("{line}");
                }
                Ok(None) => {}
                Err(err) => {
                    eprintln!("warning: unable to resolve protocol notice for {scope}: {err:#}")
                }
            }
        }
        if let Some(line) = mailbox_latency_warning(&project_root, ctx.role.as_deref()) {
            println!("{line}");
        }
        let store_path = detect_distributed_store_from(&project_root)
            .unwrap_or_else(|| project_root.join(".aida-store"));
        if let Some(line) = status_cmd::absence_notice_line(&project_root, &store_path) {
            println!("{line}");
        }
        return Ok(());
    }

    // Full view: reuse the exact same machinery as `aida status --awaiting`.
    let store = backend.load()?;
    let ctx = collect_user_context(&project_root, &store, backend, no_ci);
    let report = collect_awaiting_report(&project_root, backend, &ctx, no_ci);
    if json {
        println!("{}", crate::cache_output::json_pretty(&report.to_json())?);
    } else if agent_output_mode() {
        // BUG-695: honor AIDA_AGENT_OUTPUT like `aida integrate`/`ps`/`status` —
        // emit token-efficient TOON (flat scalars + uniform tables) instead of the
        // human box-drawing header + status emoji. `--json` above still wins for
        // structured consumers; the `--notice` compact path returned earlier.
        //
        // BUG-1739: the body moved into `AwaitingReport::render_agent_view` so a
        // test can assert what it emits. While it was inline here, nine of the
        // contributors `total()` counts drifted out of it with no test able to
        // see that, and `awaiting: 6` printed above eight empty tables.
        // trace:BUG-695 | ai:claude
        // trace:BUG-1739 | ai:claude
        print!("{}", report.render_agent_view());
    } else if report.is_empty() {
        println!("{}", "─── Awaiting you (0) ───".bold().dimmed());
        println!("  Nothing awaits you right now.");
        println!();
    } else {
        let stdout = std::io::stdout();
        let _ = report.render(verbose, stdout.lock());
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_465_awaiting_report_tests.rs"]
mod story_465_awaiting_report_tests;

#[cfg(test)]
#[path = "tests/task_515_status_agent_lease_fallback_tests.rs"]
mod task_515_status_agent_lease_fallback_tests;

pub(crate) fn print_status_hygiene_section(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
    verbose: bool,
    no_hygiene: bool,
) -> Result<()> {
    if no_hygiene {
        return Ok(());
    }

    let mut findings = collect_doctor_findings(project_root, store, None)?;

    // Filter findings
    if !verbose {
        findings.retain(|f| {
            matches!(
                f.category.as_str(),
                "spec-status-drift" | "stale-locks" | "orphan-worktrees" | "orphan-branches"
            )
        });
    }

    // TASK-673: a one-line count of Completed specs git can't corroborate. The
    // full listing + remediation lives in `aida doctor`; here we just surface
    // the integrity tripwire so the substrate polices its own completion claim
    // even on a quick `aida status`. Honors AIDA_DOCTOR_COMPLETED_SINCE for the
    // legacy-history exemption. trace:TASK-673 | ai:claude
    let integrity_since = std::env::var("AIDA_DOCTOR_COMPLETED_SINCE")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let uncorroborated =
        scan_completed_without_commit(project_root, store, integrity_since.as_deref()).len();

    if findings.is_empty() && uncorroborated == 0 {
        return Ok(());
    }

    // Sort findings by priority: spec-status-drift > stale-locks > orphan-worktrees > orphan-branches > others
    fn category_priority(cat: &str) -> usize {
        match cat {
            "spec-status-drift" => 0,
            "stale-locks" => 1,
            "orphan-worktrees" => 2,
            "orphan-branches" => 3,
            _ => 4,
        }
    }

    findings.sort_by(|a, b| {
        category_priority(&a.category)
            .cmp(&category_priority(&b.category))
            .then_with(|| a.category.cmp(&b.category))
            .then_with(|| a.id.cmp(&b.id))
    });

    let total = findings.len();
    let limit = if verbose { total } else { 2 };

    println!("{}", "─── Hygiene ───".bold());

    for finding in findings.iter().take(limit) {
        let heal_mode = if finding.safe_heal { "safe" } else { "manual" };
        println!(
            "  {} {}",
            crate::glyph(crate::glyphs::Glyph::Bullet),
            finding.category.cyan()
        );
        println!("    - {} [{}]", finding.summary, heal_mode.dimmed());
        println!("      {} {}", "→".yellow(), finding.action.dimmed());
    }

    if total > limit {
        let overflow = total - limit;
        println!(
            "    {} +{} more (run aida doctor)",
            crate::glyph(crate::glyphs::Glyph::Bullet),
            overflow
        );
    }

    if uncorroborated > 0 {
        println!(
            "  {} {} Completed spec{} with no corroborating commit (run {})",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            uncorroborated,
            if uncorroborated == 1 { "" } else { "s" },
            "aida doctor --category completed-without-commit".cyan()
        );
    }
    println!();

    Ok(())
}

/// True for summaries that represent real, user-authored requirements — i.e.
/// the rows `aida list` shows by default. META rows (AI-prompt customization
/// seeded by `aida init`, e.g. META-001..006) are plumbing, not work, and are
/// hidden from `aida list`'s default view (BUG-27). The `aida status`
/// Requirements panel must apply the same exclusion so its Total / per-status
/// counts reconcile with `aida list` instead of reporting phantom Draft rows
/// on a fresh store (BUG-415).
///
/// `req_type` is the cache's stored Debug form of `RequirementType`
/// (e.g. "Meta", "Task", "Bug"); the comparison is case-insensitive to be
/// robust against casing drift in the projection.
// trace:BUG-415 | ai:claude
pub(crate) fn is_real_requirement_summary(req_type: &str) -> bool {
    !req_type.eq_ignore_ascii_case("meta")
}

#[cfg(test)]
#[path = "tests/bug415_status_count_tests.rs"]
mod bug415_status_count_tests;

/// Whether a command MUTATES the shared store / queue (so a shared `"default"`
/// identity in a team context is genuinely hazardous — collides queues +
/// attribution). Reads only get a warning; writes can be refused behind
/// `AIDA_TEAM_REQUIRE_USER=1`. Conservative: anything not clearly a read is a
/// write. trace:STORY-640 | ai:claude
pub(crate) fn is_write_command(command: &Command) -> bool {
    matches!(
        command,
        Command::Do { .. }
            | Command::Add { .. }
            | Command::Edit { .. }
            | Command::Comment(_)
            | Command::Queue(_)
            | Command::Session(_)
            | Command::Defer { .. }
            | Command::Undefer { .. }
            | Command::Archive { .. }
            | Command::Unarchive { .. }
            | Command::Brief { .. }
    )
}

/// STORY-640: the team distinct-identity guard + join-the-team onboarding hint.
/// Both key off the shared node roster and are silent outside a team context.
/// Best-effort — never returns an error except the explicit opt-in refusal.
/// trace:STORY-640 | ai:claude
pub(crate) fn maybe_team_identity_guard(
    store_path: &std::path::Path,
    command: &Command,
) -> Result<()> {
    // `aida team` surfaces the same facts; the JSON status path must stay clean
    // for machine consumers. Skip both so the guard never pollutes them.
    if matches!(command, Command::Team { .. }) {
        return Ok(());
    }
    if let Command::Status { json: true, .. } = command {
        return Ok(());
    }
    // STORY-764: `--format json` pins the same machine surface; keep it clean too.
    // trace:STORY-764 | ai:claude
    if matches!(command, Command::Status { .. }) && output_format_is_json() {
        return Ok(());
    }

    let our_clone = team::our_clone_path(store_path);
    let registry = team::load_roster(store_path);
    let team_context = team::is_team_context(&registry, &our_clone);
    if !team_context {
        return Ok(()); // solo / single-own-node — no nag, ever.
    }

    // Onboarding: a fresh clone that joined an existing roster but never
    // acquired its own node id. One-time per command; a light hint, not a
    // wizard. Suppressible idempotently via a marker so we don't nag forever.
    if !team::clone_is_registered(&registry, &our_clone) {
        let project_root = store_path.parent().unwrap_or(store_path);
        let marker = project_root.join(".aida").join("team-onboarded");
        if !marker.exists() {
            let bullet = crate::glyph(crate::glyphs::Glyph::Bullet);
            eprintln!(
                "{} You've joined a shared AIDA store with {} other node(s). To keep your \
                 work attributable + your queue separate:\n  {bullet} set a distinct identity:  {}\n  \
                 {bullet} claim a node id:           {}",
                "Welcome:".cyan().bold(),
                registry.nodes.len(),
                "export AIDA_USER=<your-name>".cyan(),
                "aida node acquire".cyan(),
            );
            eprintln!();
            // Best-effort one-time suppression (dir may not exist yet).
            if let Some(parent) = marker.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&marker, "1\n");
        }
    }

    // Distinct-identity guard (BUG-89 hardening).
    let user_id = current_user_id(None);
    if team::identity_verdict(&user_id, team_context) == team::IdentityVerdict::DefaultInTeam {
        let require = std::env::var("AIDA_TEAM_REQUIRE_USER")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let writes = is_write_command(command);
        eprintln!(
            "{} This is a shared (team) store, but no distinct identity is set — you are \
             the BUG-89 '{}' fallback. A shared '{}' id collides queues + commit \
             attribution across machines.\n  Fix it for this shell:  {}",
            "warning:".yellow().bold(),
            "default".bold(),
            "default".bold(),
            "export AIDA_USER=<your-name>".cyan(),
        );
        if require && writes {
            anyhow::bail!(
                "refusing a write op on a team store with the shared '{}' identity \
                 (AIDA_TEAM_REQUIRE_USER=1). Set a distinct AIDA_USER, or unset \
                 AIDA_TEAM_REQUIRE_USER to downgrade this to a warning.",
                "default"
            );
        }
        eprintln!();
    }
    Ok(())
}

/// `aida identity link <a> <b>` (TASK-845): link two identity strings as one
/// canonical person in `registry/aliases.toml` on the store (CAS push). After
/// this, the queue / team roster / block list all resolve both strings to one
/// person. Idempotent — a redundant link reports "already linked" and commits
/// nothing.
// trace:TASK-845 | ai:claude
pub(crate) fn handle_identity_link(store_path: &std::path::Path, a: &str, b: &str) -> Result<()> {
    let fa = aida_core::node::canonical_user_id(a);
    let fb = aida_core::node::canonical_user_id(b);
    if fa.is_empty() || fb.is_empty() {
        anyhow::bail!("both identity strings must be non-empty");
    }
    if fa == fb {
        println!(
            "{} {} and {} are the same identity (after case-fold) — nothing to link.",
            "No change:".yellow().bold(),
            a.bold(),
            b.bold()
        );
        return Ok(());
    }
    let changed = aida_core::alias::link_cas(store_path, a, b)
        .map_err(|e| anyhow::anyhow!("could not write the identity link: {e}"))?;
    // Resolve to show the canonical person the two now share.
    let registry = aida_core::alias::AliasRegistry::load(store_path);
    let canonical = registry.resolve(a);
    if changed {
        println!(
            "{} {} {} {}  (canonical person: {})",
            "Linked:".green().bold(),
            a.bold(),
            crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
            b.bold(),
            canonical.cyan().bold()
        );
        println!(
            "  {}",
            "The queue, `aida team`, and `aida db block list` now resolve both to one person."
                .dimmed()
        );
    } else {
        println!(
            "{} {} and {} already resolve to one person ({}).",
            "Already linked:".yellow().bold(),
            a.bold(),
            b.bold(),
            canonical.cyan()
        );
    }
    Ok(())
}

/// `aida identity list` (TASK-845): show the recorded person links — one block
/// per canonical person with the aliases that resolve to them.
// trace:TASK-845 | ai:claude
pub(crate) fn handle_identity_list(store_path: &std::path::Path, json: bool) -> Result<()> {
    let registry = aida_core::alias::AliasRegistry::load(store_path);
    let people = registry.people();
    if json {
        let rows: Vec<serde_json::Value> = people
            .iter()
            .map(|(canonical, aliases)| {
                serde_json::json!({ "person": canonical, "aliases": aliases })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!(rows))?
        );
        return Ok(());
    }
    if people.is_empty() {
        println!("{}", "Identity links".bold());
        println!();
        println!(
            "  No links yet. Link a person's identity strings with {}.",
            "aida identity link <a> <b>".cyan()
        );
        return Ok(());
    }
    println!("{}", "Identity links".bold());
    println!();
    for (canonical, aliases) in &people {
        println!("  {}", canonical.cyan().bold());
        for alias in aliases {
            println!(
                "    {} {}",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                alias
            );
        }
    }
    println!();
    println!(
        "  {}",
        "Each block is one person; the listed aliases all resolve to the canonical id.".dimmed()
    );
    Ok(())
}

/// `aida identity show <id>` (TASK-845): show the canonical person an identity
/// resolves to (case-fold then alias-resolve) plus every alias that shares it.
// trace:TASK-845 | ai:claude
pub(crate) fn handle_identity_show(
    store_path: &std::path::Path,
    id: &str,
    json: bool,
) -> Result<()> {
    let registry = aida_core::alias::AliasRegistry::load(store_path);
    let canonical = registry.resolve(id);
    let members = registry.members_of(id);
    if json {
        let out = serde_json::json!({
            "input": id,
            "canonical": canonical,
            "members": members,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!(
        "{} {} {} {}",
        format!("{}", id).bold(),
        "→".dimmed(),
        "canonical person:".dimmed(),
        canonical.cyan().bold()
    );
    if members.len() > 1 {
        println!("  {}", "aliases:".dimmed());
        for m in &members {
            let marker = if *m == canonical {
                " (canonical)".dimmed().to_string()
            } else {
                String::new()
            };
            println!(
                "    {} {}{}",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                m,
                marker
            );
        }
    } else {
        println!(
            "  {}",
            "Not linked to any other identity — resolves to itself.".dimmed()
        );
    }
    Ok(())
}

// STORY-673: compose the terse one-line requirement breakdown for default
// `aida status`. Leads with OPEN work (what's actionable — every status that
// isn't a terminal Completed/Rejected), then the closed tallies, then a pointer
// to the per-status detail. Pure (no I/O) so the copy is unit-testable.
// trace:STORY-673 | ai:claude
pub(crate) fn requirement_breakdown_summary_line(
    by_status: &std::collections::BTreeMap<String, usize>,
) -> String {
    let is_open_lens_status = |s: &str| status_is_open_lens_work(s);
    let completed = by_status.get("Completed").copied().unwrap_or(0);
    let rejected = by_status.get("Rejected").copied().unwrap_or(0);

    // Open = the same status set as `aida list open`, with a compact per-status
    // tail in deterministic (BTreeMap) order.
    let open_total: usize = by_status
        .iter()
        .filter(|(s, _)| is_open_lens_status(s))
        .map(|(_, n)| *n)
        .sum();
    let open_parts: Vec<String> = by_status
        .iter()
        .filter(|(s, _)| is_open_lens_status(s))
        .map(|(s, n)| format!("{n} {}", s.to_ascii_lowercase()))
        .collect();

    let mut line = if open_parts.is_empty() {
        "0 open".to_string()
    } else {
        format!("{} open ({})", open_total, open_parts.join(" · "))
    };
    if completed > 0 {
        line.push_str(&format!(" · {completed} completed"));
    }
    if rejected > 0 {
        line.push_str(&format!(" · {rejected} rejected"));
    }
    line.push_str(" · `aida status --full` for the per-status breakdown");
    line
}

/// `aida status` cross-clone coordination view (STORY-640, coordination slice
/// 3): the ACTIVE `coordination/` claims (leases + drain + solo) held across
/// all clones — distinct from the LOCAL leases section. Silent when there are
/// no claims. trace:STORY-640 | ai:claude
///
/// BUG-1764: this section is also reached by `aida doctor` (which forces
/// `show_full`). It applied no expiry predicate and — unlike the lease section
/// — does not filter by clone at all, so it reported EVERY claim file as
/// active: measured on one host, 211 claims, the oldest aged 8_078_324s (93
/// days) against its own 1800s TTL, and the collapsed arm printed "211 active
/// claims" on a host holding none. Stale claims are now excluded from both
/// arms, and the count line says how many were suppressed.
// trace:BUG-1764 | ai:claude
pub(crate) fn print_status_coordination_section(
    store_root: &std::path::Path,
    now: chrono::DateTime<chrono::Utc>,
    show_full: bool,
) {
    let mut all_claims = coordination::list_claims(store_root);
    all_claims.extend(coordination::list_lock_claims(store_root));
    if all_claims.is_empty() {
        return;
    }
    // BUG-1764: "active" is a claim the staleness predicate presumes live. The
    // drain/solo lock claims mixed in here ARE `process_backed`, so they also
    // get the same-host pid evaluation; session leases are not, so the TTL
    // governs them. See `coordination::claim_staleness`.
    let (claims, stale) =
        coordination::partition_claims(all_claims, now, &coordination::hostname());
    let hidden = if stale.is_empty() {
        String::new()
    } else {
        format!(
            " · {} stale hidden (`aida session leases --all`)",
            stale.len()
        )
    };
    if claims.is_empty() {
        // Every claim on the store is stale. Say so — silence here would read
        // as "no coordination state", which is what BUG-1764 made impossible
        // to distinguish from "211 claims, all dead".
        println!("{}", "─── Cross-clone coordination ───".bold());
        // Point at the LISTING, not at `--prune-stale`: that command releases
        // only CROSS-CLONE claims (clause 4's scope), while this count also
        // includes this clone's own expired claim files — whose garbage
        // collection is BUG-1764 clause 5, deliberately out of scope.
        println!(
            "  no active claims · {} stale claim{} on the store · {} to inspect",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" },
            "aida session leases --all".cyan()
        );
        println!();
        return;
    }
    println!("{}", "─── Cross-clone coordination ───".bold());
    // STORY-673: the cross-clone claim table is a fleet long-tail — collapse to
    // a one-line count by default; `--full` / `--all` prints the table.
    // trace:STORY-673 | ai:claude
    if !show_full {
        let scopes: Vec<&str> = claims.iter().take(3).map(|c| c.scope.as_str()).collect();
        let preview = scopes.join(", ");
        let suffix = if claims.len() > scopes.len() {
            " …"
        } else {
            ""
        };
        println!(
            "  {} active claim{}: {}{} · `aida status --full`{}",
            claims.len(),
            if claims.len() == 1 { "" } else { "s" },
            preview,
            suffix,
            hidden
        );
        println!();
        return;
    }
    println!();
    println!(
        "  {:<16} {:<10} {:<14} {:<14} age",
        "scope", "host", "agent", "node"
    );
    for c in &claims {
        let age = chrono::DateTime::parse_from_rfc3339(&c.heartbeat_at)
            .ok()
            .map(|t| {
                let secs = now
                    .signed_duration_since(t.with_timezone(&chrono::Utc))
                    .num_seconds()
                    .max(0);
                format!("{secs}s")
            })
            .unwrap_or_else(|| "?".to_string());
        println!(
            "  {:<16} {:<10} {:<14} {:<14} {}",
            truncate(&c.scope, 16),
            truncate(&c.host, 10),
            truncate(if c.agent.is_empty() { "-" } else { &c.agent }, 14),
            truncate(&c.node_id, 14),
            age,
        );
        if !c.clone_path.is_empty() {
            println!("    {}", c.clone_path.dimmed());
        }
    }
    if !stale.is_empty() {
        println!(
            "  {} stale claim{} hidden · {} to inspect",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" },
            "aida session leases --all".cyan()
        );
    }
    println!();
}

/// The cache-sourced inputs the fast `aida status` snapshot renders. Kept a
/// plain data struct — assembled by [`collect_fast_status_snapshot`] (which does
/// the cache/git/queue reads) and counted by [`fast_status_counts`] (a pure
/// function) — so the "no network / no full load" contract is unit-testable
/// without spawning git or `gh`.
// trace:STORY-707 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct FastStatusSnapshot {
    pub(crate) role: String,
    pub(crate) role_is_default: bool,
    pub(crate) mcp_authority_lines: Vec<String>,
    pub(crate) branch: Option<String>,
    pub(crate) queue_depth: usize,
    /// STORY-723: of `queue_depth` items routed to this role, how many are
    /// ACTUALLY actionable (live + workable status). The raw depth is padded
    /// with archived/completed/deferred corpses; this is the reconcilable count.
    pub(crate) queue_actionable: usize,
    pub(crate) counts: FastStatusCounts,
    /// Per-status breakdown of the same row set `counts` is tallied from —
    /// the monitor-contract `requirements.by_status` field (BUG-1503).
    pub(crate) by_status: std::collections::BTreeMap<String, usize>,
    pub(crate) cache_present: bool,
}

/// Key requirement tallies for the fast snapshot, derived purely from the
/// cache's `(status, req_type, deferred)` rows (non-archived only). `open` =
/// the same actionable status/view lens as `aida list open`.
// trace:STORY-707 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FastStatusCounts {
    pub(crate) open: usize,
    pub(crate) in_progress: usize,
    pub(crate) draft: usize,
    pub(crate) total: usize,
}

pub(crate) fn status_is_open_lens_work(status: &str) -> bool {
    let normalized_status = status.trim().replace(['-', '_'], "").to_ascii_lowercase();
    aida_core::RequirementStatus::open_statuses()
        .iter()
        .any(|s| s.cache_key().to_ascii_lowercase() == normalized_status)
}

/// Pure counter over non-archived, non-deferred cache rows: each tuple is
/// `(status, req_type)`. META and other standing-artifact types are excluded
/// the same way `aida list` hides them, so the tallies agree with the
/// list/triage surfaces. Status matching is case-insensitive (the cache stores
/// the Debug form, e.g. "InProgress"/"Draft").
// trace:STORY-707 | ai:claude
#[cfg(test)]
pub(crate) fn fast_status_counts<'a>(
    rows: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> FastStatusCounts {
    fast_status_counts_with_defer(
        rows.into_iter()
            .map(|(status, req_type)| (status, req_type, false, "[]")),
    )
}

/// Pure counter over non-archived cache rows carrying the deferred view flag.
/// This is the exact fast-status analogue of `aida list open`: deferred rows
/// are parked work and must not inflate the headline `open` scalar.
// trace:BUG-1155 | ai:codex
pub(crate) fn fast_status_counts_with_defer<'a>(
    rows: impl IntoIterator<Item = (&'a str, &'a str, bool, &'a str)>,
) -> FastStatusCounts {
    let mut c = FastStatusCounts::default();
    for (status, req_type, deferred, tags_json) in rows {
        if !is_real_requirement_summary(req_type) || is_standing_artifact_type(req_type) {
            continue;
        }
        // STORY-584: match the cache list lens: deferred means the flag is set
        // OR a legacy `deferred:*` parking tag is present in tags_json.
        // trace:BUG-1155 | ai:codex
        if deferred || tags_json.contains("\"deferred:") {
            continue;
        }
        c.total += 1;
        // BUG-781: an accepted decision (a `decision` spec at `Approved`) is
        // that class's TERMINAL state, so it must not inflate the open backlog
        // the same way a not-yet-started task at Approved does — the identical
        // rule the default `aida list` lens applies. trace:BUG-781 | ai:claude
        if status_is_open_lens_work(status)
            && !aida_core::lifecycle::is_accepted_decision(req_type, status)
        {
            c.open += 1;
        }
        if status.eq_ignore_ascii_case("inprogress")
            || status.eq_ignore_ascii_case("in-progress")
            || status.eq_ignore_ascii_case("in_progress")
        {
            c.in_progress += 1;
        }
        if status.eq_ignore_ascii_case("draft") {
            c.draft += 1;
        }
    }
    c
}

/// The BUG-1503 monitor-contract companion to [`fast_status_counts_with_defer`]:
/// the SAME row set, the SAME real/standing-artifact/deferred exclusions (so
/// the values sum to exactly `counts.total`), grouped by the raw cache
/// `status` string instead of tallied into the fixed scalar buckets. Keys use
/// the cache's stored Debug-form casing (e.g. "InProgress", "Draft") — the
/// same strings the heavy text panel's `by_status` breakdown groups on
/// (`status_cmd.rs`'s `s.status.clone()`), so a monitor consumer sees
/// identical keys regardless of which status surface it polls.
// trace:BUG-1503 | ai:claude
pub(crate) fn fast_status_by_status_with_defer<'a>(
    rows: impl IntoIterator<Item = (&'a str, &'a str, bool, &'a str)>,
) -> std::collections::BTreeMap<String, usize> {
    let mut by_status: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    for (status, req_type, deferred, tags_json) in rows {
        if !is_real_requirement_summary(req_type) || is_standing_artifact_type(req_type) {
            continue;
        }
        if deferred || tags_json.contains("\"deferred:") {
            continue;
        }
        *by_status.entry(status.to_string()).or_insert(0) += 1;
    }
    by_status
}

/// Read `(status, req_type)` for every non-archived row straight from the cache
/// DB (read-only sqlite), then count via [`fast_status_counts`]. This is the
/// same read-only cache `read_draft_backlog_depth` uses — NO `backend.load()`, no
/// git spawn. Returns zeroed counts (and an empty `by_status`) when the cache is
/// absent/unreadable (a fresh `aida init` with no reads yet). A single `SELECT`
/// of `requirements_cache` (grouped in Rust rather than SQL, so it can share
/// the exact real/standing-artifact/deferred exclusions the scalar counts
/// already apply) feeds both the scalar counts and the per-status
/// breakdown — one query against the cache, not two.
// trace:STORY-707 trace:BUG-1503 | ai:claude
pub(crate) fn fast_status_counts_from_cache(
    cache_path: &std::path::Path,
) -> (FastStatusCounts, std::collections::BTreeMap<String, usize>) {
    if !cache_path.exists() {
        return (FastStatusCounts::default(), Default::default());
    }
    let conn = match rusqlite::Connection::open_with_flags(
        cache_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(c) => c,
        Err(_) => return (FastStatusCounts::default(), Default::default()),
    };
    let has_deferred_column = conn
        .prepare("SELECT deferred FROM requirements_cache LIMIT 0")
        .is_ok();
    let has_tags_json_column = conn
        .prepare("SELECT tags_json FROM requirements_cache LIMIT 0")
        .is_ok();
    let sql = match (has_deferred_column, has_tags_json_column) {
        (true, true) => {
            "SELECT status, req_type, deferred, tags_json FROM requirements_cache WHERE archived = 0"
        }
        (true, false) => {
            "SELECT status, req_type, deferred, '[]' AS tags_json FROM requirements_cache WHERE archived = 0"
        }
        (false, true) => {
            "SELECT status, req_type, 0 AS deferred, tags_json FROM requirements_cache WHERE archived = 0"
        }
        (false, false) => {
            "SELECT status, req_type, 0 AS deferred, '[]' AS tags_json FROM requirements_cache WHERE archived = 0"
        }
    };
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => return (FastStatusCounts::default(), Default::default()),
    };
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)? != 0,
            row.get::<_, String>(3)?,
        ))
    });
    let Ok(rows) = rows else {
        return (FastStatusCounts::default(), Default::default());
    };
    let collected: Vec<(String, String, bool, String)> = rows.flatten().collect();
    let counts = fast_status_counts_with_defer(
        collected
            .iter()
            .map(|(s, t, d, tags)| (s.as_str(), t.as_str(), *d, tags.as_str())),
    );
    let by_status = fast_status_by_status_with_defer(
        collected
            .iter()
            .map(|(s, t, d, tags)| (s.as_str(), t.as_str(), *d, tags.as_str())),
    );
    (counts, by_status)
}

/// Assemble the fast snapshot from cache-cheap inputs: role from
/// `AIDA_SESSION_ROLE` (implementer default), current git branch (one cheap
/// `git symbolic-ref`), queue depth (the queue dir scan the statusline uses),
/// and key counts from the read-only cache. No `gh`, no full store load, no
/// live-session probe.
// trace:STORY-707 | ai:claude
pub(crate) fn collect_fast_status_snapshot(project_root: &std::path::Path) -> FastStatusSnapshot {
    let (role, role_is_default) = effective_role_resolved();
    let branch = current_branch_at(project_root);
    let queue_depth = read_queue_depth(project_root, Some(role.as_str())).unwrap_or(0);
    // STORY-723: the subset of the role queue that is actually actionable, so
    // the snapshot can reconcile "163 routed" against a far smaller live set.
    let queue_actionable = role_queue_actionable(project_root, &role).len();
    let mcp_authority_lines = agent_registry::mcp_authority_status_lines_for_project(project_root);
    let cache_path = project_root.join(".aida/cache.db");
    let cache_present = cache_path.exists();
    let (counts, by_status) = fast_status_counts_from_cache(&cache_path);
    FastStatusSnapshot {
        role,
        role_is_default,
        mcp_authority_lines,
        branch,
        queue_depth,
        queue_actionable,
        counts,
        by_status,
        cache_present,
    }
}

/// STORY-730: print the "since you were away" morning-after banner ABOVE the
/// human `aida status` snapshot when a recent, un-acknowledged drain outcome is
/// persisted at `.aida/last-drain.json`. The single highest-trust moment is "I
/// walked away — what happened?", and this is the first place that answers it.
/// No-op when there is no recent/non-empty/un-acknowledged outcome. The pause
/// marker routes through the glyph registry so an `ascii` profile re-renders it.
// trace:STORY-730 | ai:claude
pub(crate) fn print_morning_after_banner(project_root: &std::path::Path) {
    let Some(outcome) = last_drain::LastDrainOutcome::read(project_root) else {
        return;
    };
    let pause = crate::glyphs::get(crate::glyphs::Glyph::Pause, Some(project_root));
    if let Some(line) = outcome.banner(chrono::Utc::now(), pause) {
        println!("{}", line.yellow().bold());
        if outcome.findings_to_triage > 0 {
            println!("  {}", "drill down → `aida findings list`".dimmed());
        }
        println!();
    }
}

/// STORY-730: the AGENT/TOON mirror of [`print_morning_after_banner`] — a single
/// compact `last_drain` scalar line (no glyph) above the TOON status snapshot,
/// under the same suppression rules. No-op when nothing recent to report.
// trace:STORY-730 | ai:claude
pub(crate) fn print_morning_after_toon(project_root: &std::path::Path) {
    let Some(outcome) = last_drain::LastDrainOutcome::read(project_root) else {
        return;
    };
    if let Some(compact) = outcome.compact(chrono::Utc::now()) {
        println!("{}", crate::toon::scalar("last_drain", &compact));
    }
}

/// Render the fast `aida status` snapshot. One compact screen: role, branch,
/// queue depth, and the cache-sourced key counts, plus a footer pointing at the
/// rich/diagnostic surfaces (`--full`, `aida doctor`).
// trace:STORY-707 | ai:claude
pub(crate) fn print_fast_status(snap: &FastStatusSnapshot) {
    println!("{}", "─── Status ───".bold());
    let role_cell = if snap.role_is_default {
        format!("{} (default)", snap.role)
    } else {
        snap.role.clone()
    };
    println!("  {:<10} {}", "role:".bold(), role_cell.cyan());
    for line in &snap.mcp_authority_lines {
        println!("{line}");
    }
    match &snap.branch {
        Some(b) => println!("  {:<10} {}", "branch:".bold(), b.cyan()),
        None => println!(
            "  {:<10} {}",
            "branch:".bold(),
            "(not a git branch)".dimmed()
        ),
    }
    // STORY-723: label the queue count unambiguously. `queue_depth` is the
    // SHARED role-routed queue (which can be padded with archived/completed
    // corpses), NOT the caller's personal `aida queue list` — that mismatch
    // ("163 routed" vs "your queue is empty") was a top first-impression
    // confusion. Show how many of the routed items are actually actionable and
    // point at the personal queue. trace:STORY-723
    println!(
        "  {:<10} {} routed to role:{} ({} actionable) — shared role queue, not your personal queue",
        "queue:".bold(),
        snap.queue_depth,
        snap.role,
        snap.queue_actionable,
    );
    println!(
        "  {:<10} {}",
        "",
        "your personal queue: `aida queue list`".dimmed()
    );
    if let Some(line) = crate::intent_capture::status_intent_capture_line(
        &std::env::current_dir().unwrap_or_default(),
    ) {
        println!("{line}");
    }
    println!();

    println!("{}", "─── Requirements (cache) ───".bold());
    if snap.cache_present {
        let c = &snap.counts;
        // STORY-723: name the denominator so it reconciles with `aida list`.
        // `total` here counts every ACTIVE (non-archived) spec across all
        // statuses; bare `aida list` defaults to the OPEN subset of these.
        println!(
            "  {} open · {} in-progress · {} draft · {} total active (non-archived)",
            c.open, c.in_progress, c.draft, c.total
        );
    } else {
        println!(
            "  {}",
            "(no cache yet — run `aida list` to build it)".dimmed()
        );
    }
    println!();

    // Point at the heavy surfaces the fast default deliberately omits. The PR/CI,
    // live-session/lease liveness, worktree, roster, coordination, and hygiene
    // diagnostics moved to `aida doctor`; the full rich snapshot stays opt-in
    // behind `aida status --full`. trace:STORY-707 | ai:claude
    println!(
        "  {}",
        "PR/CI, agents, worktrees, hygiene → `aida doctor` · full snapshot → `aida status --full`"
            .dimmed()
    );
    println!();
}

/// The machine-readable twin of [`print_fast_status`]. Serializes the SAME
/// [`FastStatusSnapshot`] the human bare `aida status` prints — no extra
/// cache/git/`gh` reads — so `aida status --format json` (and `--json`)
/// return in the same order of magnitude as the human form instead of
/// silently falling through to the heavy `--full`-equivalent report. Emits
/// ONLY the JSON document on stdout (no banners/text before it) so the
/// output always parses.
//
// Fix note: the pre-existing bug_1289_format_json.rs contract
// (status_bare_format_json_parses) asserts the document carries either a
// `requirements` or an `agents` key — the shape the heavy
// print_status_json path produced when bare `--format json` used to fall
// through to it. The fast path has no live-agent roster to report (that
// moved to `aida doctor`), so it keeps the contract via `requirements`: the
// SAME cache-sourced `counts` already computed for the human view, at no
// extra cost. Plain `//` keeps the marker out of any doc/help.
// trace:BUG-1503 | ai:claude
//
// The monitor contract (`monitor_contract.rs` / `docs/monitor-contract-
// fixtures/status.json`) promises `requirements.total` (an integer) AND
// `requirements.by_status` (an object) from `aida status --json`. `counts`
// (the internal/duplicate scalar key) stays as-is; `requirements` gets the
// `by_status` breakdown alongside it so the contract holds on the fast path
// too, not just when it used to fall through to the heavy report.
// trace:BUG-1503 | ai:claude
pub(crate) fn print_fast_status_json(snap: &FastStatusSnapshot) -> Result<()> {
    let counts = serde_json::json!({
        "open": snap.counts.open,
        "in_progress": snap.counts.in_progress,
        "draft": snap.counts.draft,
        "total": snap.counts.total,
    });
    let mut requirements = counts.clone();
    requirements["by_status"] = serde_json::json!(snap.by_status);
    let out = serde_json::json!({
        "role": snap.role,
        "role_is_default": snap.role_is_default,
        "branch": snap.branch,
        "queue": {
            "depth": snap.queue_depth,
            "actionable": snap.queue_actionable,
        },
        "cache_present": snap.cache_present,
        "counts": counts,
        "requirements": requirements,
    });
    println!("{}", crate::cache_output::json_pretty(&out)?);
    Ok(())
}

/// Assemble the AGENT-MODE scalar head lines for `aida status` with a single,
/// unambiguous "is there work for me?" signal. `queue_actionable` LEADS: it is
/// the count that actually answers the question (live, workable, role-routed
/// specs) and is consistent with `aida queue list`. The raw `queue_depth` (the
/// role-routed queue-file scan) is the total routed count — larger than
/// actionable when the queue is padded with archived/completed/deferred corpses.
///
/// BUG-675: `queue_depth` now resolves user identity through the SAME two-step
/// path `aida queue list` uses (`current_user_id` + the `resolve_queue_user`
/// case-fold, TASK-951), so it is trustworthy — it no longer collapses to zero on
/// a case-only identity mismatch. That removes the reason for BUG-670's
/// divergence-suppression (which dropped the scalar whenever it disagreed with
/// `queue_actionable`, partly because the count could be a spurious zero). The
/// depth now ALWAYS rides along, AFTER the lead actionable signal, giving the
/// agent both "how many are workable now" and "how many are routed in total".
/// The human TTY path ([`print_fast_status`]) keeps its fully-labelled "N routed
/// (M actionable)" line unchanged. Pure, so the ordering contract is
/// unit-testable without a queue file or a cache DB.
// trace:BUG-675 trace:BUG-670 | ai:claude — plain `//` keeps the marker out of any doc/help.
pub(crate) fn toon_status_scalar_lines(snap: &FastStatusSnapshot) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    lines.push(crate::toon::scalar("role", &snap.role));
    lines.push(crate::toon::scalar(
        "role_default",
        &snap.role_is_default.to_string(),
    ));
    for line in &snap.mcp_authority_lines {
        lines.push(crate::toon::scalar("mcp_authority", line.trim()));
    }
    lines.push(crate::toon::scalar(
        "branch",
        snap.branch.as_deref().unwrap_or(""),
    ));
    // LEAD with the actionable count — the unambiguous work-signal an agent acts
    // on. STORY-723 introduced it as the reconcilable subset of the padded depth.
    lines.push(crate::toon::scalar(
        "queue_actionable",
        &snap.queue_actionable.to_string(),
    ));
    // BUG-675: the raw role-routed depth is now identity-trustworthy (same
    // resolver as `aida queue list`), so it always rides along AFTER the lead
    // actionable signal — no longer suppressed on divergence.
    lines.push(crate::toon::scalar(
        "queue_depth",
        &snap.queue_depth.to_string(),
    ));
    if snap.cache_present {
        lines.push(crate::toon::scalar("open", &snap.counts.open.to_string()));
        lines.push(crate::toon::scalar(
            "in_progress",
            &snap.counts.in_progress.to_string(),
        ));
        lines.push(crate::toon::scalar("draft", &snap.counts.draft.to_string()));
        lines.push(crate::toon::scalar("total", &snap.counts.total.to_string()));
    } else {
        lines.push(crate::toon::scalar("cache", "absent"));
    }
    lines
}

/// AGENT-MODE token-efficient TOON render of the fast status snapshot. Scalar
/// head fields (role / branch / the lead `queue_actionable` signal / the cache
/// counts) then a uniform TOON table of the top queued items for the active
/// role. No emoji, no section rules — the token-efficient analog of
/// [`print_fast_status`], which stays byte-identical for the human TTY path.
// trace:TASK-964 trace:BUG-670
pub(crate) fn print_toon_status(snap: &FastStatusSnapshot, project_root: &std::path::Path) {
    println!("{}", toon_status_scalar_lines(snap).join("\n"));

    // STORY-723: the queued[] table + the `next` head are the ACTIONABLE set
    // (archived/completed/deferred/blocked corpses filtered out), so an agent is
    // never nudged to `aida queue work <archived-spec>`.
    let actionable = role_queue_actionable(project_root, &snap.role);
    let top: Option<(String, String)> = actionable
        .first()
        .map(|(id, _title, status)| (id.clone(), status.clone()));
    let rows: Vec<Vec<String>> = actionable
        .into_iter()
        .take(AGENT_BARE_QUEUE_TOPN)
        .map(|(id, title, _status)| vec![id, title])
        .collect();
    println!(
        "{}",
        crate::toon::table_raw("queued", &["id", "title"], &rows)
    );
    // TASK-974 (AXI #9) / STORY-723: next-step block — lead with the fastest
    // thought-to-merged path for the actionable head (`aida zen` / `aida ship`),
    // else point at the approvable backlog. BUG-670: drive the nudge off the
    // ACTIONABLE count, never the padded/divergent `queue_depth`, so a
    // `fill-queue` hint only ever appears alongside `queue_actionable: 0` (never
    // next to a non-zero depth the agent can't reconcile). trace:TASK-974 trace:STORY-723 trace:BUG-670
    let next = crate::help_next::status_next(
        snap.queue_actionable,
        top.as_ref().map(|(id, st)| (id.as_str(), st.as_str())),
    );
    if let Some(block) = crate::help_next::render(&next) {
        println!("{block}");
    }
}

/// STORY-723: is `status` an ACTIONABLE lifecycle state — one a front-door
/// `next` nudge may safely point at? Live + workable: approved / planned /
/// in-progress / needs-attention. Draft (not yet approved), done (awaiting
/// merge), and the terminal states (completed / rejected / released) are NOT
/// actionable. Case- and spelling-tolerant (the cache stores the Debug form,
/// e.g. "InProgress" / "NeedsAttention").
pub(crate) fn is_actionable_queue_status(status: &str) -> bool {
    let s = status.to_ascii_lowercase().replace(['-', '_', ' '], "");
    matches!(
        s.as_str(),
        "approved" | "planned" | "inprogress" | "needsattention"
    )
}

/// STORY-723: pure predicate — is a queued requirement row (its cache fields)
/// safe to surface as the actionable head of the front-door `next` nudge? A row
/// qualifies only when it is NOT archived, NOT deferred, NOT blocked, and sits
/// in an actionable status. Kept side-effect-free so the "never nudge an
/// archived/completed corpse" invariant is unit-testable without a queue file or
/// a cache DB.
pub(crate) fn queue_row_actionable(
    status: &str,
    archived: i64,
    deferred: i64,
    blocked: i64,
) -> bool {
    archived == 0 && deferred == 0 && blocked == 0 && is_actionable_queue_status(status)
}

/// STORY-723: should bare `aida list` apply the default OPEN/actionable lens?
/// Only when no status filter is already in play and the view wasn't widened to
/// include closed/archived/deferred rows. Pure so the default is unit-testable.
pub(crate) fn list_default_open_lens(
    has_status: bool,
    all: bool,
    archived: bool,
    deferred: bool,
) -> bool {
    !has_status && !all && !archived && !deferred
}

/// BUG-788: is the requested status filter the explicit `open` alias
/// (`aida list open` / `aida list --status open`)? The `open` shortcut is a
/// second spelling of the bare-list default open lens, so it must share that
/// lens's decision-class exclusion (BUG-781: an accepted ADR is terminal). Bare
/// list clears the status axis and takes the default lens; the explicit `open`
/// shortcut instead SETS the status axis (to the open set), which turns the
/// default lens OFF — so without this the two verbs disagreed about whether an
/// accepted ADR is open work. Matches only the lone `open` token
/// (case-insensitive); a mixed spec like `open,closed` is a deliberately wider
/// ask and keeps the terminals visible. Pure so it's unit-testable.
// trace:BUG-788 | ai:claude
pub(crate) fn status_spec_is_open_alias(raw_status: Option<&str>) -> bool {
    match raw_status {
        Some(spec) => {
            let mut tokens = spec.split(',').map(str::trim).filter(|t| !t.is_empty());
            matches!((tokens.next(), tokens.next()),
                (Some(only), None) if only.eq_ignore_ascii_case("open"))
        }
        None => false,
    }
}

/// BUG-1498: only the single explicit `draft` status activates the advisor's
/// human-first draft lens. A mixed status expression is a broader operational
/// query and must not silently lose machine-filed rows.
// trace:BUG-1498 | ai:codex
pub(crate) fn status_spec_is_exact_draft(raw: &str) -> bool {
    raw.trim().eq_ignore_ascii_case("draft")
}

pub(crate) fn is_machine_filed_draft(r: &aida_core::RequirementSummary) -> bool {
    // One definition shared with the capture-coverage report.
    // trace:STORY-1487 | ai:claude
    criteria_coverage::is_auto_drafted(&r.tags, &r.description)
}

/// Partition a draft grooming query by provenance. Returns the number hidden
/// from the human-first view so renderers can advertise the escape hatch.
// trace:BUG-1498 | ai:codex
pub(crate) fn apply_machine_draft_lens(
    reqs: &mut Vec<aida_core::RequirementSummary>,
    exact_draft_view: bool,
    machine_only: bool,
    explicit_machine_tag: bool,
) -> usize {
    if !exact_draft_view {
        return 0;
    }
    if machine_only {
        reqs.retain(is_machine_filed_draft);
        return 0;
    }
    if explicit_machine_tag {
        return 0;
    }
    let before = reqs.len();
    reqs.retain(|r| !is_machine_filed_draft(r));
    before - reqs.len()
}

#[cfg(test)]
#[path = "tests/bug_1498_machine_draft_lens_tests.rs"]
mod bug_1498_machine_draft_lens_tests;

/// BUG-788: should the open-work accepted-decision lens apply? True under the
/// bare-list default open lens (STORY-723) OR the explicit `open` shortcut, so
/// `aida list` and `aida list open` hide accepted ADRs identically. The explicit
/// shortcut still yields to any view-widening flag (`--all` / `--archived` /
/// `--deferred`) — BUG-781's guarantee that `--all` shows accepted ADRs holds
/// for `aida list open --all` too (`default_open_lens` already bakes in that
/// yield for the bare-list arm). Pure so the agreement between the two verbs is
/// unit-testable without a cache DB.
// trace:BUG-788 | ai:claude
pub(crate) fn list_applies_open_work_lens(
    default_open_lens: bool,
    explicit_open_alias: bool,
    all: bool,
    archived: bool,
    deferred: bool,
) -> bool {
    default_open_lens || (explicit_open_alias && !all && !archived && !deferred)
}

/// BUG-781: drop the ACCEPTED (terminal) decision rows from a listing under the
/// default open lens, returning how many were hidden.
///
/// For a `decision` spec (an ADR) the stored `Approved` means ACCEPTED — the
/// end of that class's lifecycle (draft = proposed, approved = accepted) — not
/// the "cleared to start" that `Approved` means for a task. Left in the open
/// lens, every ratified ADR reads as work that never finishes, and the only way
/// out was a manual `aida archive`. This is the sibling of the TASK-773
/// standing-artifact pass: same lens, same "an explicit ask overrides it" rule.
///
/// Escapes, all preserved by the caller's two gates: an explicit `--status`,
/// `--all`, `--archived` or `--deferred` clears `default_open_lens`, and
/// `--type decision` sets `asked_for_decision_type`. Nothing is written — the
/// stored status and the archive flag are untouched.
///
/// Pure (takes the row vector, not a backend) so the lens is unit-testable
/// without a cache DB.
// trace:BUG-781 | ai:claude
pub(crate) fn hide_accepted_decisions(
    reqs: &mut Vec<aida_core::RequirementSummary>,
    default_open_lens: bool,
    asked_for_decision_type: bool,
) -> usize {
    if !default_open_lens || asked_for_decision_type {
        return 0;
    }
    let before = reqs.len();
    reqs.retain(|r| !aida_core::lifecycle::is_accepted_decision(&r.req_type, &r.status));
    before - reqs.len()
}

#[cfg(test)]
#[path = "tests/bug_781_accepted_decision_lens_tests.rs"]
mod bug_781_accepted_decision_lens_tests;

// trace:BUG-788 | ai:claude
#[cfg(test)]
#[path = "tests/bug_788_open_shortcut_accepted_decision_tests.rs"]
mod bug_788_open_shortcut_accepted_decision_tests;

// trace:TASK-1176 | ai:claude
#[cfg(test)]
#[path = "tests/task_1176_superseded_state_tests.rs"]
mod task_1176_superseded_state_tests;

/// STORY-723: the denominator label for the agent `aida list` `count: N of M`
/// header — `open` under the default actionable lens (so it reconciles with the
/// `aida status` active total, which counts all statuses) else plain `matched`.
/// BUG-1737: the one sentence the default `aida list` view owes the reader —
/// that it is also withholding the archived and deferred tiers.
///
/// The lens already discloses every other class it drops (closed rows via
/// STORY-723, accepted ADRs via BUG-781, machine-filed drafts via BUG-1498),
/// each with a count and a flag. Archived and deferred were the only two
/// omitted silently, which let the disclosed list read as exhaustive: an
/// operator seeing `count: 30 of 53 open` plus three `note:` lines concluded
/// the 53 was the project's open backlog. It is not — it is the live slice of
/// it, and on this repository 1548 archived and 119 deferred rows sat outside
/// it unmentioned.
///
/// This is deliberately NOT the counted STORY-441 / STORY-584 nudge. BUG-783
/// turned those off by default on purpose ("an explicit open-work request
/// doesn't need reminding on every invocation that the other two tiers
/// exist"), and each costs an extra `list_summaries` query that the hot path
/// skips. That decision stands and `[list] show_hidden_hints` keeps its
/// meaning. What was missing is scope disclosure for the *count*, which needs
/// no count — hence a `bool` in and a fixed string out, so wiring it up cannot
/// add a backend query to the default path.
///
/// Gated on the open-WORK lens rather than the bare-list lens so `aida list`
/// and `aida list open` disclose identically — BUG-788 established that the
/// explicit `open` alias is a second spelling of the same view and filed their
/// disagreement as a defect. Every other explicit `--status`, and any
/// `--all` / `--archived` / `--deferred`, returns `None`: those views are not
/// hiding these tiers behind the reader's back.
// trace:BUG-1737 | ai:claude
pub(crate) fn list_lens_scope_disclosure(open_work_lens: bool) -> Option<&'static str> {
    open_work_lens.then_some("open lens excludes archived and deferred specs")
}

#[cfg(test)]
#[path = "tests/bug_1737_lens_scope_disclosure_tests.rs"]
mod bug_1737_lens_scope_disclosure_tests;

pub(crate) fn list_count_denom_label(default_open_lens: bool) -> &'static str {
    if default_open_lens {
        "open"
    } else {
        "matched"
    }
}

/// Render the human `aida list` footer without hiding an explicit `--limit`
/// slice. Untrimmed output remains byte-for-byte compatible with the historic
/// footer; a trimmed slice shares the TOON denominator vocabulary.
// trace:BUG-1209 | ai:codex
pub(crate) fn list_human_count_footer(
    visible: usize,
    total_after_filters: usize,
    limit: Option<usize>,
    default_open_lens: bool,
) -> String {
    if total_after_filters > visible {
        if let Some(limit) = limit {
            return format!(
                "{visible} of {total_after_filters} {} requirements (--limit {limit}; drop it or raise N to see the rest)",
                list_count_denom_label(default_open_lens)
            );
        }
    }
    // trace:STORY-1642 | ai:claude — singular for exactly one row.
    if visible == 1 {
        "1 requirement".to_string()
    } else {
        format!("{visible} requirements")
    }
}

/// Print the count footer for an empty human list only when an explicit limit
/// hid matching rows. A genuinely empty filtered result keeps the existing
/// concise `No requirements found.` presentation.
// trace:BUG-1209 | ai:codex
pub(crate) fn print_empty_human_list_truncation_footer(
    total_after_filters: usize,
    limit: Option<usize>,
    default_open_lens: bool,
) {
    if total_after_filters > 0 && limit.is_some() {
        println!(
            "\n{}",
            list_human_count_footer(0, total_after_filters, limit, default_open_lens)
        );
    }
}

#[cfg(test)]
#[path = "tests/bug_1209_list_limit_footer_tests.rs"]
mod bug_1209_list_limit_footer_tests;

/// STORY-723: the role-routed queue entries that are ACTUALLY actionable, in
/// queue-position order. The raw role queue is padded with archived / completed
/// / deferred corpses that were never dequeued (BUG: the front-door `next` once
/// nudged `aida queue work STORY-248` for an archived spec); this is the set a
/// nudge may safely point at. Each tuple is `(display_id, title, status)`. Live
/// = not archived, not deferred, not blocked, and in a workable status. Reads
/// the queue YAML + read-only cache only — NO full store load. (Supersedes
/// TASK-970's `agent_bare_top_queued`, which did no liveness filtering.)
// trace:STORY-723 trace:TASK-970
pub(crate) fn role_queue_actionable(
    project_root: &std::path::Path,
    role: &str,
) -> Vec<(String, String, String)> {
    // BUG-675: fold identity the SAME way read_queue_depth / `aida queue list`
    // do, so the actionable set is read from the case-correct queue file (a shell
    // reporting `Joe` finds `joe.yaml`) rather than silently reading zero.
    // trace:BUG-675 | ai:claude
    let store_path = project_root.join(".aida-store");
    let user = aida_core::db::resolve_queue_user(&store_path, &current_user_id(None));
    let queue_path = store_path
        .join("registry/queues")
        .join(format!("{}.yaml", user));
    // Own file first (its ordering is this shell's own intent), then every
    // sibling file — a `--for <role>` routing written by a peer lands in the
    // PEER's queue file, and reading only our own hid it. Same role predicate
    // either way; nothing is written back. trace:BUG-774 | ai:claude
    let mut queue_files: Vec<std::path::PathBuf> = vec![queue_path.clone()];
    if let Ok(read) = std::fs::read_dir(store_path.join("registry/queues")) {
        let me = aida_core::node::canonical_user_id(&user);
        let mut siblings: Vec<std::path::PathBuf> = read
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
            .filter(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|stem| aida_core::node::canonical_user_id(stem) != me)
                    .unwrap_or(false)
            })
            .collect();
        siblings.sort();
        queue_files.extend(siblings);
    }
    // Mirror read_queue_depth's role filter, then order by queue position.
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut role_entries: Vec<(i64, String)> = Vec::new();
    for path in &queue_files {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(entries) = serde_yaml::from_str::<Vec<serde_yaml::Value>>(&content) else {
            continue;
        };
        let mut from_file: Vec<(i64, String)> = entries
            .iter()
            .filter(|e| {
                e.get("for_role")
                    .and_then(serde_yaml::Value::as_str)
                    .map(|r| r == role)
                    .unwrap_or(false)
            })
            .filter_map(|e| {
                let pos = e
                    .get("position")
                    .and_then(serde_yaml::Value::as_i64)
                    .unwrap_or(0);
                let id = e
                    .get("requirement_id")
                    .and_then(serde_yaml::Value::as_str)?
                    .to_string();
                Some((pos, id))
            })
            .collect();
        from_file.sort_by_key(|(pos, _)| *pos);
        for (pos, id) in from_file {
            if seen_ids.insert(id.clone()) {
                role_entries.push((pos, id));
            }
        }
    }
    // Resolve id + title + liveness fields from the read-only cache (no full
    // store load), keyed by the requirement UUID the queue entry carries, and
    // KEEP only the actionable ones.
    let cache_path = project_root.join(".aida/cache.db");
    let conn = rusqlite::Connection::open_with_flags(
        &cache_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok();
    role_entries
        .into_iter()
        .filter_map(|(_, uuid)| {
            let row = conn.as_ref().and_then(|c| {
                c.query_row(
                    "SELECT COALESCE(agreed_id, spec_id, ''), title, status, \
                     archived, deferred, blocked \
                     FROM requirements_cache WHERE id = ?1",
                    [&uuid],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .ok()
            })?;
            let (id, title, status, archived, deferred, blocked) = row;
            // Drop archived / deferred / blocked / non-workable-status corpses —
            // never surface one as the actionable head. trace:STORY-723
            if !queue_row_actionable(&status, archived, deferred, blocked) {
                return None;
            }
            let display_id = if id.is_empty() { uuid } else { id };
            Some((display_id, title, status))
        })
        .collect()
}

/// STORY-723: the top-N ACTIONABLE queued items for the active role as
/// `(display_id, title)` — the head of [`role_queue_actionable`].
pub(crate) fn agent_bare_top_queued(
    project_root: &std::path::Path,
    role: &str,
    n: usize,
) -> Vec<(String, String)> {
    role_queue_actionable(project_root, role)
        .into_iter()
        .take(n)
        .map(|(id, title, _status)| (id, title))
        .collect()
}

/// TASK-970: content-first bare `aida` for AGENT MODE. Prints the same fast
/// `aida status` snapshot a human gets from `aida status` (role / branch /
/// queue depth / counts), then lists the top queued items for the active role.
/// Self-resolves the project root and reads the cache read-only — no full store
/// load — mirroring the STORY-707 fast path. Falls back to the getting-started
/// menu when not inside an AIDA project (where the snapshot would be empty).
// trace:TASK-970
pub(crate) fn handle_bare_agent_status() -> Result<()> {
    let Ok(project_root) = find_project_root() else {
        print_tiered_help();
        return Ok(());
    };
    let snap = collect_fast_status_snapshot(&project_root);
    // TASK-964: in TOON agent mode, emit the token-efficient snapshot (this
    // handler is already agent-only, but AIDA_AGENT_OUTPUT=0 force-selects the
    // human shape for parity with the explicit `aida status`). STORY-730: lead
    // with the morning-after drain banner. trace:TASK-964 trace:STORY-730
    let drain_root = find_main_worktree_root().unwrap_or_else(|_| project_root.clone());
    if agent_output_mode() {
        print_live_drain_status_line(&drain_root);
        print_morning_after_toon(&project_root);
        print_toon_status(&snap, &project_root);
        return Ok(());
    }
    print_live_drain_status_line(&drain_root);
    print_morning_after_banner(&project_root);
    print_fast_status(&snap);

    let queued = agent_bare_top_queued(&project_root, &snap.role, AGENT_BARE_QUEUE_TOPN);
    println!("{}", "─── Queued (top) ───".bold());
    if queued.is_empty() {
        println!("  {}", "(nothing queued for this role)".dimmed());
    } else {
        for (id, title) in &queued {
            if title.is_empty() {
                println!("  {id}");
            } else {
                println!("  {}  {}", id.bold(), title);
            }
        }
        if snap.queue_depth > queued.len() {
            println!(
                "  {}",
                format!(
                    "(+{} more — `aida queue list`)",
                    snap.queue_depth - queued.len()
                )
                .dimmed()
            );
        }
    }
    println!();
    Ok(())
}

/// Surface an in-flight drain at the top of `aida status` using the same
/// local-only liveness probe as `statusline` and `statusbar`. Stale/no drain
/// renders nothing, preserving existing bytes on quiet projects.
// trace:TASK-1194 | ai:codex
pub(crate) fn print_live_drain_status_line(project_root: &std::path::Path) {
    if let Some(probe) = crate::drain_state::drain_liveness_probe(project_root) {
        println!("{}", probe.status_line().cyan().bold());
        println!();
    }
}

#[cfg(test)]
#[path = "tests/story723_front_door_tests.rs"]
mod story723_front_door_tests;

#[cfg(test)]
#[path = "tests/task1456_rework_visibility_tests.rs"]
mod task1456_rework_visibility_tests;

#[cfg(test)]
#[path = "tests/story707_fast_status_tests.rs"]
mod story707_fast_status_tests;

// BUG-670 → BUG-675: the AGENT-MODE `aida status` projection must give an
// unambiguous "is there work for me?" signal — lead with `queue_actionable`. With
// BUG-675 making `queue_depth` identity-trustworthy (same resolver as
// `aida queue list`), the raw depth now ALWAYS rides along after the lead signal
// instead of being suppressed on divergence.
#[cfg(test)]
#[path = "tests/bug670_agent_status_tests.rs"]
mod bug670_agent_status_tests;

/// TASK-1055: warm the independent gh-backed status probes concurrently so a
/// `--full` render pays the latency of the SLOWEST probe, not the SUM. Each
/// target is behind a process-lifetime memo (open-PR snapshot via BUG-613,
/// PR/CI facts + recently-merged via TASK-1055), so calling them here only
/// pre-populates the cache the sequential render reads next — output is
/// unchanged. The current-branch facts need the branch name, derived from a
/// cheap local `git` call. Worktree rows read `collect_open_prs(main_root)`,
/// so both the cwd root and the main-worktree root are warmed (deduped to one
/// fetch when they're the same canonical path).
// trace:TASK-1055
pub(crate) fn warm_status_network_probes(project_root: &std::path::Path) {
    let main_root = main_worktree_root_from(project_root);
    let branch_name = collect_branch_facts(project_root).map(|b| b.name);

    // Distinct canonical roots whose open-PR snapshot the render will read.
    let proj_canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let main_canon = main_root
        .canonicalize()
        .unwrap_or_else(|_| main_root.clone());
    let mut pr_roots: Vec<std::path::PathBuf> = vec![project_root.to_path_buf()];
    if main_canon != proj_canon {
        pr_roots.push(main_root.clone());
    }

    std::thread::scope(|s| {
        for root in &pr_roots {
            s.spawn(move || {
                let _ = collect_open_prs(root);
            });
        }
        s.spawn(|| {
            // Matches the limit the recently-merged render uses below.
            let _ = collect_recently_merged_prs(project_root, 5);
        });
        if let Some(branch) = branch_name.as_deref() {
            s.spawn(move || {
                let _ = collect_pr_facts(project_root, branch);
            });
        }
    });
}

/// TASK-1065: assemble the `RequirementsStore` the rich `aida status` sections
/// consume from the CACHE read-projection instead of a full `backend.load()`.
///
/// The store's metadata (name / title / description / features / id-config) comes
/// from one cheap `metadata.yaml` read; its `requirements` are reconstructed from
/// `list_summaries` (archive + defer = Both, so the projection mirrors the full
/// on-disk set `backend.load()` returned). Each lightweight `Requirement` carries
/// exactly the fields the `aida status --full` hygiene/cleanup doctor scans, the
/// queue snapshot, and the Project/Requirements sections read: id, spec_id,
/// agreed_id, title, status, req_type, modified_at, archived. Fields not projected
/// into the cache (description body, comments, relationships, the decision-request
/// payload, …) are left at their `Requirement::new` defaults — the status sections
/// never read them (the decision-inbox count reads the `has_pending_decision`
/// cache column via `backend.pending_decision_count()` instead).
///
/// Fidelity notes (same cache-vs-load divergences `aida list` already carries):
/// an EPIC's status is the cache's derived rollup rather than its stored value,
/// and a spec with a *custom* status string maps to `Draft` + `custom_status`
/// since it isn't one of the eight canonical variants. Both are exotic and do not
/// affect the doctor scans (which exclude epics and only match canonical
/// statuses).
// trace:TASK-1065 | ai:claude
pub(crate) fn build_status_store_from_cache(
    backend: &aida_core::CachedGitBackend,
) -> Result<aida_core::models::RequirementsStore> {
    use aida_core::{RequirementStatus, RequirementType};

    let mut store = backend.load_metadata_only()?;

    let summaries = backend.list_summaries(&aida_core::ListFilter {
        archive: aida_core::ArchiveFilter::Both,
        defer: aida_core::db::DeferFilter::Both,
        ..Default::default()
    })?;

    store.requirements = summaries
        .into_iter()
        .map(|s| {
            let mut req = aida_core::Requirement::new(s.title, String::new());
            req.id = s.id;
            req.spec_id = s.spec_id;
            req.agreed_id = s.agreed_id;
            req.owner = s.owner;
            req.assignee = s.assignee;
            req.feature = s.feature;
            req.archived = s.archived;
            req.deferred = s.deferred;
            // The cache stores RequirementType's Debug form; map it back. An
            // unrecognized (custom) type falls back to Task — it won't match any
            // status-scan type predicate, which is the safe default.
            req.req_type =
                RequirementType::from_cache_str(&s.req_type).unwrap_or(RequirementType::Task);
            // Canonical status → the typed enum. A custom status string isn't one
            // of the eight variants, so preserve it verbatim in `custom_status`
            // (and leave the enum at Draft) rather than mis-map it.
            match RequirementStatus::from_filter_str(&s.status) {
                Some(st) => req.status = st,
                None => {
                    req.status = RequirementStatus::Draft;
                    req.custom_status = Some(s.status);
                }
            }
            req.modified_at = chrono::DateTime::parse_from_rfc3339(&s.modified_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or(req.modified_at);
            req
        })
        .collect();

    Ok(store)
}

// TASK-1065: the rich `aida status --full` store is sourced from the cache
// read-projection, never a full `backend.load()` over the object YAMLs, and the
// decision-inbox count reads the `has_pending_decision` cache column. These tests
// lock both contracts. trace:TASK-1065 | ai:claude
#[cfg(test)]
#[path = "tests/task_1065_status_cacheback_tests.rs"]
mod task_1065_status_cacheback_tests;

/// Detect whether `project_root` is the aida repo itself — used to opt into
/// the developer-context section. We check the workspace root Cargo.toml for
/// the joemooney/aida repository URL.
pub(crate) fn is_aida_repo(project_root: &std::path::Path) -> bool {
    let cargo_toml = project_root.join("Cargo.toml");
    if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
        // Match the workspace.package repository field exactly so this is
        // robust against forks: only the canonical repo gets dev context.
        return content.contains("repository = \"https://github.com/joemooney/aida\"")
            && content.contains("[workspace]");
    }
    false
}

pub(crate) fn print_aida_dev_context(project_root: &std::path::Path) {
    println!("{}", "─── AIDA development context ───".bold());

    // Workspace version vs latest tag.
    let workspace_version = read_workspace_version(project_root).unwrap_or_else(|| "?".into());
    let latest_tag = git_describe_latest_tag(project_root).unwrap_or_else(|| "(none)".into());
    let commits_since_tag = git_commits_since_tag(project_root, &latest_tag).unwrap_or(0);
    // ADR-72: releases no longer gate on a cross-platform run.
    // trace:TASK-1723 | ai:claude
    let cross_platform_ci = if commits_since_tag > 0 && CROSS_PLATFORM_WORKFLOW_SCHEDULED {
        Some(cross_platform_ci_status(project_root))
    } else {
        None
    };
    println!("  Running binary:     {}", build_banner());
    println!("  Workspace version:  v{}", workspace_version);
    println!("  Latest release tag: {}", latest_tag);
    if commits_since_tag > 0 {
        let readiness = match cross_platform_ci.as_ref().map(|ci| ci.release_gate) {
            Some(CrossPlatformReleaseGate::Ready) => "ready to cut a release".green().to_string(),
            Some(CrossPlatformReleaseGate::Blocked) => "BLOCKED".red().bold().to_string(),
            Some(CrossPlatformReleaseGate::Unknown) => "unknown".yellow().to_string(),
            None => "ready to cut a release".green().to_string(),
        };
        println!(
            "  {} commits ahead of {} — release-readiness: {}",
            commits_since_tag, latest_tag, readiness
        );
        if let Some(ci) = &cross_platform_ci {
            println!("  Cross-platform CI:  {}", ci.summary);
            if let Some(detail) = &ci.detail {
                println!("    {}", detail);
            }
        }
    } else {
        println!("  Tree matches latest tag — no pending release");
    }

    // Template-symlink integrity.
    let symlink_status = check_template_symlinks(project_root);
    println!("  Template symlinks:  {}", symlink_status);

    // Quick build sanity (just check `target/` exists and Cargo.lock is in sync).
    let cargo_lock_synced = project_root.join("Cargo.lock").exists();
    println!(
        "  Cargo.lock present: {}",
        if cargo_lock_synced {
            "yes".green().to_string()
        } else {
            "NO".red().to_string()
        }
    );

    println!();
    println!("  Helpful:");
    println!(
        "    {}    — bump version + tag + push",
        "scripts/release.sh".cyan()
    );
    println!(
        "    {}    — cargo publish to crates.io",
        "scripts/publish.sh".cyan()
    );
    println!();
}

/// STORY-673: terse one-line AIDA-dev-context for the default `aida status`.
/// Shows the running binary + workspace version + commits-since-tag, and
/// crucially SKIPS the cross-platform-CI network probe and template/lockfile
/// checks the full block runs — so the default path stays cheap. `--full` /
/// `--all` calls `print_aida_dev_context` for the complete picture.
/// trace:STORY-673 | ai:claude
pub(crate) fn print_aida_dev_context_summary(project_root: &std::path::Path) {
    let workspace_version = read_workspace_version(project_root).unwrap_or_else(|| "?".into());
    let latest_tag = git_describe_latest_tag(project_root).unwrap_or_else(|| "(none)".into());
    let commits_since_tag = git_commits_since_tag(project_root, &latest_tag).unwrap_or(0);
    let ahead = if commits_since_tag > 0 {
        format!(", {commits_since_tag} commits ahead of {latest_tag}")
    } else {
        ", matches latest tag".to_string()
    };
    println!("{}", "─── AIDA development context ───".bold());
    println!(
        "  {}  (v{}{}) · `aida status --full`",
        build_banner(),
        workspace_version,
        ahead
    );
    println!();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CrossPlatformReleaseGate {
    Ready,
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CrossPlatformCiSummary {
    pub(crate) release_gate: CrossPlatformReleaseGate,
    pub(crate) summary: String,
    pub(crate) detail: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct CrossPlatformCiCache {
    pub(crate) fetched_at: chrono::DateTime<chrono::Utc>,
    pub(crate) runs: Vec<GhWorkflowRun>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct GhWorkflowRun {
    pub(crate) status: Option<String>,
    pub(crate) conclusion: Option<String>,
    #[serde(rename = "createdAt")]
    pub(crate) created_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(rename = "databaseId")]
    pub(crate) database_id: Option<u64>,
    #[serde(rename = "headSha")]
    pub(crate) head_sha: Option<String>,
    pub(crate) url: Option<String>,
}

pub(crate) fn cross_platform_ci_status(project_root: &std::path::Path) -> CrossPlatformCiSummary {
    const CACHE_TTL_SECONDS: i64 = 300;
    let now = chrono::Utc::now();

    if let Some(cache) = read_cross_platform_ci_cache(project_root) {
        if now.signed_duration_since(cache.fetched_at).num_seconds() < CACHE_TTL_SECONDS {
            return summarize_cross_platform_ci_runs(now, Ok(cache.runs));
        }
    }

    let runs = fetch_cross_platform_ci_runs(project_root);
    if let Ok(runs) = &runs {
        let cache = CrossPlatformCiCache {
            fetched_at: now,
            runs: runs.clone(),
        };
        let _ = write_cross_platform_ci_cache(project_root, &cache);
    }
    summarize_cross_platform_ci_runs(now, runs)
}

pub(crate) fn fetch_cross_platform_ci_runs(
    project_root: &std::path::Path,
) -> std::result::Result<Vec<GhWorkflowRun>, String> {
    fetch_cross_platform_ci_runs_with_args(project_root, &[])
}

// trace:STORY-1043 | ai:codex
pub(crate) fn fetch_scheduled_cross_platform_ci_runs(
    project_root: &std::path::Path,
) -> std::result::Result<Vec<GhWorkflowRun>, String> {
    fetch_cross_platform_ci_runs_with_args(project_root, &["--event", "schedule"])
}

// trace:STORY-1043 | ai:codex
pub(crate) fn fetch_cross_platform_ci_runs_with_args(
    project_root: &std::path::Path,
    extra_args: &[&str],
) -> std::result::Result<Vec<GhWorkflowRun>, String> {
    let mut args = vec![
        "run",
        "list",
        "--workflow",
        "cross-platform.yml",
        "--branch",
        "main",
        "--limit",
        "20",
        "--json",
        "status,conclusion,createdAt,databaseId,headSha,url",
    ];
    args.extend_from_slice(extra_args);
    let output = std::process::Command::new("gh")
        .args(args)
        .current_dir(project_root)
        .output()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "gh run list failed".to_string()
        } else {
            stderr
        });
    }
    serde_json::from_slice(&output.stdout).map_err(|err| err.to_string())
}

pub(crate) fn read_cross_platform_ci_cache(
    project_root: &std::path::Path,
) -> Option<CrossPlatformCiCache> {
    let path = cross_platform_ci_cache_path(project_root);
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

pub(crate) fn write_cross_platform_ci_cache(
    project_root: &std::path::Path,
    cache: &CrossPlatformCiCache,
) -> Result<()> {
    let path = cross_platform_ci_cache_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_atomic(&path, serde_json::to_string_pretty(cache)?.as_bytes())?;
    Ok(())
}

pub(crate) fn cross_platform_ci_cache_path(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root
        .join(".aida")
        .join("cache")
        .join("cross-platform-ci-status.json")
}

pub(crate) fn summarize_cross_platform_ci_runs(
    now: chrono::DateTime<chrono::Utc>,
    runs: std::result::Result<Vec<GhWorkflowRun>, String>,
) -> CrossPlatformCiSummary {
    let runs = match runs {
        Ok(runs) => runs,
        Err(_) => {
            return CrossPlatformCiSummary {
                release_gate: CrossPlatformReleaseGate::Unknown,
                summary: "unknown (gh unreachable)".yellow().to_string(),
                detail: Some(
                    "Release readiness cannot be confirmed without GitHub CI status.".to_string(),
                ),
            };
        }
    };

    let Some(latest) = runs.first() else {
        return CrossPlatformCiSummary {
            release_gate: CrossPlatformReleaseGate::Unknown,
            summary: "unknown (no cross-platform run found)".yellow().to_string(),
            detail: Some(
                "Run `gh workflow run cross-platform.yml --ref main` before release prep."
                    .to_string(),
            ),
        };
    };

    let latest_age = latest
        .created_at
        .map(|created| format_ci_age(now, created))
        .unwrap_or_else(|| "unknown age".to_string());
    let latest_run = latest
        .database_id
        .map(|id| format!(", run {}", id))
        .unwrap_or_default();
    let latest_completed_success = latest.status.as_deref() == Some("completed")
        && latest.conclusion.as_deref() == Some("success");
    let latest_fresh = latest
        .created_at
        .map(|created| now.signed_duration_since(created).num_hours() < 24)
        .unwrap_or(false);

    if latest_completed_success && latest_fresh {
        return CrossPlatformCiSummary {
            release_gate: CrossPlatformReleaseGate::Ready,
            summary: format!(
                "{} green ({}{})",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                latest_age,
                latest_run
            ),
            detail: None,
        };
    }

    let last_green = runs.iter().find(|run| {
        run.status.as_deref() == Some("completed") && run.conclusion.as_deref() == Some("success")
    });
    let last_green_detail = last_green.and_then(|run| {
        run.created_at.map(|created| {
            format!(
                "Last green: {}. Releases require <24h green.",
                format_ci_age(now, created)
            )
        })
    });

    let reason = if latest_completed_success {
        "stale"
    } else {
        latest.conclusion.as_deref().unwrap_or("not green")
    };
    CrossPlatformCiSummary {
        release_gate: CrossPlatformReleaseGate::Blocked,
        summary: format!(
            "{} {} ({}{})",
            crate::glyph(crate::glyphs::Glyph::Cross).red(),
            reason,
            latest_age,
            latest_run
        ),
        detail: last_green_detail.or_else(|| {
            Some(
                "No previous green cross-platform run found. Releases require <24h green."
                    .to_string(),
            )
        }),
    }
}

/// ADR-72: AIDA is Linux-only and `cross-platform.yml` is workflow_dispatch
/// only, so there is no scheduled cross-platform run to be red. Its historical
/// scheduled failures stay in `gh run list --event schedule` forever, so the
/// red signal must be switched off here rather than left to age out. Flip this
/// only together with a new operator decision that re-adds the schedule.
// trace:TASK-1723 | ai:claude
pub(crate) const CROSS_PLATFORM_WORKFLOW_SCHEDULED: bool = false;

// trace:STORY-1043 | ai:codex
pub(crate) fn nightly_red_status(
    project_root: &std::path::Path,
) -> Option<awaiting_you::NightlyRedItem> {
    // trace:TASK-1723 | ai:claude
    if !CROSS_PLATFORM_WORKFLOW_SCHEDULED {
        return None;
    }
    let now = chrono::Utc::now();
    let latest_completed_main_run =
        fetch_cross_platform_ci_runs_with_args(project_root, &["--status", "completed"])
            .ok()
            .and_then(|runs| runs.into_iter().next());
    summarize_nightly_red_runs(
        now,
        fetch_scheduled_cross_platform_ci_runs(project_root).ok(),
        latest_completed_main_run.as_ref(),
        |ancestor, descendant| {
            is_ancestor_commit(project_root, ancestor, descendant).unwrap_or(false)
        },
    )
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct NightlyRedCache {
    pub(crate) fetched_at: chrono::DateTime<chrono::Utc>,
    pub(crate) item: Option<awaiting_you::NightlyRedItem>,
}

// BUG-1288: machine consumers poll this surface. Bound the two workflow API
// calls with a documented 30-second freshness window; a miss refreshes the
// exact same report and subsequent polls remain network-free.
// trace:BUG-1288 | ai:codex
pub(crate) fn cached_nightly_red_status(
    project_root: &std::path::Path,
) -> Option<awaiting_you::NightlyRedItem> {
    // A cache written before ADR-72 must not resurrect the red line.
    // trace:TASK-1723 | ai:claude
    if !CROSS_PLATFORM_WORKFLOW_SCHEDULED {
        return None;
    }
    let path = project_root
        .join(".aida")
        .join("cache")
        .join("awaiting-nightly-red.json");
    let now = chrono::Utc::now();
    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Ok(cache) = serde_json::from_str::<NightlyRedCache>(&raw) {
            if now.signed_duration_since(cache.fetched_at).num_seconds() < 30 {
                return cache.item;
            }
        }
    }
    let item = nightly_red_status(project_root);
    let cache = NightlyRedCache {
        fetched_at: now,
        item: item.clone(),
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(raw) = serde_json::to_vec(&cache) {
        let _ = write_atomic(&path, &raw);
    }
    item
}

// trace:STORY-1043 | ai:codex
pub(crate) fn summarize_nightly_red_runs(
    now: chrono::DateTime<chrono::Utc>,
    runs: Option<Vec<GhWorkflowRun>>,
    latest_completed_main_run: Option<&GhWorkflowRun>,
    head_descends_from_latest_scheduled_failure: impl Fn(&str, &str) -> bool,
) -> Option<awaiting_you::NightlyRedItem> {
    let runs = runs?;
    let latest = runs.first()?;
    if latest.status.as_deref() != Some("completed")
        || latest.conclusion.as_deref() == Some("success")
    {
        return None;
    }
    if latest_completed_main_run_clears_nightly_red(
        latest,
        latest_completed_main_run,
        head_descends_from_latest_scheduled_failure,
    ) {
        return None;
    }
    let streak: Vec<&GhWorkflowRun> = runs
        .iter()
        .take_while(|run| {
            run.status.as_deref() == Some("completed")
                && run.conclusion.as_deref().is_some_and(|c| c != "success")
        })
        .collect();
    let nights = streak.len().max(1);
    let since_run = streak.last().copied().unwrap_or(latest);
    let since = since_run
        .created_at
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|| "unknown date".to_string());
    let run = latest.database_id;
    let run_text = run
        .map(|id| format!("run {id}"))
        .unwrap_or_else(|| "run unknown".to_string());
    let summary = format!(
        "cross-platform nightly red since {since} ({run_text}, {nights} night{})",
        if nights == 1 { "" } else { "s" }
    );
    let _ = now;
    Some(awaiting_you::NightlyRedItem {
        summary,
        run_id: run,
        nights,
    })
}

// trace:BUG-1148 | ai:codex
pub(crate) fn latest_completed_main_run_clears_nightly_red(
    latest_scheduled_failure: &GhWorkflowRun,
    latest_completed_main_run: Option<&GhWorkflowRun>,
    head_descends_from_latest_scheduled_failure: impl Fn(&str, &str) -> bool,
) -> bool {
    let Some(latest_completed_main_run) = latest_completed_main_run else {
        return false;
    };
    if latest_completed_main_run.status.as_deref() != Some("completed")
        || latest_completed_main_run.conclusion.as_deref() != Some("success")
    {
        return false;
    }
    let Some(failure_created_at) = latest_scheduled_failure.created_at else {
        return false;
    };
    let Some(clearing_created_at) = latest_completed_main_run.created_at else {
        return false;
    };
    if clearing_created_at <= failure_created_at {
        return false;
    }
    let Some(failure_sha) = latest_scheduled_failure
        .head_sha
        .as_deref()
        .filter(|sha| !sha.trim().is_empty())
    else {
        return false;
    };
    let Some(clearing_sha) = latest_completed_main_run
        .head_sha
        .as_deref()
        .filter(|sha| !sha.trim().is_empty())
    else {
        return false;
    };
    head_descends_from_latest_scheduled_failure(failure_sha, clearing_sha)
}

pub(crate) fn format_ci_age(
    now: chrono::DateTime<chrono::Utc>,
    then: chrono::DateTime<chrono::Utc>,
) -> String {
    let seconds = now.signed_duration_since(then).num_seconds().max(0);
    if seconds < 3600 {
        let minutes = (seconds / 60).max(1);
        format!("{}m ago", minutes)
    } else if seconds < 48 * 3600 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{} days ago", seconds / 86_400)
    }
}

#[cfg(test)]
#[path = "tests/task_486_cross_platform_ci_status_tests.rs"]
mod task_486_cross_platform_ci_status_tests;
