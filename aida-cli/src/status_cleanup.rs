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
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    /// One-line summary suitable for appending to the default
    /// `aida status` output when the report is non-empty.
    pub fn summary_line(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let n = self.total();
        Some(format!(
            "{} {} item{} need cleanup attention — `aida status --cleanup` for details",
            "⚠".yellow(),
            n,
            if n == 1 { "" } else { "s" },
        ))
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
        render_sticky_in_progress(&self.sticky_in_progress, cap, &mut w)?;
        render_branches_ahead(&self.branches_ahead_no_pr, cap, &mut w)?;
        render_missed_auto_bump(&self.missed_auto_bump, cap, &mut w)?;
        render_open_prs(&self.open_prs, cap, &mut w)?;
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

fn render_open_prs(items: &[OpenPrItem], cap: usize, w: &mut impl Write) -> std::io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    writeln!(
        w,
        "{} Open PRs awaiting review/merge ({}):",
        "⚠".yellow(),
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
    writeln!(
        w,
        "    → `gh pr checks <N> --watch && gh pr merge <N> --squash --delete-branch && aida pull`"
    )?;
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
        report.orphan_project_dirs.push(OrphanProjectDirItem {
            path: "/tmp/foo".into(),
            decoded_cwd: "/missing".to_string(),
            jsonl_count: 2,
        });
        let line = report.summary_line().unwrap();
        assert!(line.contains("1 item need"));
        assert!(line.contains("aida status --cleanup"));
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
        // We assert that by checking the line count: 7 empty categories =>
        // 7 healthy lines.
        let healthy_count = out
            .lines()
            .filter(|l| l.trim_start().starts_with("✓"))
            .count();
        assert_eq!(healthy_count, 7);
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
}
