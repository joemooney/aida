//! `aida status --cleanup` — surface cleanup-actionable state that would
//! otherwise require manual archaeology across `git worktree list`,
//! `aida session leases --all`, `gh pr list`, `git status`, and recent
//! `git log`.
//!
//! The collection of facts (lease list, live-process probe, `git`
//! invocations, `gh` calls, `~/.claude/projects/` scan) happens in
//! `main.rs` next to the existing helpers. This module owns the pure
//! shape of the report plus the text + JSON renderers, so the rendering
//! is unit-testable without touching the filesystem.
//!
//! Categories are stakes-ordered: loss-risky items first (uncommitted
//! WIP, sticky In-Progress, unpushed branches with no PR), then
//! visibility-risky (missed auto-bumps), then attention-needed (open
//! PRs awaiting merge), then pure-cleanup (dormant leases, stale
//! reviewer leases on merged PRs, orphan project dirs).
//!
//! trace:STORY-385 | ai:claude

use colored::Colorize;
use std::io::Write;

/// One snapshot of every "needs attention" category. Each `Vec` may be
/// empty — an empty vec for a category means "nothing to clean here";
/// a `Healthy` footer enumerates the categories that came back empty
/// when rendering.
#[derive(Debug, Default, Clone)]
pub(crate) struct CleanupReport {
    /// Worktrees with uncommitted modifications. Loss-risky: real work
    /// sitting on disk only.
    pub uncommitted_wip: Vec<UncommittedWipItem>,
    /// Specs ◐ In Progress with no Live or Dormant lease covering them.
    /// May still have local/pushed branch commits — those are recoverable
    /// but easy to forget about.
    pub sticky_in_progress: Vec<StickyInProgressItem>,
    /// Local branches with commits ahead of `origin/main` and no open PR.
    /// Orphaned work — once the worktree closes, the commits become
    /// archaeology.
    pub branches_ahead_no_pr: Vec<BranchAheadItem>,
    /// Specs ◉ Done where a referencing commit landed on `main` but the
    /// auto-bump scanner didn't promote them (commit format mismatch,
    /// timing race, or BUG-96 unreadable YAML).
    pub missed_auto_bump: Vec<MissedAutoBumpItem>,
    /// Open PRs — CI / review / merge state worth surfacing.
    pub open_prs: Vec<OpenPrItem>,
    /// Dormant leases (worktree present, no live claude, <24h old).
    /// Recoverable but not actively worked.
    pub dormant_leases: Vec<DormantLeaseItem>,
    /// Reviewer leases (`--owns PR-N`) where the PR has been merged.
    /// Pure cleanup — safe to end.
    pub stale_reviewer_leases: Vec<StaleReviewerLeaseItem>,
    /// `~/.claude/projects/<slug>` directories whose recorded cwd no
    /// longer exists. Pure cleanup — covered by
    /// `aida session prune --orphans`.
    pub orphan_project_dirs: Vec<OrphanProjectDirItem>,
    /// STORY-469 Guard 3: specs whose substrate status claims Done/Completed
    /// but whose local reality (an active lease + a dirty worktree, and/or no
    /// commit + no PR) contradicts the claim — an agent's "I shipped" that the
    /// substrate doesn't corroborate. Loss-/trust-risky: the operator sees the
    /// divergence at status time instead of trusting the verbal claim.
    pub claimed_done_diverged: Vec<ClaimedDoneDivergedItem>,
    /// STORY-508/TASK-651: the project's active forge, so the open-PR hint
    /// names the right CLI (gh/glab) or none (pure-git). `None` means "not
    /// resolved" (e.g. a `default()`-built report in a test) — rendered as
    /// GitHub for back-compat. Set by `collect_cleanup_report`.
    pub forge_kind: Option<crate::forge::ForgeKind>,
}

/// A worktree with uncommitted modifications. Surfaces the path + lease
/// scope (when a lease covers the worktree) so the operator knows which
/// spec the work belongs to.
#[derive(Debug, Clone)]
pub(crate) struct UncommittedWipItem {
    pub worktree_path: std::path::PathBuf,
    pub branch: String,
    pub scope: Option<String>,
    pub modified_files: usize,
    pub age_hours: i64,
}

/// A spec marked In Progress whose lease has died (or never existed).
/// `local_commits` and `pushed_commits` distinguish loss-risky cases
/// from the merely-stuck ones.
#[derive(Debug, Clone)]
pub(crate) struct StickyInProgressItem {
    pub spec_id: String,
    pub title: String,
    pub branch: Option<String>,
    /// Commits on the branch not yet pushed (loss-risky).
    pub unpushed_commits: u32,
    /// Commits pushed to the upstream branch (recoverable via PR).
    pub pushed_commits: u32,
    pub age_hours: Option<i64>,
}

/// A branch ahead of `origin/main` with no open PR. Distinct from
/// sticky-in-progress: the branch may exist for a spec whose status is
/// already Done — the PR just was never opened (or was closed without
/// merge).
#[derive(Debug, Clone)]
pub(crate) struct BranchAheadItem {
    pub branch: String,
    pub commits_ahead: u32,
    pub has_upstream: bool,
}

/// A Done spec with a candidate landing commit on `main` that the
/// auto-bump scanner missed.
#[derive(Debug, Clone)]
pub(crate) struct MissedAutoBumpItem {
    pub spec_id: String,
    pub title: String,
    pub landing_sha: String,
}

/// An open PR (any state) worth surfacing in cleanup.
#[derive(Debug, Clone)]
pub(crate) struct OpenPrItem {
    pub number: u64,
    pub title: String,
    pub head_branch: String,
    pub ci_rollup: Option<String>,
    pub mergeable: Option<String>,
    /// `reviewDecision` from `gh pr list`: `APPROVED`, `CHANGES_REQUESTED`,
    /// `REVIEW_REQUIRED`, or `""`/None when no review is set up on the repo.
    /// The "Awaiting you" classifier excludes `CHANGES_REQUESTED` PRs.
    /// trace:STORY-465 | ai:claude
    pub review_decision: Option<String>,
}

/// A dormant lease (worktree present, no live process, <24h old).
///
/// BUG-376: `spec_done` is the *"lingering implementer with done queue"*
/// subcategory marker — the lease's scope is a spec that has already
/// reached **Done** or **Completed**, so the agent finished work
/// correctly but never exited. Informational rather than error: the
/// PR shipped, nothing is at risk; the only cost is operator attention
/// and a forced manual Ctrl+D. Surfaced as a sub-line within the
/// Dormant category rather than a sibling category so the recovery
/// verb stays `aida session end <lease>`.
#[derive(Debug, Clone)]
pub(crate) struct DormantLeaseItem {
    pub lease_id: String,
    pub scope: String,
    pub role: Option<String>,
    pub worktree_path: std::path::PathBuf,
    pub age_hours: i64,
    /// True when the lease's scope is a spec at status Done or Completed
    /// — the BUG-376 lingering-implementer signal. trace:BUG-376
    pub spec_done: bool,
}

/// A reviewer-role lease for a PR that has already merged. Safe to end.
#[derive(Debug, Clone)]
pub(crate) struct StaleReviewerLeaseItem {
    pub lease_id: String,
    pub pr_number: u64,
    pub worktree_path: std::path::PathBuf,
    pub age_hours: i64,
}

/// A `~/.claude/projects/<slug>` directory whose recorded cwd no longer
/// exists on disk.
#[derive(Debug, Clone)]
pub(crate) struct OrphanProjectDirItem {
    pub path: std::path::PathBuf,
    pub decoded_cwd: String,
    pub jsonl_count: usize,
}

/// STORY-469 Guard 3: a spec whose status claims Done/Completed but whose
/// local reality contradicts the claim. `kind` names which contradiction
/// fired so the renderer can word the recovery hint precisely.
/// trace:STORY-469 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct ClaimedDoneDivergedItem {
    pub spec_id: String,
    pub title: String,
    /// The spec's substrate status word ("Done" / "Completed").
    pub claimed_status: String,
    pub kind: DivergenceKind,
}

/// Why a claimed-Done spec is flagged as diverged. trace:STORY-469 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DivergenceKind {
    /// An active lease covers the spec AND the worktree has uncommitted
    /// modifications — work is still on disk despite the Done claim.
    DirtyWorktree {
        branch: String,
        modified_files: usize,
        age_hours: i64,
    },
    /// The spec is Done but no commit references it and no PR exists — the
    /// "I shipped" claim has no substrate evidence.
    NoCommitNoPr,
}

impl CleanupReport {
    /// Total number of items across every category. Zero means nothing
    /// needs attention — the renderer prints a single all-clear line.
    pub fn total(&self) -> usize {
        self.uncommitted_wip.len()
            + self.sticky_in_progress.len()
            + self.branches_ahead_no_pr.len()
            + self.missed_auto_bump.len()
            + self.open_prs.len()
            + self.dormant_leases.len()
            + self.stale_reviewer_leases.len()
            + self.orphan_project_dirs.len()
            + self.claimed_done_diverged.len()
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    /// Multi-line summary suitable for appending to the default
    /// `aida status` output when the report is non-empty. Inlines
    /// per-category counts + a single representative item per non-empty
    /// category so the operator doesn't have to run `aida status
    /// --cleanup` to learn what the count refers to. The hint at the
    /// bottom still points at `--cleanup` for the full report (with
    /// recovery commands per item).
    /// trace:TASK-1-099-companion | ai:claude
    pub fn summary_line(&self) -> Option<String> {
        // Project-scoped count for the summary: excludes orphan_project_dirs
        // (system-wide cross-project state surfaced in --cleanup instead;
        // the dedicated 'aida session prune --orphans' verb is the
        // canonical recovery for those).
        let project_scoped_count = self.uncommitted_wip.len()
            + self.sticky_in_progress.len()
            + self.branches_ahead_no_pr.len()
            + self.missed_auto_bump.len()
            + self.open_prs.len()
            + self.dormant_leases.len()
            + self.stale_reviewer_leases.len()
            + self.claimed_done_diverged.len();
        if project_scoped_count == 0 {
            return None;
        }
        let n = project_scoped_count;
        let mut lines: Vec<String> = Vec::new();
        lines.push(format!(
            "{} {} item{} need cleanup attention:",
            "⚠".yellow(),
            n,
            if n == 1 { "" } else { "s" },
        ));

        // Per-category brief — show category name + count + first item.
        // Order matches the --cleanup full report's stakes order so the
        // top-listed line is the highest-impact category.
        if !self.uncommitted_wip.is_empty() {
            lines.push(format!(
                "  • Uncommitted work at risk ({}): {}",
                self.uncommitted_wip.len(),
                self.uncommitted_wip[0].branch.dimmed(),
            ));
        }
        if !self.claimed_done_diverged.is_empty() {
            lines.push(format!(
                "  • Claimed Done but substrate disagrees ({}): {}",
                self.claimed_done_diverged.len(),
                self.claimed_done_diverged[0].spec_id.dimmed(),
            ));
        }
        if !self.sticky_in_progress.is_empty() {
            lines.push(format!(
                "  • Specs In Progress without lease ({}): {}",
                self.sticky_in_progress.len(),
                self.sticky_in_progress[0].spec_id.dimmed(),
            ));
        }
        if !self.branches_ahead_no_pr.is_empty() {
            lines.push(format!(
                "  • Local branches ahead of main, no PR ({}): {}",
                self.branches_ahead_no_pr.len(),
                self.branches_ahead_no_pr[0].branch.dimmed(),
            ));
        }
        if !self.missed_auto_bump.is_empty() {
            lines.push(format!(
                "  • Done specs missed by auto-bump ({}): {}",
                self.missed_auto_bump.len(),
                self.missed_auto_bump[0].spec_id.dimmed(),
            ));
        }
        if !self.open_prs.is_empty() {
            lines.push(format!(
                "  • Open PRs ({}): PR-{}",
                self.open_prs.len(),
                self.open_prs[0].number,
            ));
        }
        if !self.dormant_leases.is_empty() {
            lines.push(format!(
                "  • Dormant leases ({}): {} ({})",
                self.dormant_leases.len(),
                self.dormant_leases[0].lease_id.dimmed(),
                self.dormant_leases[0].scope,
            ));
        }
        if !self.stale_reviewer_leases.is_empty() {
            lines.push(format!(
                "  • Stale reviewer leases ({}): PR-{}",
                self.stale_reviewer_leases.len(),
                self.stale_reviewer_leases[0].pr_number,
            ));
        }
        // Orphan Claude Code project dirs are intentionally OMITTED from
        // the inline summary — the scanner is system-wide (walks
        // ~/.claude/projects/) and surfaces cross-project state that
        // doesn't belong in the per-project status summary. Cross-project
        // cleanup still lives in `aida status --cleanup`'s full report
        // and `aida session prune --orphans` is the dedicated verb.
        lines.push(format!(
            "  {} `aida status --cleanup` for full report + recovery commands.",
            "→".dimmed(),
        ));

        Some(lines.join("\n  "))
    }

    /// Machine-readable JSON shape. Stable contract for TUI / scripting
    /// consumers — adding a new category appends a field; renaming or
    /// removing one is a breaking change.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "total": self.total(),
            "categories": {
                "uncommitted_wip": self.uncommitted_wip.iter().map(|i| serde_json::json!({
                    "worktree_path": i.worktree_path,
                    "branch": i.branch,
                    "scope": i.scope,
                    "modified_files": i.modified_files,
                    "age_hours": i.age_hours,
                })).collect::<Vec<_>>(),
                "sticky_in_progress": self.sticky_in_progress.iter().map(|i| serde_json::json!({
                    "spec_id": i.spec_id,
                    "title": i.title,
                    "branch": i.branch,
                    "unpushed_commits": i.unpushed_commits,
                    "pushed_commits": i.pushed_commits,
                    "age_hours": i.age_hours,
                })).collect::<Vec<_>>(),
                "branches_ahead_no_pr": self.branches_ahead_no_pr.iter().map(|i| serde_json::json!({
                    "branch": i.branch,
                    "commits_ahead": i.commits_ahead,
                    "has_upstream": i.has_upstream,
                })).collect::<Vec<_>>(),
                "missed_auto_bump": self.missed_auto_bump.iter().map(|i| serde_json::json!({
                    "spec_id": i.spec_id,
                    "title": i.title,
                    "landing_sha": i.landing_sha,
                })).collect::<Vec<_>>(),
                "open_prs": self.open_prs.iter().map(|i| serde_json::json!({
                    "number": i.number,
                    "title": i.title,
                    "head_branch": i.head_branch,
                    "ci_rollup": i.ci_rollup,
                    "mergeable": i.mergeable,
                })).collect::<Vec<_>>(),
                "dormant_leases": self.dormant_leases.iter().map(|i| serde_json::json!({
                    "lease_id": i.lease_id,
                    "scope": i.scope,
                    "role": i.role,
                    "worktree_path": i.worktree_path,
                    "age_hours": i.age_hours,
                    "spec_done": i.spec_done,
                })).collect::<Vec<_>>(),
                "stale_reviewer_leases": self.stale_reviewer_leases.iter().map(|i| serde_json::json!({
                    "lease_id": i.lease_id,
                    "pr_number": i.pr_number,
                    "worktree_path": i.worktree_path,
                    "age_hours": i.age_hours,
                })).collect::<Vec<_>>(),
                "orphan_project_dirs": self.orphan_project_dirs.iter().map(|i| serde_json::json!({
                    "path": i.path,
                    "decoded_cwd": i.decoded_cwd,
                    "jsonl_count": i.jsonl_count,
                })).collect::<Vec<_>>(),
                "claimed_done_diverged": self.claimed_done_diverged.iter().map(|i| {
                    let (kind, branch, modified_files, age_hours) = match &i.kind {
                        DivergenceKind::DirtyWorktree { branch, modified_files, age_hours } => (
                            "dirty_worktree",
                            Some(branch.clone()),
                            Some(*modified_files),
                            Some(*age_hours),
                        ),
                        DivergenceKind::NoCommitNoPr => ("no_commit_no_pr", None, None, None),
                    };
                    serde_json::json!({
                        "spec_id": i.spec_id,
                        "title": i.title,
                        "claimed_status": i.claimed_status,
                        "kind": kind,
                        "branch": branch,
                        "modified_files": modified_files,
                        "age_hours": age_hours,
                    })
                }).collect::<Vec<_>>(),
            }
        })
    }

    /// Render the text "Needs attention" report. `verbose` lifts the
    /// max-3-per-category cap so every item prints. The trailing Healthy
    /// footer enumerates categories that came back empty (when at least
    /// one other category has items) — explicit confirmation that
    /// nothing was missed.
    pub fn render(&self, verbose: bool, mut w: impl Write) -> std::io::Result<()> {
        writeln!(w, "{}", "─── Needs attention ───".bold())?;
        writeln!(w)?;

        if self.is_empty() {
            writeln!(w, "  {} Nothing needs cleanup attention.", "✓".green())?;
            writeln!(w)?;
            return Ok(());
        }

        let cap = if verbose { usize::MAX } else { 3 };

        render_uncommitted_wip(&self.uncommitted_wip, cap, &mut w)?;
        render_claimed_done_diverged(&self.claimed_done_diverged, cap, &mut w)?;
        render_sticky_in_progress(&self.sticky_in_progress, cap, &mut w)?;
        render_branches_ahead(&self.branches_ahead_no_pr, cap, &mut w)?;
        render_missed_auto_bump(&self.missed_auto_bump, cap, &mut w)?;
        render_open_prs(&self.open_prs, self.forge_kind, cap, &mut w)?;
        render_dormant_leases(&self.dormant_leases, cap, &mut w)?;
        render_stale_reviewer_leases(&self.stale_reviewer_leases, cap, &mut w)?;
        render_orphan_project_dirs(&self.orphan_project_dirs, cap, &mut w)?;

        // Healthy footer — enumerate categories that came back empty so
        // the operator sees explicitly which sweeps passed clean. Only
        // printed when at least one category had items (the all-clear
        // case above already covered the everything-clean output).
        let mut healthy_lines: Vec<&str> = Vec::new();
        if self.uncommitted_wip.is_empty() {
            healthy_lines.push("No uncommitted work at risk.");
        }
        if self.claimed_done_diverged.is_empty() {
            healthy_lines.push("No specs claiming Done that the substrate contradicts.");
        }
        if self.sticky_in_progress.is_empty() {
            healthy_lines.push("No specs In Progress without an active lease.");
        }
        if self.branches_ahead_no_pr.is_empty() {
            healthy_lines.push("No local branches ahead of main missing a PR.");
        }
        if self.missed_auto_bump.is_empty() {
            healthy_lines.push("No Done specs missed by the auto-bump scanner.");
        }
        if self.open_prs.is_empty() {
            healthy_lines.push("No open PRs.");
        }
        if self.dormant_leases.is_empty() {
            healthy_lines.push("No dormant leases.");
        }
        if self.stale_reviewer_leases.is_empty() {
            healthy_lines.push("No stale reviewer leases on merged PRs.");
        }
        if self.orphan_project_dirs.is_empty() {
            healthy_lines.push("No orphan Claude Code project dirs.");
        }
        if !healthy_lines.is_empty() {
            writeln!(w, "{}", "─── Healthy ───".bold())?;
            for line in healthy_lines {
                writeln!(w, "  {} {}", "✓".green(), line)?;
            }
            writeln!(w)?;
        }

        writeln!(
            w,
            "  Total: {} item{} need attention.",
            self.total(),
            if self.total() == 1 { "" } else { "s" }
        )?;
        Ok(())
    }
}

fn render_uncommitted_wip(
    items: &[UncommittedWipItem],
    cap: usize,
    w: &mut impl Write,
) -> std::io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    writeln!(
        w,
        "{} Uncommitted work at risk ({}):",
        "⚠".yellow(),
        items.len()
    )?;
    for item in items.iter().take(cap) {
        let scope = item.scope.as_deref().unwrap_or("(no lease)");
        writeln!(
            w,
            "    {} — {} modified file{} on branch `{}` in {} ({}h, no commits)",
            scope.bold(),
            item.modified_files,
            if item.modified_files == 1 { "" } else { "s" },
            item.branch,
            item.worktree_path.display(),
            item.age_hours,
        )?;
    }
    print_overflow(items.len(), cap, w)?;
    writeln!(w, "    → commit the work, or `aida queue recover <spec>`")?;
    writeln!(w)?;
    Ok(())
}

fn render_sticky_in_progress(
    items: &[StickyInProgressItem],
    cap: usize,
    w: &mut impl Write,
) -> std::io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    writeln!(
        w,
        "{} Specs {} In Progress without active lease ({}):",
        "⚠".yellow(),
        "◐".yellow(),
        items.len()
    )?;
    for item in items.iter().take(cap) {
        let branch = item.branch.as_deref().unwrap_or("(no branch)");
        let commits = match (item.unpushed_commits, item.pushed_commits) {
            (0, 0) => "no commits".to_string(),
            (n, 0) => format!("{} unpushed commit{}", n, if n == 1 { "" } else { "s" }),
            (0, n) => format!(
                "{} pushed commit{}, no PR",
                n,
                if n == 1 { "" } else { "s" }
            ),
            (u, p) => format!(
                "{} unpushed + {} pushed commit{}",
                u,
                p,
                if u + p == 1 { "" } else { "s" }
            ),
        };
        let age = item
            .age_hours
            .map(|h| format!(", {}h", h))
            .unwrap_or_default();
        writeln!(
            w,
            "    {} — branch `{}` has {}{}",
            item.spec_id.bold(),
            branch,
            commits,
            age
        )?;
        if !item.title.is_empty() {
            writeln!(w, "        {}", item.title.dimmed())?;
        }
    }
    print_overflow(items.len(), cap, w)?;
    writeln!(
        w,
        "    → `aida queue recover <spec>`  or  `aida pull` if the spec already shipped"
    )?;
    writeln!(w)?;
    Ok(())
}

fn render_branches_ahead(
    items: &[BranchAheadItem],
    cap: usize,
    w: &mut impl Write,
) -> std::io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    writeln!(
        w,
        "{} Local branches ahead of main with no PR ({}):",
        "⚠".yellow(),
        items.len()
    )?;
    for item in items.iter().take(cap) {
        let push_state = if item.has_upstream {
            "pushed"
        } else {
            "local-only"
        };
        writeln!(
            w,
            "    `{}` — {} commit{} ahead ({})",
            item.branch.bold(),
            item.commits_ahead,
            if item.commits_ahead == 1 { "" } else { "s" },
            push_state,
        )?;
    }
    print_overflow(items.len(), cap, w)?;
    writeln!(w, "    → open a PR, or merge/abandon the branch")?;
    writeln!(w)?;
    Ok(())
}

fn render_missed_auto_bump(
    items: &[MissedAutoBumpItem],
    cap: usize,
    w: &mut impl Write,
) -> std::io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    writeln!(
        w,
        "{} Specs {} Done missed by auto-bump ({}):",
        "⚠".yellow(),
        "◉".green(),
        items.len()
    )?;
    for item in items.iter().take(cap) {
        let short = item.landing_sha.get(..8).unwrap_or(&item.landing_sha);
        writeln!(
            w,
            "    {} — landed at {}",
            item.spec_id.bold(),
            short.dimmed()
        )?;
        if !item.title.is_empty() {
            writeln!(w, "        {}", item.title.dimmed())?;
        }
    }
    print_overflow(items.len(), cap, w)?;
    writeln!(
        w,
        "    → `aida db reconcile-status` (or `--spec <ID>` for one)"
    )?;
    writeln!(w)?;
    Ok(())
}

fn render_open_prs(
    items: &[OpenPrItem],
    forge_kind: Option<crate::forge::ForgeKind>,
    cap: usize,
    w: &mut impl Write,
) -> std::io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    // STORY-508/TASK-651: forge-aware noun + command chain. Unresolved (test
    // default()) renders as GitHub for back-compat.
    let kind = forge_kind.unwrap_or(crate::forge::ForgeKind::GitHub);
    let noun = kind.change_noun();
    writeln!(
        w,
        "{} Open {}s awaiting review/merge ({}):",
        "⚠".yellow(),
        noun,
        items.len()
    )?;
    for item in items.iter().take(cap) {
        let ci = item.ci_rollup.as_deref().unwrap_or("?");
        let mergeable = item.mergeable.as_deref().unwrap_or("?");
        writeln!(
            w,
            "    PR-{} `{}` — CI {} · mergeable {}",
            item.number.to_string().bold(),
            item.head_branch,
            ci,
            mergeable,
        )?;
        if !item.title.is_empty() {
            writeln!(w, "        {}", item.title.dimmed())?;
        }
    }
    print_overflow(items.len(), cap, w)?;
    // STORY-508/TASK-651: the watch→merge→pull hint, in each forge's command
    // shape (gh checks/--delete-branch vs glab ci status/--remove-source-branch;
    // pure-git names no forge CLI).
    let chain = match kind {
        crate::forge::ForgeKind::GitHub => {
            "`gh pr checks <N> --watch && gh pr merge <N> --squash --delete-branch && aida pull`"
                .to_string()
        }
        crate::forge::ForgeKind::GitLab => {
            "`glab ci status && glab mr merge <N> --squash --remove-source-branch && aida pull`"
                .to_string()
        }
        crate::forge::ForgeKind::None => {
            "merge each change to your default branch, then `aida pull`".to_string()
        }
    };
    writeln!(w, "    → {chain}")?;
    writeln!(w)?;
    Ok(())
}

fn render_dormant_leases(
    items: &[DormantLeaseItem],
    cap: usize,
    w: &mut impl Write,
) -> std::io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    // BUG-376: count the *"lingering implementer with done queue"*
    // sub-cohort so the section header can hint at the subcategory's
    // presence before the detail lines render it.
    let lingering = items.iter().filter(|i| i.spec_done).count();
    let header_suffix = if lingering > 0 {
        format!(", {lingering} lingering after ship")
    } else {
        String::new()
    };
    writeln!(
        w,
        "{} Dormant leases ({}{}):",
        "⚠".yellow(),
        items.len(),
        header_suffix
    )?;
    for item in items.iter().take(cap) {
        let role = item.role.as_deref().unwrap_or("-");
        // BUG-376: annotate the "lingering implementer with done queue"
        // subcategory inline. Informational tag (ℹ), not a warning —
        // the work shipped; the only issue is that the agent did not
        // exit. Recovery verb is the same as a plain dormant lease.
        let lingering_tag = if item.spec_done {
            format!(" {} lingering after ship", "ℹ".cyan())
        } else {
            String::new()
        };
        writeln!(
            w,
            "    {} {} · role {} · {}h ({}){}",
            item.lease_id.dimmed(),
            item.scope.bold(),
            role,
            item.age_hours,
            item.worktree_path.display(),
            lingering_tag,
        )?;
    }
    print_overflow(items.len(), cap, w)?;
    writeln!(
        w,
        "    → resume the worktree, or `aida session end <lease>` to release"
    )?;
    if lingering > 0 {
        writeln!(
            w,
            "    {} `lingering after ship`: BUG-376 — implementer shipped + queue-done correctly but did not exit; safe to `aida session end`",
            "ℹ".cyan(),
        )?;
    }
    writeln!(w)?;
    Ok(())
}

fn render_stale_reviewer_leases(
    items: &[StaleReviewerLeaseItem],
    cap: usize,
    w: &mut impl Write,
) -> std::io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    writeln!(
        w,
        "{} Stale reviewer leases on merged PRs ({}):",
        "⚠".yellow(),
        items.len()
    )?;
    for item in items.iter().take(cap) {
        writeln!(
            w,
            "    {} PR-{} · {}h · {}",
            item.lease_id.dimmed(),
            item.pr_number.to_string().bold(),
            item.age_hours,
            item.worktree_path.display(),
        )?;
    }
    print_overflow(items.len(), cap, w)?;
    writeln!(w, "    → `aida session end <lease>`")?;
    writeln!(w)?;
    Ok(())
}

fn render_orphan_project_dirs(
    items: &[OrphanProjectDirItem],
    cap: usize,
    w: &mut impl Write,
) -> std::io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    writeln!(
        w,
        "{} Orphan Claude Code project dirs ({}):",
        "⚠".yellow(),
        items.len()
    )?;
    for item in items.iter().take(cap) {
        writeln!(
            w,
            "    {} — cwd {} missing ({} jsonl{})",
            item.path.display().to_string().dimmed(),
            item.decoded_cwd.yellow(),
            item.jsonl_count,
            if item.jsonl_count == 1 { "" } else { "s" },
        )?;
    }
    print_overflow(items.len(), cap, w)?;
    writeln!(w, "    → `aida session prune --orphans`")?;
    writeln!(w)?;
    Ok(())
}

fn render_claimed_done_diverged(
    items: &[ClaimedDoneDivergedItem],
    cap: usize,
    w: &mut impl Write,
) -> std::io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    writeln!(
        w,
        "{} Claimed Done but substrate disagrees ({}):",
        "⚠".yellow(),
        items.len()
    )?;
    for item in items.iter().take(cap) {
        match &item.kind {
            DivergenceKind::DirtyWorktree {
                branch,
                modified_files,
                age_hours,
            } => {
                writeln!(
                    w,
                    "    {} ({}) worktree on `{}` has {} modified file{} ({}h) — uncommitted, yet spec is {}.",
                    item.spec_id.yellow(),
                    item.title.dimmed(),
                    branch.dimmed(),
                    modified_files,
                    if *modified_files == 1 { "" } else { "s" },
                    age_hours,
                    item.claimed_status,
                )?;
                writeln!(
                    w,
                    "      → commit + `aida pr ship`, or reopen the spec (`aida edit {} --status in-progress`).",
                    item.spec_id
                )?;
            }
            DivergenceKind::NoCommitNoPr => {
                writeln!(
                    w,
                    "    {} ({}) is {} but no commit references it and no PR exists.",
                    item.spec_id.yellow(),
                    item.title.dimmed(),
                    item.claimed_status,
                )?;
                writeln!(
                    w,
                    "      → verify the work actually shipped, or reopen (`aida edit {} --status in-progress`).",
                    item.spec_id
                )?;
            }
        }
    }
    print_overflow(items.len(), cap, w)?;
    writeln!(w)?;
    Ok(())
}

/// One spec's local-vs-substrate facts, fed to [`detect_claimed_done_divergence`].
/// Pure data so the detector is filesystem-free and unit-testable; `main.rs`
/// gathers these from leases + git + the store. trace:STORY-469 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct ClaimedDoneInput {
    pub spec_id: String,
    pub title: String,
    /// The spec's substrate status word ("Done" / "Completed" / other).
    pub status: String,
    /// True when an active (live or dormant) lease covers this spec.
    pub has_active_lease: bool,
    /// Branch the lease covers (for the dirty-worktree message).
    pub branch: Option<String>,
    /// Count of uncommitted modifications in the lease's worktree (0 = clean).
    pub modified_files: usize,
    /// Lease age in hours (for the message).
    pub age_hours: i64,
    /// True when at least one commit references this spec (subject trailer or
    /// landed commit) — substrate evidence the work exists.
    pub has_commit: bool,
    /// True when an open or merged PR exists for the spec's branch.
    pub has_pr: bool,
}

/// STORY-469 Guard 3 (pure core): flag specs whose status claims Done/Completed
/// but whose local reality contradicts the claim. Two contradictions fire:
///   1. an active lease + a dirty worktree (work still on disk despite Done), and
///   2. no commit references the spec AND no PR exists (no shipping evidence).
/// Specs whose status is not Done/Completed are never flagged here (sticky
/// In-Progress is a separate category). Returns the diverged items in input
/// order. trace:STORY-469 | ai:claude
pub(crate) fn detect_claimed_done_divergence(
    inputs: &[ClaimedDoneInput],
) -> Vec<ClaimedDoneDivergedItem> {
    let mut out = Vec::new();
    for input in inputs {
        let claims_done = input.status.eq_ignore_ascii_case("done")
            || input.status.eq_ignore_ascii_case("completed");
        if !claims_done {
            continue;
        }
        // Contradiction 1: active lease + dirty worktree. Highest signal — the
        // agent declared Done while uncommitted work sits in the worktree.
        if input.has_active_lease && input.modified_files > 0 {
            out.push(ClaimedDoneDivergedItem {
                spec_id: input.spec_id.clone(),
                title: input.title.clone(),
                claimed_status: input.status.clone(),
                kind: DivergenceKind::DirtyWorktree {
                    branch: input
                        .branch
                        .clone()
                        .unwrap_or_else(|| "(detached)".to_string()),
                    modified_files: input.modified_files,
                    age_hours: input.age_hours,
                },
            });
            continue;
        }
        // Contradiction 2: no commit + no PR. The Done claim has no substrate
        // evidence at all (a `Completed` spec is merge-driven so it always has
        // a commit — this realistically only fires for `Done`).
        if !input.has_commit && !input.has_pr {
            out.push(ClaimedDoneDivergedItem {
                spec_id: input.spec_id.clone(),
                title: input.title.clone(),
                claimed_status: input.status.clone(),
                kind: DivergenceKind::NoCommitNoPr,
            });
        }
    }
    out
}

fn print_overflow(total: usize, cap: usize, w: &mut impl Write) -> std::io::Result<()> {
    if total > cap {
        writeln!(
            w,
            "    {} ({} more — pass --verbose to show all)",
            "…".dimmed(),
            total - cap
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_ansi(s: &str) -> String {
        // ESC[...m sequences are pure ASCII — we can scan the string by
        // char and emit anything that isn't part of one, preserving the
        // multi-byte UTF-8 glyphs (box-drawing dashes, em-dash, the ⚠
        // marker) verbatim.
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' && chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                while let Some(&p) = chars.peek() {
                    chars.next();
                    if p.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn render_to_string(report: &CleanupReport, verbose: bool) -> String {
        let mut buf = Vec::new();
        report.render(verbose, &mut buf).unwrap();
        strip_ansi(&String::from_utf8(buf).unwrap())
    }

    #[test]
    fn empty_report_renders_all_clear() {
        colored::control::set_override(false);
        let report = CleanupReport::default();
        let out = render_to_string(&report, false);
        colored::control::unset_override();
        assert!(out.contains("Nothing needs cleanup attention"));
        assert!(report.is_empty());
        assert!(report.summary_line().is_none());
    }

    #[test]
    fn summary_line_present_when_non_empty() {
        let mut report = CleanupReport::default();
        // Use a project-scoped category — orphan_project_dirs are
        // deliberately excluded from the inline summary (cross-project
        // state surfaces in --cleanup only). trace:TASK-1-099-companion
        report.uncommitted_wip.push(UncommittedWipItem {
            worktree_path: "/x".into(),
            branch: "demo-branch".into(),
            scope: Some("TASK-1".into()),
            modified_files: 3,
            age_hours: 5,
        });
        let line = report.summary_line().unwrap();
        assert!(line.contains("1 item need"));
        assert!(line.contains("aida status --cleanup"));
    }

    #[test]
    fn summary_line_skips_orphan_only_reports() {
        // Cross-project orphan Claude Code project dirs surface ONLY in
        // --cleanup, not in the per-project status summary. A report
        // containing nothing but orphan dirs returns None so the
        // operator's per-project status stays uncluttered.
        // trace:TASK-1-099-companion | ai:claude
        let mut report = CleanupReport::default();
        report.orphan_project_dirs.push(OrphanProjectDirItem {
            path: "/tmp/foo".into(),
            decoded_cwd: "/missing".to_string(),
            jsonl_count: 2,
        });
        assert!(report.summary_line().is_none());
    }

    #[test]
    fn each_category_prints_recovery_verb() {
        colored::control::set_override(false);
        let mut report = CleanupReport::default();
        report.uncommitted_wip.push(UncommittedWipItem {
            worktree_path: "/x".into(),
            branch: "b".into(),
            scope: Some("TASK-1".into()),
            modified_files: 3,
            age_hours: 5,
        });
        report.sticky_in_progress.push(StickyInProgressItem {
            spec_id: "TASK-2".into(),
            title: "t".into(),
            branch: Some("b2".into()),
            unpushed_commits: 1,
            pushed_commits: 0,
            age_hours: Some(2),
        });
        report.branches_ahead_no_pr.push(BranchAheadItem {
            branch: "b3".into(),
            commits_ahead: 1,
            has_upstream: false,
        });
        report.missed_auto_bump.push(MissedAutoBumpItem {
            spec_id: "TASK-3".into(),
            title: "t3".into(),
            landing_sha: "deadbeefcafe".into(),
        });
        report.open_prs.push(OpenPrItem {
            number: 42,
            title: "t4".into(),
            head_branch: "b4".into(),
            ci_rollup: Some("pass".into()),
            mergeable: Some("clean".into()),
            review_decision: None,
        });
        report.dormant_leases.push(DormantLeaseItem {
            lease_id: "abc".into(),
            scope: "TASK-4".into(),
            role: Some("implementer".into()),
            worktree_path: "/y".into(),
            age_hours: 6,
            spec_done: false,
        });
        report.stale_reviewer_leases.push(StaleReviewerLeaseItem {
            lease_id: "def".into(),
            pr_number: 99,
            worktree_path: "/z".into(),
            age_hours: 12,
        });
        report.orphan_project_dirs.push(OrphanProjectDirItem {
            path: "/p".into(),
            decoded_cwd: "/missing".into(),
            jsonl_count: 4,
        });
        let out = render_to_string(&report, false);
        colored::control::unset_override();

        assert!(out.contains("aida queue recover"));
        assert!(out.contains("aida db reconcile-status"));
        assert!(out.contains("gh pr merge"));
        assert!(out.contains("aida session end"));
        assert!(out.contains("aida session prune --orphans"));
        assert!(out.contains("Total: 8 items need attention"));
    }

    // STORY-508/TASK-651: the open-change section is forge-aware — GitLab gets
    // "Open MRs" + glab commands, pure-git names no forge CLI.
    #[test]
    fn open_changes_hint_is_forge_aware() {
        colored::control::set_override(false);
        let mut report = CleanupReport::default();
        report.open_prs.push(OpenPrItem {
            number: 42,
            title: "t".into(),
            head_branch: "b".into(),
            ci_rollup: Some("pass".into()),
            mergeable: Some("clean".into()),
            review_decision: None,
        });

        report.forge_kind = Some(crate::forge::ForgeKind::GitLab);
        let gitlab = render_to_string(&report, false);
        assert!(gitlab.contains("Open MRs awaiting"), "{gitlab}");
        assert!(gitlab.contains("glab mr merge"), "{gitlab}");
        assert!(gitlab.contains("glab ci status"), "{gitlab}");
        assert!(!gitlab.contains("gh pr"), "{gitlab}");

        report.forge_kind = Some(crate::forge::ForgeKind::None);
        let pure = render_to_string(&report, false);
        assert!(pure.contains("Open changes awaiting"), "{pure}");
        assert!(!pure.contains("gh pr"), "{pure}");
        assert!(!pure.contains("glab "), "{pure}");
        colored::control::unset_override();
    }

    #[test]
    fn cap_applied_without_verbose() {
        colored::control::set_override(false);
        let mut report = CleanupReport::default();
        for i in 0..5 {
            report.orphan_project_dirs.push(OrphanProjectDirItem {
                path: format!("/dir-{i}").into(),
                decoded_cwd: format!("/cwd-{i}"),
                jsonl_count: 1,
            });
        }
        let out_short = render_to_string(&report, false);
        let out_verbose = render_to_string(&report, true);
        colored::control::unset_override();

        // Default cap=3 → 2 hidden, overflow line says so.
        assert!(out_short.contains("(2 more — pass --verbose to show all)"));
        assert!(!out_short.contains("/dir-4"));
        // Verbose: every item present.
        assert!(out_verbose.contains("/dir-4"));
        assert!(!out_verbose.contains("more — pass --verbose"));
    }

    #[test]
    fn healthy_footer_lists_empty_categories() {
        colored::control::set_override(false);
        let mut report = CleanupReport::default();
        report.uncommitted_wip.push(UncommittedWipItem {
            worktree_path: "/x".into(),
            branch: "b".into(),
            scope: None,
            modified_files: 1,
            age_hours: 1,
        });
        let out = render_to_string(&report, false);
        colored::control::unset_override();

        assert!(out.contains("─── Healthy ───"));
        assert!(out.contains("No dormant leases."));
        assert!(out.contains("No orphan Claude Code project dirs."));
        // The one populated category must NOT appear in the Healthy block.
        // We assert that by checking the line count: 8 empty categories =>
        // 8 healthy lines (STORY-469 added the claimed-Done-divergence one).
        let healthy_count = out
            .lines()
            .filter(|l| l.trim_start().starts_with("✓"))
            .count();
        assert_eq!(healthy_count, 8);
    }

    /// BUG-376: a dormant lease whose scope is a Done/Completed spec is
    /// surfaced as a *"lingering implementer with done queue"* subcategory
    /// within the Dormant section — informational annotation on the
    /// detail line + a section-header count + an explanatory footer.
    /// Plain dormant leases (work still in progress, lease just idle)
    /// render unchanged.
    #[test]
    fn dormant_lease_with_spec_done_surfaces_lingering_subcategory() {
        colored::control::set_override(false);
        let mut report = CleanupReport::default();
        report.dormant_leases.push(DormantLeaseItem {
            lease_id: "abc12345".into(),
            scope: "TASK-376".into(),
            role: Some("implementer".into()),
            worktree_path: "/tmp/task-376".into(),
            age_hours: 2,
            spec_done: true,
        });
        report.dormant_leases.push(DormantLeaseItem {
            lease_id: "def67890".into(),
            scope: "TASK-377".into(),
            role: Some("implementer".into()),
            worktree_path: "/tmp/task-377".into(),
            age_hours: 4,
            spec_done: false,
        });
        let out = render_to_string(&report, false);
        colored::control::unset_override();

        // Header annotates the subcategory count.
        assert!(
            out.contains("Dormant leases (2, 1 lingering after ship)"),
            "header missing lingering subcategory count: {out}"
        );
        // The lingering lease gets the inline tag.
        let lingering_line = out
            .lines()
            .find(|l| l.contains("TASK-376"))
            .expect("TASK-376 line missing");
        assert!(
            lingering_line.contains("lingering after ship"),
            "lingering tag missing on TASK-376 line: {lingering_line}"
        );
        // The non-lingering lease must NOT get the tag.
        let plain_line = out
            .lines()
            .find(|l| l.contains("TASK-377"))
            .expect("TASK-377 line missing");
        assert!(
            !plain_line.contains("lingering after ship"),
            "plain dormant lease incorrectly tagged: {plain_line}"
        );
        // Footer explains the subcategory + names BUG-376.
        assert!(
            out.contains("BUG-376"),
            "footer missing BUG-376 attribution: {out}"
        );
        // Recovery verb is the same as plain dormant — informational, not error.
        assert!(out.contains("aida session end"));
    }

    /// BUG-376: when ZERO dormant leases are "lingering after ship",
    /// neither the header count nor the footer mention the subcategory.
    #[test]
    fn dormant_lease_subcategory_absent_when_no_spec_done() {
        colored::control::set_override(false);
        let mut report = CleanupReport::default();
        report.dormant_leases.push(DormantLeaseItem {
            lease_id: "abc12345".into(),
            scope: "TASK-377".into(),
            role: Some("implementer".into()),
            worktree_path: "/tmp/task-377".into(),
            age_hours: 4,
            spec_done: false,
        });
        let out = render_to_string(&report, false);
        colored::control::unset_override();

        assert!(out.contains("Dormant leases (1):"), "{out}");
        assert!(!out.contains("lingering"), "{out}");
        assert!(!out.contains("BUG-376"), "{out}");
    }

    /// BUG-376: the JSON view exposes `spec_done` on every dormant lease
    /// so the TUI / scripted consumers can render the same subcategory.
    #[test]
    fn dormant_lease_json_includes_spec_done() {
        let mut report = CleanupReport::default();
        report.dormant_leases.push(DormantLeaseItem {
            lease_id: "abc".into(),
            scope: "TASK-376".into(),
            role: Some("implementer".into()),
            worktree_path: "/tmp/x".into(),
            age_hours: 1,
            spec_done: true,
        });
        let v = report.to_json();
        assert_eq!(
            v["categories"]["dormant_leases"][0]["spec_done"].as_bool(),
            Some(true)
        );
    }

    #[test]
    fn json_shape_round_trips_total_and_arrays() {
        let mut report = CleanupReport::default();
        report.orphan_project_dirs.push(OrphanProjectDirItem {
            path: "/p".into(),
            decoded_cwd: "/missing".into(),
            jsonl_count: 4,
        });
        let v = report.to_json();
        assert_eq!(v["total"].as_u64(), Some(1));
        assert_eq!(
            v["categories"]["orphan_project_dirs"][0]["decoded_cwd"].as_str(),
            Some("/missing")
        );
        // Empty categories serialize as `[]`, not missing.
        assert!(v["categories"]["uncommitted_wip"].is_array());
        assert_eq!(
            v["categories"]["uncommitted_wip"].as_array().unwrap().len(),
            0
        );
    }

    // ── STORY-469 Guard 3: claimed-Done-vs-substrate divergence ──

    fn done_input(spec_id: &str) -> ClaimedDoneInput {
        ClaimedDoneInput {
            spec_id: spec_id.to_string(),
            title: format!("{spec_id} title"),
            status: "Done".to_string(),
            has_active_lease: false,
            branch: None,
            modified_files: 0,
            age_hours: 0,
            // Default to "has evidence" so the no-commit-no-pr signal only
            // fires when a test explicitly removes it.
            has_commit: true,
            has_pr: true,
        }
    }

    #[test]
    fn guard3_flags_done_spec_with_dirty_worktree() {
        let inputs = vec![ClaimedDoneInput {
            has_active_lease: true,
            branch: Some("task-542".to_string()),
            modified_files: 3,
            age_hours: 6,
            ..done_input("TASK-542")
        }];
        let out = detect_claimed_done_divergence(&inputs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].spec_id, "TASK-542");
        assert_eq!(
            out[0].kind,
            DivergenceKind::DirtyWorktree {
                branch: "task-542".to_string(),
                modified_files: 3,
                age_hours: 6,
            }
        );
    }

    #[test]
    fn guard3_flags_done_spec_with_no_commit_and_no_pr() {
        let inputs = vec![ClaimedDoneInput {
            has_commit: false,
            has_pr: false,
            ..done_input("STORY-487")
        }];
        let out = detect_claimed_done_divergence(&inputs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].spec_id, "STORY-487");
        assert_eq!(out[0].kind, DivergenceKind::NoCommitNoPr);
    }

    #[test]
    fn guard3_dirty_worktree_takes_precedence_over_no_evidence() {
        // A Done spec that is BOTH dirty AND has no commit/PR is reported once,
        // under the higher-signal dirty-worktree kind.
        let inputs = vec![ClaimedDoneInput {
            has_active_lease: true,
            branch: Some("task-9".to_string()),
            modified_files: 1,
            has_commit: false,
            has_pr: false,
            ..done_input("TASK-9")
        }];
        let out = detect_claimed_done_divergence(&inputs);
        assert_eq!(out.len(), 1, "reported once, not twice");
        assert!(matches!(out[0].kind, DivergenceKind::DirtyWorktree { .. }));
    }

    #[test]
    fn guard3_no_false_positive_on_clean_done_spec() {
        // The normal happy path: Done with a commit + a PR, clean worktree (or
        // no lease at all). Must NOT be flagged.
        let inputs = vec![
            done_input("TASK-1"),
            ClaimedDoneInput {
                has_active_lease: true,
                branch: Some("task-2".to_string()),
                modified_files: 0, // clean
                ..done_input("TASK-2")
            },
        ];
        let out = detect_claimed_done_divergence(&inputs);
        assert!(out.is_empty(), "got false positives: {out:?}");
    }

    #[test]
    fn guard3_ignores_non_done_specs() {
        // An In-Progress spec (even dirty / no commit) is NOT this category's
        // concern — sticky-in-progress covers it.
        let inputs = vec![ClaimedDoneInput {
            status: "InProgress".to_string(),
            has_active_lease: true,
            modified_files: 5,
            has_commit: false,
            has_pr: false,
            ..done_input("TASK-3")
        }];
        let out = detect_claimed_done_divergence(&inputs);
        assert!(out.is_empty(), "non-Done spec should be skipped: {out:?}");
    }

    #[test]
    fn guard3_completed_status_also_qualifies() {
        let inputs = vec![ClaimedDoneInput {
            status: "Completed".to_string(),
            has_commit: false,
            has_pr: false,
            ..done_input("TASK-4")
        }];
        let out = detect_claimed_done_divergence(&inputs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].claimed_status, "Completed");
    }

    #[test]
    fn guard3_counts_in_total_and_json_and_render() {
        let mut report = CleanupReport::default();
        report.claimed_done_diverged = detect_claimed_done_divergence(&[ClaimedDoneInput {
            has_active_lease: true,
            branch: Some("task-542".to_string()),
            modified_files: 3,
            age_hours: 6,
            ..done_input("TASK-542")
        }]);
        assert_eq!(report.total(), 1);

        let v = report.to_json();
        assert_eq!(v["total"].as_u64(), Some(1));
        assert_eq!(
            v["categories"]["claimed_done_diverged"][0]["spec_id"].as_str(),
            Some("TASK-542")
        );
        assert_eq!(
            v["categories"]["claimed_done_diverged"][0]["kind"].as_str(),
            Some("dirty_worktree")
        );

        let rendered = render_to_string(&report, false);
        assert!(
            rendered.contains("Claimed Done but substrate disagrees"),
            "render: {rendered}"
        );
        assert!(rendered.contains("TASK-542"), "render: {rendered}");
        assert!(
            rendered.contains("3 modified files"),
            "render shows the modified count: {rendered}"
        );
    }
}
