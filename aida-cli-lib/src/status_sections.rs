//! status report sections: worktrees, open and recently merged PRs, remote activity, findings, working tree and process hygiene (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// TASK-53: list distinct files touched by commits on `branch` since
/// `since` (a git-friendly time string like "14 days ago"). Returns
/// an empty vec on any git error or when the branch has no commits in
/// the window. Used by `aida session start`'s pre-flight conflict
/// warning so the user sees what another concurrent session has been
/// touching. trace:TASK-53 | ai:claude
pub(crate) fn recent_files_for_branch(
    repo: &std::path::Path,
    branch: &str,
    since: &str,
    max: usize,
) -> Vec<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        // Options first, then `--end-of-options` so a lease branch is
        // always a revision. trace:BUG-1622 | ai:claude
        .args([
            "log",
            &format!("--since={}", since),
            "--name-only",
            "--pretty=format:",
            git_arg_guard::END_OF_OPTIONS,
            branch,
            "--",
        ])
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    let mut seen = std::collections::BTreeSet::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            seen.insert(trimmed.to_string());
        }
        if seen.len() >= max {
            break;
        }
    }
    seen.into_iter().collect()
}

/// BUG-67: list every line of `git status --porcelain` inside `worktree`,
/// skipping the leading two-char status code so output is human-readable.
/// Ignored entries are excluded by default (no `--ignored=normal`), so
/// `target/`, `.aida/cache.db`, etc. never appear here — only tracked-
/// and-modified or untracked-but-not-ignored files. Returns an empty
/// vec when the worktree is clean OR when `git status` itself fails
/// (we treat an unparseable status as clean and let `git worktree
/// remove --force` produce the authoritative error). trace:BUG-67 | ai:claude
pub(crate) fn worktree_dirty_entries(worktree: &std::path::Path) -> Vec<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["status", "--porcelain"])
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// STORY-457: is this untracked path auto-flaggable as safe-to-remove? Editor
/// scratch, build droppings, and OS cruft — never source. Matched on the file
/// name / extension or a path segment. trace:STORY-457 | ai:claude
pub(crate) fn untracked_is_safe_to_remove(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    const SUFFIXES: &[&str] = &[
        ".bak", ".tmp", ".swp", ".swo", ".orig", ".pyc", ".pyo", ".class", "~",
    ];
    if SUFFIXES.iter().any(|s| name.ends_with(s)) {
        return true;
    }
    if name == ".DS_Store" || name == "Thumbs.db" {
        return true;
    }
    // Build/cache dirs anywhere in the path.
    path.split('/')
        .any(|seg| seg == "__pycache__" || seg == ".mypy_cache" || seg == ".pytest_cache")
}

/// STORY-456: structured per-worktree status row — the assembled merge of
/// `git worktree list`, the session lease covering it, live/dormant process
/// state, working-tree cleanliness, commits-ahead-of-default, and the open
/// PR (if any) on its branch. Drives both the text Worktrees section and the
/// `--json` projection so the two never drift. The PR-derived fields are
/// populated from a single batched `gh pr list` snapshot — never one gh call
/// per row. trace:STORY-456 | ai:claude
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WorktreeStatusRow {
    pub(crate) path: std::path::PathBuf,
    pub(crate) branch: String,
    /// Spec the worktree is tied to (lease scope, else branch-name inference).
    pub(crate) tied_spec: Option<String>,
    pub(crate) lease_scope: Option<String>,
    pub(crate) has_live: bool,
    pub(crate) dirty_count: usize,
    /// Commits the branch is ahead of the default ref. `None` when it could
    /// not be computed (detached HEAD, missing default ref, git failure).
    pub(crate) ahead: Option<u32>,
    pub(crate) pr_number: Option<u64>,
    pub(crate) pr_ci: Option<String>,
    pub(crate) pr_mergeable: Option<String>,
    /// Obsolescence verdict — TIED (in flight) vs OBSOLETE (safe to remove).
    pub(crate) obsolete: bool,
}

/// STORY-456: tie a worktree to a spec id. The lease scope is authoritative;
/// fall back to inferring `TASK-425` from a `task-425` / `task-425-2` branch
/// name so leaseless worktrees still get a spec hint. trace:STORY-456
pub(crate) fn infer_tied_spec(lease_scope: Option<&str>, branch: &str) -> Option<String> {
    if let Some(scope) = lease_scope {
        if !scope.is_empty() {
            return Some(scope.to_string());
        }
    }
    // Branch names are slugged lowercase (e.g. `task-425-2`,
    // `story-456-status-worktrees`). Recover a leading `<type>-<num>`.
    let mut parts = branch.splitn(3, '-');
    let kind = parts.next()?;
    let num = parts.next()?;
    if num.chars().all(|c| c.is_ascii_digit()) && !num.is_empty() {
        Some(format!("{}-{}", kind.to_uppercase(), num))
    } else {
        None
    }
}

/// STORY-456: pure assembly of the Worktrees rows from already-collected
/// inputs (no I/O) so the merge + obsolescence logic is unit-testable.
///
/// Obsolescence verdict (per spec): a worktree is TIED (in flight) if ANY of
/// — an active lease covers it, the tree is dirty, a live process runs there,
/// it has un-merged commits ahead of the default branch, or it has an open
/// PR. It is OBSOLETE (safe to remove) only when ALL of those are false: no
/// lease, clean tree, nothing live, nothing ahead, no open PR. The conjunction
/// keeps the "safe to remove" signal conservative. trace:STORY-456 | ai:claude
pub(crate) fn assemble_worktree_status_rows(
    worktrees: &[WorktreeRecord],
    main_root: &std::path::Path,
    leases: &[SessionLease],
    live: &[process_probe::LiveSession],
    pr_by_branch: &std::collections::HashMap<String, status_cleanup::OpenPrItem>,
    ahead_by_path: &std::collections::HashMap<std::path::PathBuf, u32>,
) -> Vec<WorktreeStatusRow> {
    let mut rows = Vec::new();
    for wt in worktrees {
        // Skip the main worktree and the AIDA-managed orphan store.
        if wt.path == *main_root || wt.branch.as_deref() == Some("aida-store") {
            continue;
        }
        let branch = wt
            .branch
            .clone()
            .unwrap_or_else(|| "(detached)".to_string());
        let dirty_count = worktree_dirty_entries(&wt.path).len();
        let lease = leases.iter().find(|l| l.worktree_path == wt.path);
        let lease_scope = lease.map(|l| l.scope.clone());
        let has_live = live
            .iter()
            .any(|s| !s.stale_cwd && (s.cwd == wt.path || s.cwd.starts_with(&wt.path)));
        let ahead = ahead_by_path.get(&wt.path).copied();
        let pr = pr_by_branch.get(&branch);
        let tied_spec = infer_tied_spec(lease_scope.as_deref(), &branch);

        let has_open_pr = pr.is_some();
        let has_unmerged = ahead.map(|a| a > 0).unwrap_or(false);
        let obsolete =
            lease.is_none() && dirty_count == 0 && !has_live && !has_unmerged && !has_open_pr;

        rows.push(WorktreeStatusRow {
            path: wt.path.clone(),
            branch,
            tied_spec,
            lease_scope,
            has_live,
            dirty_count,
            ahead,
            pr_number: pr.map(|p| p.number),
            pr_ci: pr.and_then(|p| p.ci_rollup.clone()),
            pr_mergeable: pr.and_then(|p| p.mergeable.clone()),
            obsolete,
        });
    }
    rows
}

/// BUG-609: above this many worktree rows the default `aida status` collapses
/// the section to a one-line summary instead of listing every row — fanned-out
/// agent worktrees accumulate (observed ~45, many abandoned) and the full list
/// drowns the rest of the status surface. `--all` always lists. Below the
/// threshold the list is short enough to print in full. trace:BUG-609
pub(crate) const WORKTREE_SUMMARY_THRESHOLD: usize = 8;

/// BUG-609: render one worktree row's display line (display-only; no I/O).
/// Pulled out of `print_status_worktrees_section` so the full-list path and any
/// future caller share one formatter. trace:BUG-609 | ai:claude
pub(crate) fn format_worktree_status_line(row: &WorktreeStatusRow) -> String {
    let dirty_part = if row.dirty_count == 0 {
        "clean".to_string()
    } else {
        format!("dirty({})", row.dirty_count).yellow().to_string()
    };
    let lease_part = match &row.lease_scope {
        Some(scope) => format!("lease:{scope}"),
        None => "no-lease".to_string(),
    };
    let live_part = if row.has_live {
        "live".green().to_string()
    } else if row.lease_scope.is_some() {
        "dormant".yellow().to_string()
    } else {
        String::new()
    };

    let mut line = format!(
        "  {} ({}) — {} · {}",
        row.path.display(),
        row.branch.cyan(),
        dirty_part,
        lease_part
    );
    if !live_part.is_empty() {
        line.push_str(&format!(" · {live_part}"));
    }
    if let Some(ahead) = row.ahead {
        if ahead > 0 {
            line.push_str(&format!(
                " · {ahead}{}main",
                crate::glyph(crate::glyphs::Glyph::FlowQueued)
            ));
        }
    }
    if let Some(n) = row.pr_number {
        let ci = row.pr_ci.as_deref().unwrap_or("?");
        line.push_str(&format!(" · {}", format!("PR #{n} [CI:{ci}]").cyan()));
    }
    // Verdict marker: in flight / obsolete — <recovery command>.
    if row.obsolete {
        line.push_str(
            &format!(
                "  {} obsolete — `git worktree remove`",
                crate::glyph(crate::glyphs::Glyph::Warning)
            )
            .dimmed()
            .to_string(),
        );
    } else {
        line.push_str(
            &format!("  {} in flight", crate::glyph(crate::glyphs::Glyph::Check))
                .dimmed()
                .to_string(),
        );
    }
    line
}

/// BUG-609: assemble the collapsed one-line summary the default (non-`--all`)
/// worktree section prints once the roster crosses `WORKTREE_SUMMARY_THRESHOLD`.
/// Pure (no I/O) so the collapse copy is unit-testable: "Worktrees: M
/// (K obsolete — `aida session gc` to reap)". The reaping itself lives in
/// `aida session gc` / `aida doctor heal` (already built — BUG-614); status
/// only summarizes. trace:BUG-609 | ai:claude
pub(crate) fn worktree_summary_line(rows: &[WorktreeStatusRow]) -> String {
    let total = rows.len();
    let obsolete = rows.iter().filter(|r| r.obsolete).count();
    let mut s = format!("  Worktrees: {total}");
    if obsolete > 0 {
        s.push_str(&format!(
            " ({obsolete} obsolete — `aida session gc` to reap)"
        ));
    }
    s.push_str(" · `aida status --all` to list");
    s
}

/// STORY-456: `aida status` Worktrees section. Display-only — lists each
/// non-main, non-store worktree with its branch, tied spec, clean/dirty
/// state, the session lease covering it, live/dormant status, commits-ahead
/// of the default branch, its open PR (number + CI + mergeability), and an
/// obsolescence verdict (in flight / obsolete — `git worktree remove`).
/// Silent when only the main worktree exists. The open-PR fields come from a
/// single batched `gh pr list` snapshot — one gh call regardless of worktree
/// count. trace:STORY-456 | ai:claude
///
/// BUG-609: `show_all` (from `aida status --all`) forces the full list. By
/// default a roster larger than `WORKTREE_SUMMARY_THRESHOLD` collapses to a
/// one-line count + obsolete tally so abandoned fanned-out worktrees stop
/// drowning the surface. trace:BUG-609 | ai:claude
pub(crate) fn print_status_worktrees_section(project_root: &std::path::Path, show_all: bool) {
    let main_root = main_worktree_root_from(project_root);
    let rows = collect_worktree_status_rows(&main_root);
    if rows.is_empty() {
        return;
    }
    println!("{}", "─── Worktrees ───".bold());
    if !show_all && rows.len() > WORKTREE_SUMMARY_THRESHOLD {
        println!("{}", worktree_summary_line(&rows).dimmed());
        println!();
        return;
    }
    for row in &rows {
        println!("{}", format_worktree_status_line(row));
    }
    println!();
}

// BUG-609: fleet-state hygiene — the agent roster's headline counts only LIVE
// agents (dead-PID corpses partitioned out), and the worktree section collapses
// a large roster to a one-line summary by default. trace:BUG-609 | ai:claude
#[cfg(test)]
#[path = "tests/fleet_state_hygiene_tests.rs"]
mod fleet_state_hygiene_tests;

// STORY-673: terse default + opt-in detail for `aida status`. The default
// folds the long-tail rosters (open-PRs, recently-merged, remote activity,
// coordination, recent-activity feed, per-status requirement breakdown,
// AIDA-dev-context) behind one-line summaries; `--full` / `--all` expands them.
// These tests pin the PURE pieces of that contract: the requirement-breakdown
// summary copy and the open-PRs collapse threshold. trace:STORY-673 | ai:claude
#[cfg(test)]
#[path = "tests/story_673_terse_status_tests.rs"]
mod story_673_terse_status_tests;

/// STORY-456: I/O wrapper that gathers the inputs (worktrees, leases, live
/// sessions, the batched open-PR snapshot, per-worktree ahead-of-default
/// counts) and hands them to the pure `assemble_worktree_status_rows`. Shared
/// by the text section and the `--json` projection so they never drift.
/// trace:STORY-456 | ai:claude
pub(crate) fn collect_worktree_status_rows(main_root: &std::path::Path) -> Vec<WorktreeStatusRow> {
    let worktrees = list_worktrees(main_root);
    let leases = list_leases(main_root);
    let live = process_probe::probe_live_claude_sessions();
    // One batched gh call for every worktree's PR (never per-row).
    let pr_by_branch = collect_open_prs(main_root).by_branch;
    // Commits-ahead-of-default per worktree branch.
    let default_ref =
        detect_default_branch_ref(main_root).unwrap_or_else(|| "origin/main".to_string());
    // TASK-1056: one batched `git for-each-ref` ahead-count map for every local
    // branch, instead of a `git rev-list --count` per worktree branch. Falls
    // back to the per-branch probe for any branch the batch didn't cover (or
    // when the git version doesn't support the field — the map comes back
    // empty). trace:TASK-1056 | ai:claude
    let ahead_by_branch = collect_branch_ahead_of(main_root, &default_ref);
    let mut ahead_by_path = std::collections::HashMap::new();
    for wt in &worktrees {
        if wt.path == *main_root || wt.branch.as_deref() == Some("aida-store") {
            continue;
        }
        if let Some(branch) = wt.branch.as_deref() {
            let ahead = ahead_by_branch
                .get(branch)
                .copied()
                .or_else(|| branch_ahead_of(main_root, branch, &default_ref));
            if let Some(a) = ahead {
                ahead_by_path.insert(wt.path.clone(), a);
            }
        }
    }
    assemble_worktree_status_rows(
        &worktrees,
        main_root,
        &leases,
        &live,
        &pr_by_branch,
        &ahead_by_path,
    )
}

/// STORY-456: `aida status` Open PRs section. Lists every open PR (one
/// batched `gh pr list` call) with number + title, CI rollup, mergeability,
/// and a recommended next step per state. Silent when there are no open PRs
/// or gh is unavailable — display-only orientation, not a gate.
/// trace:STORY-456 | ai:claude
// STORY-673: above this many open PRs, the default `aida status` collapses the
// roster to a one-line count and shows only the first few — the full list is
// behind `--full` / `--all`. Keeps the actionable "is there a PR that needs me"
// signal at a glance without dumping a 10-PR wall. trace:STORY-673 | ai:claude
pub(crate) const OPEN_PRS_SUMMARY_THRESHOLD: usize = 3;

pub(crate) fn print_status_open_prs_section(project_root: &std::path::Path, show_full: bool) {
    let snapshot = collect_open_prs(project_root);
    if snapshot.by_branch.is_empty() {
        return;
    }
    let mut prs: Vec<&status_cleanup::OpenPrItem> = snapshot.by_branch.values().collect();
    prs.sort_by_key(|p| p.number);

    let total = prs.len();
    println!("{}", "─── Open PRs ───".bold());
    // STORY-673: terse default caps the roster at OPEN_PRS_SUMMARY_THRESHOLD;
    // `--full` lists every open PR. trace:STORY-673 | ai:claude
    let shown = if show_full {
        total
    } else {
        total.min(OPEN_PRS_SUMMARY_THRESHOLD)
    };
    for pr in prs.iter().take(shown) {
        let ci = pr.ci_rollup.as_deref().unwrap_or("?");
        let merge = pr.mergeable.as_deref().unwrap_or("UNKNOWN");
        let next = open_pr_next_step(
            ci,
            &merge.to_ascii_uppercase(),
            pr.review_decision.as_deref(),
        );
        let title = truncate_for_width(&pr.title, 56);
        println!(
            "  {} {} [CI:{} · {}]",
            format!("PR #{}", pr.number).cyan(),
            title,
            ci,
            merge.to_ascii_lowercase()
        );
        println!("    {} · {}", pr.head_branch.dimmed(), next.dimmed());
    }
    if !show_full && total > shown {
        // STORY-673: pointer to the full open-PR roster. trace:STORY-673
        println!(
            "  {}",
            format!("… {} more open — `aida status --full`", total - shown).dimmed()
        );
    }
    println!();
}

/// STORY-456: recommended next step for an open PR, keyed off CI rollup,
/// mergeability, and review decision. Distinguishes CI-failing from
/// merge-conflict from awaiting-review — the conflation the spec calls out
/// (`gh pr checks --watch` blurs CI with mergeability). Pure. trace:STORY-456
pub(crate) fn open_pr_next_step(
    ci: &str,
    mergeable: &str,
    review_decision: Option<&str>,
) -> String {
    if ci == "fail" {
        return "CI failing — fix & push".to_string();
    }
    if mergeable == "CONFLICTING" {
        return "merge conflict — rebase onto default".to_string();
    }
    if ci == "pending" {
        return "CI in progress — wait".to_string();
    }
    match review_decision {
        Some("CHANGES_REQUESTED") => "changes requested — address review".to_string(),
        Some("APPROVED") => "approved — ready to merge".to_string(),
        Some("REVIEW_REQUIRED") => "awaiting review".to_string(),
        _ if ci == "pass" => "checks green — ready to merge".to_string(),
        _ => "awaiting review".to_string(),
    }
}

/// Truncate `s` to at most `max` chars, appending an ellipsis when cut.
pub(crate) fn truncate_for_width(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// STORY-456: `aida status` Recently merged section — last N merged PRs (one
/// batched `gh pr list --state merged` call) for orientation. Silent when
/// there are none or gh is unavailable. trace:STORY-456 | ai:claude
pub(crate) fn print_status_recently_merged_section(
    project_root: &std::path::Path,
    limit: usize,
    show_full: bool,
) {
    let merged = collect_recently_merged_prs(project_root, limit);
    if merged.is_empty() {
        return;
    }
    println!("{}", "─── Recently merged ───".bold());
    // STORY-673: the recently-merged tail is pure backward-looking orientation
    // — collapse to a one-line "latest + count" by default; `--full` / `--all`
    // lists the tail. trace:STORY-673 | ai:claude
    if !show_full {
        let (number, title, when) = &merged[0];
        let title = truncate_for_width(title, 48);
        let when = when.as_deref().unwrap_or("");
        let more = if merged.len() > 1 {
            format!(" (+{} more — `aida status --full`)", merged.len() - 1)
        } else {
            String::new()
        };
        println!(
            "  latest: {} {} {}{}",
            format!("PR #{number}").green(),
            title,
            when.dimmed(),
            more.dimmed()
        );
        println!();
        return;
    }
    for (number, title, when) in &merged {
        let title = truncate_for_width(title, 56);
        let when = when.as_deref().unwrap_or("");
        println!(
            "  {} {} {}",
            format!("PR #{number}").green(),
            title,
            when.dimmed()
        );
    }
    println!();
}

/// STORY-456: parse `gh pr list --state merged --json number,title,mergedAt`
/// into `(number, title, humanized-merge-time)` rows. Pure — unit-tested
/// against captured gh JSON. trace:STORY-456 | ai:claude
pub(crate) fn parse_recently_merged_prs(stdout: &str) -> Vec<(u64, String, Option<String>)> {
    let json: serde_json::Value = match serde_json::from_str(stdout) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut rows = Vec::new();
    for pr in json.as_array().cloned().unwrap_or_default() {
        let Some(number) = pr.get("number").and_then(|v| v.as_u64()) else {
            continue;
        };
        let title = pr
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let when = pr
            .get("mergedAt")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| humanize_relative(dt.with_timezone(&chrono::Utc)));
        rows.push((number, title, when));
    }
    rows
}

/// STORY-456: batched fetch of the last `limit` merged PRs. Empty when gh is
/// missing or the call fails. trace:STORY-456 | ai:claude
pub(crate) fn collect_recently_merged_prs(
    project_root: &std::path::Path,
    limit: usize,
) -> Vec<(u64, String, Option<String>)> {
    // TASK-1055: process-lifetime memo, keyed by (canonical root, limit), so a
    // `--full` run can warm this gh probe concurrently with the others up front
    // and the recently-merged render below hits a warm cache. Merged-PR state
    // does not change within a single `status` run — same rationale as the
    // BUG-613 open-PR memo. trace:TASK-1055
    use std::sync::Mutex;
    use std::sync::OnceLock;
    #[allow(clippy::type_complexity)]
    static CACHE: OnceLock<
        Mutex<
            std::collections::HashMap<
                (std::path::PathBuf, usize),
                Vec<(u64, String, Option<String>)>,
            >,
        >,
    > = OnceLock::new();
    let key = (
        project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf()),
        limit,
    );
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(hit) = guard.get(&key) {
            return hit.clone();
        }
    }
    let rows = collect_recently_merged_prs_uncached(project_root, limit);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, rows.clone());
    }
    rows
}

pub(crate) fn collect_recently_merged_prs_uncached(
    project_root: &std::path::Path,
    limit: usize,
) -> Vec<(u64, String, Option<String>)> {
    let gh_bin = match resolve_gh_binary() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "list",
            "--state",
            "merged",
            "--limit",
            &limit.to_string(),
            "--json",
            "number,title,mergedAt",
        ])
        .output_retrying_etxtbsy();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    let mut rows = parse_recently_merged_prs(&String::from_utf8_lossy(&out.stdout));
    rows.truncate(limit);
    rows
}

/// STORY-452: read the tip commit of each remote-tracking branch under
/// `origin/*` into `RemoteCommit`s for the remote-activity inference. One
/// `git for-each-ref` call, no per-branch git invocations. Records use the
/// short branch name (`origin/bug-250` → `bug-250`). Empty when git is
/// unavailable or there are no remote branches. trace:STORY-452 | ai:claude
pub(crate) fn collect_remote_branch_commits(
    project_root: &std::path::Path,
) -> Vec<remote_activity::RemoteCommit> {
    // Tab-delimited: <short-name>\t<committerdate-iso8601-strict>\t<subject>.
    let out = std::process::Command::new("git")
        .current_dir(project_root)
        .args([
            "for-each-ref",
            "--format=%(refname:short)\t%(committerdate:iso8601-strict)\t%(subject)",
            "refs/remotes/origin",
        ])
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_remote_branch_commits(&stdout)
}

/// STORY-452: parse the tab-delimited `git for-each-ref` output into
/// `RemoteCommit`s. Pure — unit-tested without git. `origin/HEAD` (a symbolic
/// pointer, not a real branch) is skipped. trace:STORY-452 | ai:claude
pub(crate) fn parse_remote_branch_commits(stdout: &str) -> Vec<remote_activity::RemoteCommit> {
    let mut commits = Vec::new();
    for line in stdout.lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(refname) = parts.next() else {
            continue;
        };
        // `git for-each-ref ... refs/remotes/origin` yields `origin/<branch>`.
        let branch = refname.strip_prefix("origin/").unwrap_or(refname);
        if branch.is_empty() || branch == "HEAD" {
            continue;
        }
        let when = parts
            .next()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s.trim()).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));
        let subject = parts.next().unwrap_or("").to_string();
        if subject.trim().is_empty() {
            continue;
        }
        commits.push(remote_activity::RemoteCommit {
            branch: branch.to_string(),
            subject,
            when,
        });
    }
    commits
}

/// STORY-452: `aida status` "Recent remote activity" section. Infers cloud /
/// cross-machine agent work from `[AI:...]` commit trailers on remote branches
/// that have NO local session lease, since those agents never appear in the
/// local registry. Read-only, lossy-by-design, and silent when there is no
/// remote signal (or git is unavailable). trace:STORY-452 | ai:claude
pub(crate) fn print_status_remote_activity_section(
    project_root: &std::path::Path,
    limit: usize,
    show_full: bool,
) {
    let commits = collect_remote_branch_commits(project_root);
    if commits.is_empty() {
        return;
    }
    let lease_branches: Vec<String> = list_leases(project_root)
        .into_iter()
        .map(|l| l.branch)
        .filter(|b| !b.is_empty())
        .collect();
    let rows = remote_activity::infer_remote_activity(&commits, &lease_branches, limit);
    if rows.is_empty() {
        return;
    }
    println!("{}", "─── Recent remote activity (inferred) ───".bold());
    // STORY-673: inferred cross-machine activity is a lossy-by-design long-tail
    // — collapse to a one-line count by default; `--full` / `--all` expands the
    // per-branch feed. trace:STORY-673 | ai:claude
    if !show_full {
        let row = &rows[0];
        let spec = row.spec_id.as_deref().unwrap_or("");
        let more = if rows.len() > 1 {
            format!(" (+{} more — `aida status --full`)", rows.len() - 1)
        } else {
            String::new()
        };
        println!(
            "  {} on {} {}{}",
            row.agent_type.cyan(),
            spec.bold(),
            format!("(branch {})", row.branch).dimmed(),
            more.dimmed()
        );
        println!();
        return;
    }
    for row in &rows {
        let subject = truncate_for_width(&row.subject, 52);
        let when = row.when.map(humanize_relative).unwrap_or_default();
        let spec = row.spec_id.as_deref().unwrap_or("");
        println!(
            "  {:<11} {:<14} {} {}",
            row.agent_type.cyan(),
            spec.bold(),
            subject,
            when.dimmed()
        );
        println!("    {} {}", "branch:".dimmed(), row.branch.dimmed());
    }
    println!(
        "  {}",
        "(inferred from commit trailers on lease-less branches — local agents shown above)"
            .dimmed()
    );
    println!();
}

#[cfg(test)]
#[path = "tests/story_452_remote_activity_tests.rs"]
mod story_452_remote_activity_tests;

#[cfg(test)]
#[path = "tests/story_456_status_worktrees_tests.rs"]
mod story_456_status_worktrees_tests;

/// TASK-539: `aida status` Findings section. Display-only — surfaces the
/// pending-triage findings backlog (silent when empty) so it isn't invisible
/// until the user remembers `aida findings list`. Reuses the same
/// `build_findings_view` the findings command uses; shows a compact per-finding
/// line (source + id + origin + title), capped, with a pointer to the full
/// list. trace:TASK-539 | ai:claude
pub(crate) fn print_status_findings_section(backend: &aida_core::CachedGitBackend) {
    // Findings are DRAFT specs carrying a from-* tag (matches `aida findings
    // list`). Filtering to draft avoids counting completed/rejected specs that
    // still carry their origin from-review/from-implementer tag.
    let filter = aida_core::ListFilter {
        status: Some("draft".to_string()),
        ..Default::default()
    };
    let Ok(summaries) = backend.list_summaries(&filter) else {
        return;
    };
    let sections = crate::findings::build_findings_view(
        &summaries,
        &crate::findings::FindingsFilter::default(),
    );
    let total = crate::findings::count_findings(&sections);
    if total == 0 {
        return;
    }
    println!("{}", "─── Findings ───".bold());
    println!(
        "  {} pending finding(s) awaiting triage",
        total.to_string().yellow()
    );
    const CAP: usize = 6;
    let mut shown = 0;
    'outer: for section in &sections {
        for group in &section.groups {
            for row in &group.rows {
                println!(
                    "  {} {} {} ({}) — {}",
                    crate::glyph(crate::glyphs::Glyph::Bullet),
                    section.source.label().dimmed(),
                    row.display_id.yellow(),
                    group.origin,
                    row.title
                );
                shown += 1;
                if shown >= CAP {
                    break 'outer;
                }
            }
        }
    }
    if total > shown {
        println!(
            "  … {} more — `aida findings list` (promote/dismiss to triage)",
            total - shown
        );
    }
    println!();
}

/// STORY-457: `aida status` working-tree section. Display-only — parses
/// `git status --porcelain` in `root` and groups into staged / modified-tracked
/// / untracked, auto-flagging safe-to-remove untracked cruft with an `rm`
/// recommendation and surfacing the rest for a commit-or-remove decision.
/// Silent when the tree is clean. (The mtime recent-vs-stale split + the
/// content-matches-HEAD heuristic from the spec are a follow-up refinement.)
/// trace:STORY-457 | ai:claude
/// STORY-457 persistence: per-clone status-run state under `.aida/`, gitignored
/// by the deny-by-default `.aida/*` rule (no .gitignore change needed). Two
/// files drive the recent-vs-stale untracked heuristic and TASK-662's
/// delta-since-last-run:
///   - `last-status.toml`: timestamp of the previous `aida status` run.
///   - `untracked-history.toml`: first-observed timestamp per untracked path.
///     All I/O is best-effort: failures degrade to an empty / in-memory view
///     (everything reads as first-seen-now) and never break `aida status`.
///     trace:STORY-457 | ai:claude
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct LastStatusRecord {
    pub(crate) last_status_at: Option<String>,
}

pub(crate) fn status_last_path(root: &std::path::Path) -> std::path::PathBuf {
    root.join(".aida").join("last-status.toml")
}

pub(crate) fn status_untracked_history_path(root: &std::path::Path) -> std::path::PathBuf {
    root.join(".aida").join("untracked-history.toml")
}

/// Read the previous `aida status` run timestamp, if one was recorded.
/// Consumed by the recent-vs-stale heuristic and (later) TASK-662's delta.
/// trace:STORY-457 | ai:claude
// why: paired reader for write_last_status_at; wired into tests now, awaiting TASK-662's delta surface in production.
#[allow(dead_code)]
pub(crate) fn read_last_status_at(root: &std::path::Path) -> Option<chrono::DateTime<chrono::Utc>> {
    let txt = std::fs::read_to_string(status_last_path(root)).ok()?;
    let rec: LastStatusRecord = toml::from_str(&txt).ok()?;
    rec.last_status_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// Record `now` as the latest `aida status` run time (best-effort; no-op when
/// `.aida/` is absent, i.e. not an initialized project). trace:STORY-457
pub(crate) fn write_last_status_at(root: &std::path::Path, now: chrono::DateTime<chrono::Utc>) {
    if !root.join(".aida").is_dir() {
        return;
    }
    let rec = LastStatusRecord {
        last_status_at: Some(now.to_rfc3339()),
    };
    if let Ok(txt) = toml::to_string(&rec) {
        let _ = std::fs::write(status_last_path(root), txt);
    }
}

/// TASK-662: per-clone snapshot of the finding IDs seen on the previous
/// `aida status` run, persisted under `.aida/last-findings.toml` (gitignored by
/// the deny-by-default `.aida/*` rule — no .gitignore change needed). Drives the
/// `findings.delta` block in `aida status --json` (new-since-last-run). All I/O
/// is best-effort: a missing/unreadable file reads as "no prior run" so the
/// first delta is suppressed, and a write failure never breaks `aida status`.
/// trace:TASK-662 | ai:claude
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct LastFindingsRecord {
    /// Sorted-unique finding display IDs (e.g. `TASK-5`) seen last run.
    #[serde(default)]
    pub(crate) ids: Vec<String>,
}

pub(crate) fn status_last_findings_path(root: &std::path::Path) -> std::path::PathBuf {
    root.join(".aida").join("last-findings.toml")
}

/// Read the finding IDs recorded on the previous run. `None` distinguishes
/// "no prior run recorded" (suppress the delta entirely on a first run) from
/// "prior run had zero findings" (`Some(empty)`). trace:TASK-662 | ai:claude
pub(crate) fn read_last_findings(root: &std::path::Path) -> Option<Vec<String>> {
    let txt = std::fs::read_to_string(status_last_findings_path(root)).ok()?;
    let rec: LastFindingsRecord = toml::from_str(&txt).ok()?;
    Some(rec.ids)
}

/// Record the current finding IDs as the new baseline (best-effort; no-op when
/// `.aida/` is absent, i.e. not an initialized project). trace:TASK-662
pub(crate) fn write_last_findings(root: &std::path::Path, ids: &[String]) {
    if !root.join(".aida").is_dir() {
        return;
    }
    let mut ids = ids.to_vec();
    ids.sort();
    ids.dedup();
    let rec = LastFindingsRecord { ids };
    if let Ok(txt) = toml::to_string(&rec) {
        let _ = std::fs::write(status_last_findings_path(root), txt);
    }
}

/// The delta between the previous run's findings snapshot and the current set.
/// Serialized into `findings.delta` of `aida status --json`. trace:TASK-662
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct FindingsDelta {
    /// Total findings recorded on the previous run.
    pub(crate) previous_total: usize,
    /// Total findings now.
    pub(crate) current_total: usize,
    /// Findings present now but absent last run (newly filed since last run).
    pub(crate) new_count: usize,
    /// IDs of the new findings (sorted), so consumers can name them.
    pub(crate) new_ids: Vec<String>,
    /// Findings present last run but gone now (triaged / promoted / dismissed).
    pub(crate) resolved_count: usize,
}

/// Pure core of the delta-since-last-run: compare the persisted snapshot
/// (`previous`, `None` = no prior run) against the `current` finding IDs.
/// `None` ⇒ `None` (first run has no baseline to diff). Order-insensitive and
/// dedup-safe so a reordered or duplicated snapshot can't fabricate a delta.
/// trace:TASK-662 | ai:claude
pub(crate) fn compute_findings_delta(
    previous: Option<&[String]>,
    current: &[String],
) -> Option<FindingsDelta> {
    let previous = previous?;
    let prev_set: std::collections::HashSet<&str> = previous.iter().map(|s| s.as_str()).collect();
    let cur_set: std::collections::HashSet<&str> = current.iter().map(|s| s.as_str()).collect();
    let mut new_ids: Vec<String> = cur_set
        .iter()
        .filter(|id| !prev_set.contains(*id))
        .map(|id| id.to_string())
        .collect();
    new_ids.sort();
    let resolved_count = prev_set.iter().filter(|id| !cur_set.contains(*id)).count();
    Some(FindingsDelta {
        previous_total: prev_set.len(),
        current_total: cur_set.len(),
        new_count: new_ids.len(),
        new_ids,
        resolved_count,
    })
}

/// Load → upsert (first-observed = `now` for new paths) → prune (drop paths no
/// longer untracked) → persist the untracked-history map, returning the
/// first-observed timestamp for each currently-untracked path. Pure-data core
/// is `reconcile_untracked_map` (unit-tested); this wrapper does the I/O.
/// trace:STORY-457 | ai:claude
pub(crate) fn reconcile_untracked_history(
    root: &std::path::Path,
    current: &[String],
    now: chrono::DateTime<chrono::Utc>,
) -> std::collections::HashMap<String, chrono::DateTime<chrono::Utc>> {
    let path = status_untracked_history_path(root);
    let stored: std::collections::HashMap<String, String> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default();
    let merged = reconcile_untracked_map(stored, current, now);
    if root.join(".aida").is_dir() {
        if let Ok(txt) = toml::to_string(&merged) {
            let _ = std::fs::write(&path, txt);
        }
    }
    merged
        .into_iter()
        .filter_map(|(p, ts)| {
            chrono::DateTime::parse_from_rfc3339(&ts)
                .ok()
                .map(|dt| (p, dt.with_timezone(&chrono::Utc)))
        })
        .collect()
}

/// Pure core of `reconcile_untracked_history`: upsert new paths with
/// first-observed = `now`, prune entries no longer untracked. RFC3339 strings
/// in/out so it's trivially serializable + testable. trace:STORY-457 | ai:claude
pub(crate) fn reconcile_untracked_map(
    mut stored: std::collections::HashMap<String, String>,
    current: &[String],
    now: chrono::DateTime<chrono::Utc>,
) -> std::collections::HashMap<String, String> {
    let cur: std::collections::HashSet<&str> = current.iter().map(|s| s.as_str()).collect();
    stored.retain(|p, _| cur.contains(p.as_str()));
    for p in current {
        stored.entry(p.clone()).or_insert_with(|| now.to_rfc3339());
    }
    stored
}

/// STORY-457 recent-vs-stale classification of an untracked path given its
/// first-observed time: recent (<1h), stale (≥1d), or mid. trace:STORY-457
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UntrackedAge {
    Recent,
    Mid,
    Stale,
}

pub(crate) fn classify_untracked_age(
    first_seen: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> UntrackedAge {
    // Missing first-seen → just observed → recent.
    let seen = first_seen.unwrap_or(now);
    let age = now.signed_duration_since(seen);
    if age < chrono::Duration::hours(1) {
        UntrackedAge::Recent
    } else if age >= chrono::Duration::days(1) {
        UntrackedAge::Stale
    } else {
        UntrackedAge::Mid
    }
}

pub(crate) fn print_status_working_tree_section(root: &std::path::Path) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output();
    let Ok(out) = out else { return };
    if !out.status.success() {
        return;
    }
    let text = String::from_utf8_lossy(&out.stdout);

    let mut staged = 0usize;
    let mut modified: Vec<String> = Vec::new();
    let mut untracked_safe: Vec<String> = Vec::new();
    let mut untracked_other: Vec<String> = Vec::new();

    for line in text.lines() {
        if line.len() < 3 {
            continue;
        }
        let code = &line[..2];
        let path = line[3..].trim().to_string();
        let x = code.chars().next().unwrap_or(' '); // index (staged) column
        let y = code.chars().nth(1).unwrap_or(' '); // worktree column
        if code == "??" {
            if untracked_is_safe_to_remove(&path) {
                untracked_safe.push(path);
            } else {
                untracked_other.push(path);
            }
            continue;
        }
        // Staged: index column carries a change other than space/?.
        if x != ' ' && x != '?' {
            staged += 1;
        }
        // Modified/deleted in the worktree (unstaged).
        if y == 'M' || y == 'D' {
            modified.push(path);
        }
    }

    // STORY-457 persistence: record this run's timestamp + first-observation
    // history BEFORE the clean-tree early-return, so the recent-vs-stale clock
    // advances on every `aida status` (and TASK-662's delta has a baseline)
    // regardless of whether the tree is dirty. trace:STORY-457 | ai:claude
    let now = chrono::Utc::now();
    let all_untracked: Vec<String> = untracked_safe
        .iter()
        .chain(untracked_other.iter())
        .cloned()
        .collect();
    let first_seen = reconcile_untracked_history(root, &all_untracked, now);
    write_last_status_at(root, now);

    if staged == 0 && modified.is_empty() && untracked_safe.is_empty() && untracked_other.is_empty()
    {
        return;
    }

    let preview = |paths: &[String]| {
        paths
            .iter()
            .take(3)
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let preview_refs = |paths: &[&String]| {
        paths
            .iter()
            .take(3)
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };

    // STORY-457: split non-cruft untracked into recent (<1h) / mid / stale (≥1d)
    // by first-observed age so long-lingering files stand out from fresh ones.
    let mut untracked_recent: Vec<&String> = Vec::new();
    let mut untracked_mid: Vec<&String> = Vec::new();
    let mut untracked_stale: Vec<&String> = Vec::new();
    for p in &untracked_other {
        match classify_untracked_age(first_seen.get(p).copied(), now) {
            UntrackedAge::Recent => untracked_recent.push(p),
            UntrackedAge::Mid => untracked_mid.push(p),
            UntrackedAge::Stale => untracked_stale.push(p),
        }
    }

    println!("{}", "─── Working tree ───".bold());
    if staged > 0 {
        println!(
            "  {} {} staged file(s) — `git diff --cached` to review",
            "●".green(),
            staged
        );
    }
    if !modified.is_empty() {
        println!(
            "  {} {} modified — {}{}",
            "✎".yellow(),
            modified.len(),
            preview(&modified),
            if modified.len() > 3 { ", …" } else { "" }
        );
    }
    if !untracked_recent.is_empty() {
        println!(
            "  {} {} untracked — recent (<1h) — {}{}  (new)",
            "?".cyan(),
            untracked_recent.len(),
            preview_refs(&untracked_recent),
            if untracked_recent.len() > 3 {
                ", …"
            } else {
                ""
            }
        );
    }
    if !untracked_mid.is_empty() {
        println!(
            "  {} {} untracked — {}{}  (commit or remove)",
            "?".cyan(),
            untracked_mid.len(),
            preview_refs(&untracked_mid),
            if untracked_mid.len() > 3 { ", …" } else { "" }
        );
    }
    if !untracked_stale.is_empty() {
        println!(
            "  {} {} untracked — stale (≥1d) — {}{}  (commit or remove)",
            "?".yellow(),
            untracked_stale.len(),
            preview_refs(&untracked_stale),
            if untracked_stale.len() > 3 {
                ", …"
            } else {
                ""
            }
        );
    }
    if !untracked_safe.is_empty() {
        println!(
            "  🧹 {} removable cruft — {}{}  (safe: `rm`)",
            untracked_safe.len(),
            preview(&untracked_safe),
            if untracked_safe.len() > 3 {
                ", …"
            } else {
                ""
            }
        );
    }
    println!();
}

/// Which of `pids` are running right now.
///
/// A ZOMBIE does not count. A terminated child stays in the process table until its parent
/// reaps it — so treating "present" as "alive" would make `agent stop` report that it had
/// failed to kill something it had just killed. The canonical probe already excludes zombies,
/// and it is O(1) per pid instead of a full process-table walk.
// trace:BUG-1703 | ai:claude
// trace:TASK-1750 | ai:claude
pub(crate) fn live_pids(pids: &[u32]) -> Vec<u32> {
    pids.iter()
        .copied()
        .filter(|&pid| aida_core::liveness::pid_is_alive(pid))
        .collect()
}

/// BUG-61: SIGTERM each pid, sleep `grace_secs`, then SIGKILL any that
/// are still alive. trace:BUG-61 | ai:claude
/// SIGTERM, wait, SIGKILL the survivors, then report who is STILL alive.
///
/// BUG-1703: this used to return `()`, and `agent stop` printed success regardless. Callers must
/// be able to tell a real stop from a signal that landed on nothing.
// trace:BUG-1703 | ai:claude
pub(crate) fn terminate_pids_with_grace(pids: &[u32], grace_secs: u64) -> Vec<u32> {
    use sysinfo::{ProcessRefreshKind, RefreshKind, Signal, System};
    let mut sys =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    sys.refresh_processes_specifics(ProcessRefreshKind::new());
    for &pid in pids {
        if let Some(p) = sys.process(sysinfo::Pid::from_u32(pid)) {
            let _ = p.kill_with(Signal::Term);
        }
    }
    std::thread::sleep(std::time::Duration::from_secs(grace_secs));
    sys.refresh_processes_specifics(ProcessRefreshKind::new());
    for &pid in pids {
        if let Some(p) = sys.process(sysinfo::Pid::from_u32(pid)) {
            eprintln!(
                "  pid {} still alive after SIGTERM — sending SIGKILL",
                pid.to_string().yellow()
            );
            let _ = p.kill_with(Signal::Kill);
        }
    }
    // SIGKILL is not instantaneous; give the kernel a moment before judging.
    std::thread::sleep(std::time::Duration::from_millis(500));
    live_pids(pids)
}

/// TASK-75: a status transition extracted from a commit's YAML diff,
/// annotated with where it came from. trace:TASK-75 | ai:claude
pub(crate) struct StatusTransition {
    pub(crate) spec_id: String,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) source: String,
}

/// TASK-75: classify a commit subject as a "PR-N merge" (squash via
/// `(#N)` suffix), an explicit commit referencing a REQ-ID, or "manual".
/// trace:TASK-75 | ai:claude
pub(crate) fn classify_commit_source(subject: &str) -> String {
    let trimmed = subject.trim();
    // PR-N merge: trailing `(#N)`.
    if let Some(open) = trimmed.rfind('(') {
        let inner = &trimmed[open + 1..];
        if let Some(close) = inner.find(')') {
            let body = &inner[..close];
            if let Some(n) = body.strip_prefix('#') {
                if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
                    return format!("PR-{} merge", n);
                }
            }
        }
    }
    // commit-with-REQ-ID: subject extractor finds at least one SPEC-ID
    // in the trailing parens.
    let ids = extract_spec_ids_from_commit(subject);
    if !ids.is_empty() {
        return format!("commit ({})", ids.join(", "));
    }
    "manual".to_string()
}

/// TASK-75: parse each commit's diff to find status transitions in the
/// modified YAML files. Returns one StatusTransition per (commit, spec)
/// pair that actually changed status. trace:TASK-75 | ai:claude
pub(crate) fn extract_status_changes_from_commits(
    store_path: &std::path::Path,
    commits: &[(String, String, String)],
    modified_specs_per_commit: &std::collections::BTreeMap<String, Vec<String>>,
) -> Vec<StatusTransition> {
    let mut out = Vec::new();
    for (sha, _subj, source) in commits {
        let Some(specs) = modified_specs_per_commit.get(sha) else {
            continue;
        };
        if specs.is_empty() {
            continue;
        }
        let diff = match aida_core::git_ops::host_git_command(store_path)
            .args(["show", "--no-color", "--unified=0", sha, "--"])
            .args(specs.iter().map(|s| format!("objects/*/{}.yaml", s)))
            .output()
        {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => continue,
        };
        for tr in parse_status_transitions_from_diff(&diff) {
            out.push(StatusTransition {
                spec_id: tr.0,
                from: tr.1,
                to: tr.2,
                source: source.clone(),
            });
        }
    }
    out
}

/// TASK-75: scan unified-diff output for status transitions. Each YAML
/// status change appears as a `-status: X` / `+status: Y` pair within a
/// single file's hunk. We walk file-by-file: track which spec the hunk
/// belongs to (via `diff --git a/objects/.../X.yaml`), then pair the
/// `-status` and `+status` lines. trace:TASK-75 | ai:claude
pub(crate) fn parse_status_transitions_from_diff(diff: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut current_spec: Option<String> = None;
    let mut prev_status: Option<String> = None;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            // rest: `objects/TYPE/000/SPEC-ID.yaml b/objects/...`
            current_spec = rest
                .split_whitespace()
                .next()
                .and_then(|p| spec_id_from_orphan_path(p).into())
                .filter(|s| s != "?");
            prev_status = None;
            continue;
        }
        let Some(spec) = current_spec.as_deref() else {
            continue;
        };
        if let Some(rest) = line.strip_prefix("-status:") {
            prev_status = Some(rest.trim().trim_matches('"').to_string());
        } else if let Some(rest) = line.strip_prefix("+status:") {
            let to = rest.trim().trim_matches('"').to_string();
            if let Some(from) = prev_status.take() {
                if from != to {
                    out.push((spec.to_string(), from, to));
                }
            }
        }
    }
    out
}

pub(crate) fn spec_id_from_orphan_path(path: &str) -> String {
    // objects/TYPE/000/SPEC-ID.yaml
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string()
}

pub(crate) fn req_type_from_orphan_path(path: &str) -> String {
    path.split('/').nth(1).unwrap_or("?").to_string()
}

#[cfg(test)]
#[path = "tests/pull_summary_status_change_tests.rs"]
mod pull_summary_status_change_tests;

#[cfg(test)]
#[path = "tests/queue_work_tests.rs"]
mod queue_work_tests;

#[cfg(test)]
#[path = "tests/bug_1607_reviewer_vendor_tests.rs"]
mod bug_1607_reviewer_vendor_tests;

// trace:BUG-1686 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1686_agy_headless_argv_tests.rs"]
mod bug_1686_agy_headless_argv_tests;

// trace:BUG-2133 | ai:claude
#[cfg(test)]
#[path = "tests/bug_2133_contained_drain_children_tests.rs"]
mod bug_2133_contained_drain_children_tests;

#[cfg(test)]
#[path = "tests/bug_1609_gitlab_reviewer_preflight_tests.rs"]
mod bug_1609_gitlab_reviewer_preflight_tests;

#[cfg(test)]
#[path = "tests/queue_rework_tests.rs"]
mod queue_rework_tests;

// trace:BUG-1632 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1632_status_writer_tests.rs"]
mod bug_1632_status_writer_tests;

// BUG-1637: every status writer records its transition through the one shared
// helper with the right author class. trace:BUG-1637 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1637_status_writer_tests.rs"]
mod bug_1637_status_writer_tests;

// BUG-1647: atomic findings dismiss and the findings promote edge cases.
// trace:BUG-1647 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1647_findings_atomic_tests.rs"]
mod bug_1647_findings_atomic_tests;

// BUG-1651: compare-and-swap queue rollback on a failed findings promote.
// trace:BUG-1651 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1651_promote_queue_cas_tests.rs"]
mod bug_1651_promote_queue_cas_tests;

// trace:BUG-1664 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1664_stale_sweep_tests.rs"]
mod bug_1664_stale_sweep_tests;

// BUG-1671: the sweeps re-check each candidate INSIDE the store write lock, so
// a reopen landing between the re-check and the write is not overwritten.
// trace:BUG-1671 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1671_sweep_lock_window_tests.rs"]
mod bug_1671_sweep_lock_window_tests;

// BUG-1672: `aida human` reviews-awaiting must not list already-approved
// specs. trace:BUG-1672 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1672_reviews_awaiting_approved_tests.rs"]
mod bug_1672_reviews_awaiting_approved_tests;

// BUG-1633: pull/push follow-ups to BUG-1625 and BUG-1626. trace:BUG-1633 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1633_pull_push_followups_tests.rs"]
mod bug_1633_pull_push_followups_tests;

#[cfg(test)]
#[path = "tests/eval_subcommand_hint_tests.rs"]
mod eval_subcommand_hint_tests;

#[cfg(test)]
#[path = "tests/worktree_enter_wrapper_staleness_tests.rs"]
mod worktree_enter_wrapper_staleness_tests;

#[cfg(test)]
#[path = "tests/epic_agent_new_refusal_tests.rs"]
mod epic_agent_new_refusal_tests;

#[cfg(test)]
#[path = "tests/binary_selection_tests.rs"]
mod binary_selection_tests;

#[cfg(test)]
#[path = "tests/ci_action_tests.rs"]
mod ci_action_tests;

/// Distributed-mode status: read from CachedGitBackend so we get cache-backed
/// counts, sync state, and recent activity.
/// TASK-220: gathered facts for the unified `aida status` view. Each
/// optional field's `None` means "section absent" (no session covering
/// cwd, no PR open, gh missing) — call sites graceful-degrade per
/// section without bailing on the whole command. trace:TASK-220
#[derive(Debug)]
pub(crate) struct UserStatusContext {
    pub(crate) session: Option<SessionLease>,
    pub(crate) role: Option<String>,
    pub(crate) branch: Option<BranchFacts>,
    pub(crate) pr: Option<PrFacts>,
    pub(crate) queue_head: Vec<QueueRow>,
    pub(crate) queue_total: usize,
    pub(crate) agents: Vec<agent_registry::AgentRegistryView>,
}

#[derive(Debug, Clone)]
pub(crate) struct BranchFacts {
    pub(crate) name: String,
    pub(crate) dirty: bool,
    pub(crate) ahead_main: Option<u32>,
    pub(crate) behind_main: Option<u32>,
    pub(crate) ahead_upstream: Option<u32>,
    pub(crate) behind_upstream: Option<u32>,
    pub(crate) has_upstream: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct PrFacts {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) state: String,
    pub(crate) ci_rollup: Option<String>,
    pub(crate) gh_status: GhStatus,
}

#[derive(Debug, Clone)]
pub(crate) enum GhStatus {
    Ok,
    Missing,
    Failed(String),
    Skipped,
    /// BUG-560: the remote isn't GitHub, so `gh` is the wrong tool — we never
    /// spawned it. Carries the detected forge so the section can name the right
    /// CLI (`glab`) / degrade cleanly instead of leaking gh's "known GitHub
    /// host" auth error to a GitLab/pure-git user. trace:BUG-560 | ai:claude
    NotGitHub(forge::ForgeKind),
}

#[derive(Debug, Clone)]
pub(crate) struct QueueRow {
    pub(crate) spec_id: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) for_role: Option<String>,
    pub(crate) in_progress: bool,
    pub(crate) lease_id: Option<String>,
    pub(crate) lease_started_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub(crate) fn collect_user_context(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
    backend: &aida_core::CachedGitBackend,
    no_ci: bool,
) -> UserStatusContext {
    let leases = list_leases(project_root);
    let session = std::env::current_dir().ok().and_then(|cwd| {
        let canon = cwd.canonicalize().unwrap_or(cwd);
        leases
            .iter()
            .find(|&l| lease_covers_cwd(l, &canon))
            .cloned()
    });
    let role = std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| session.as_ref().and_then(|l| l.role.clone()));

    let branch = collect_branch_facts(project_root);

    let pr = if no_ci {
        Some(PrFacts {
            number: 0,
            title: String::new(),
            url: String::new(),
            state: String::new(),
            ci_rollup: None,
            gh_status: GhStatus::Skipped,
        })
    } else {
        branch
            .as_ref()
            .map(|b| collect_pr_facts(project_root, &b.name))
    };

    let (queue_head, queue_total) =
        collect_queue_snapshot(backend, store, role.as_deref(), &leases);
    let agent_ctx = build_agent_classify_context(project_root, &leases);
    let registry_agents = agent_registry::list_agent_views(project_root, &agent_ctx);
    let agents =
        merge_agent_views_with_lease_fallback(project_root, &leases, registry_agents, &agent_ctx);

    UserStatusContext {
        session,
        role,
        branch,
        pr,
        queue_head,
        queue_total,
        agents,
    }
}

/// STORY-435: assemble the `AgentClassifyContext` `list_agent_views` needs
/// to compute busy/idle. The live-lease snapshot reuses the same
/// `probe_live_claude_sessions` that `aida session leases` shows under the
/// ● live glyph so the two views agree about which scopes are "being
/// worked." trace:STORY-435 | ai:claude
pub(crate) fn build_agent_classify_context(
    project_root: &std::path::Path,
    leases: &[SessionLease],
) -> agent_registry::AgentClassifyContext {
    let now = chrono::Utc::now();
    let cfg = agent_registry::Config::load(project_root);
    // BUG-613/TASK-1061: ONE memoized live-session probe drives liveness for
    // every lease — the mapping below is a pure fold over this single snapshot,
    // never an O(leases) fan-out of separate `/proc` probes. trace:TASK-1061
    let live_sessions = process_probe::probe_live_claude_sessions();
    let live_lease_worktrees = live_lease_worktrees(now, leases, &live_sessions);
    agent_registry::AgentClassifyContext::new(now, cfg.busy_threshold_secs, live_lease_worktrees)
        .with_work_probe(
            aida_core::liveness::probe_process_tree(),
            cfg.work_grace_secs,
        )
}

/// TASK-1061: compute the set of live-lease worktree paths from a SINGLE shared
/// `live_sessions` snapshot. Pulled out of `build_agent_classify_context` as a
/// pure fold over the already-probed slice so the live-session `/proc` probe
/// stays bounded to the one memoized walk (BUG-613) regardless of how many
/// leases are open — adding leases never adds probes. Taking `live_sessions` by
/// reference is the structural guarantee: this function CANNOT re-probe per
/// lease.
// trace:TASK-1061
pub(crate) fn live_lease_worktrees(
    now: chrono::DateTime<chrono::Utc>,
    leases: &[SessionLease],
    live_sessions: &[process_probe::LiveSession],
) -> Vec<std::path::PathBuf> {
    leases
        .iter()
        .filter_map(|l| {
            if l.worktree_path.as_os_str().is_empty() {
                return None;
            }
            let worktree_exists = l.worktree_path.exists();
            let live_in_worktree = live_sessions.iter().any(|s| {
                !s.stale_cwd && (s.cwd == l.worktree_path || s.cwd.starts_with(&l.worktree_path))
            });
            let age_hours = now.signed_duration_since(l.started_at).num_hours();
            match classify_lease_state(worktree_exists, live_in_worktree, age_hours) {
                LeaseState::Live => Some(l.worktree_path.clone()),
                _ => None,
            }
        })
        .collect()
}

// TASK-1061: the live-session `/proc` probe must stay bounded — one memoized
// walk drives liveness for EVERY lease, never an O(leases) fan-out of separate
// probes. `live_lease_worktrees` enforces this structurally by taking the
// already-probed `live_sessions` slice; these tests lock that contract: N
// leases are classified entirely from a SINGLE injected snapshot, with zero
// probes performed by the function itself. trace:TASK-1061
#[cfg(test)]
#[path = "tests/live_lease_worktrees_tests.rs"]
mod live_lease_worktrees_tests;

/// TASK-515: launcher/MCP registry rows are richer and win. Raw agent launches
/// still create session leases, though, so synthesize lease-backed rows for any
/// active lease not already represented by registry metadata.
pub(crate) fn merge_agent_views_with_lease_fallback(
    project_root: &std::path::Path,
    leases: &[SessionLease],
    mut registry_agents: Vec<agent_registry::AgentRegistryView>,
    ctx: &agent_registry::AgentClassifyContext,
) -> Vec<agent_registry::AgentRegistryView> {
    let live_sessions = process_probe::probe_live_claude_sessions();
    for lease in leases {
        if registry_agents
            .iter()
            .any(|agent| agent_view_covers_lease(agent, lease))
        {
            continue;
        }
        registry_agents.push(lease_agent_view(project_root, lease, ctx, &live_sessions));
    }
    registry_agents.sort_by(|a, b| {
        a.agent_type
            .cmp(&b.agent_type)
            .then_with(|| a.current_spec.cmp(&b.current_spec))
            .then_with(|| a.id.cmp(&b.id))
    });
    registry_agents
}

pub(crate) fn agent_view_covers_lease(
    agent: &agent_registry::AgentRegistryView,
    lease: &SessionLease,
) -> bool {
    let same_scope = agent
        .current_spec
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case(&lease.scope))
        .unwrap_or(false);
    let same_worktree = !lease.worktree_path.as_os_str().is_empty()
        && (agent.worktree_path == lease.worktree_path
            || agent.worktree_path.starts_with(&lease.worktree_path)
            || lease.worktree_path.starts_with(&agent.worktree_path));
    same_scope || same_worktree
}

pub(crate) fn lease_agent_view(
    project_root: &std::path::Path,
    lease: &SessionLease,
    ctx: &agent_registry::AgentClassifyContext,
    live_sessions: &[process_probe::LiveSession],
) -> agent_registry::AgentRegistryView {
    // BUG-511: review-verb leases classify by creator PID, not worktree.
    let lease_state = lease_state_for(lease, live_sessions, ctx.now);
    let status = match lease_state {
        LeaseState::Stale => agent_registry::AgentStatus::Stale,
        LeaseState::Live | LeaseState::Dormant => agent_registry::AgentStatus::Busy,
    };

    agent_registry::AgentRegistryView {
        id: format!("lease-{}", lease.id),
        agent_type: lease_agent_type(lease),
        pid: lease.creator_pid.unwrap_or(0),
        name: None,
        description: None,
        tty: None,
        terminal: None,
        started_at: lease.started_at,
        last_active_at: lease_activity_timestamp(project_root, lease).unwrap_or(lease.started_at),
        role: lease.role.clone(),
        current_spec: Some(lease.scope.clone()),
        worktree_path: lease.worktree_path.clone(),
        source: "lease".to_string(),
        binary_version: None,
        build_sha: None,
        status,
        // STORY-528: lease-derived views have no availability state of their
        // own — only registry-backed agents can be paused.
        availability: agent_registry::Availability::Available,
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id: None,
        ended_at: None,
        resumed_from: None,
    }
}

pub(crate) fn lease_agent_type(lease: &SessionLease) -> String {
    let env_path = lease.worktree_path.join(".aida").join("session-env.sh");
    std::fs::read_to_string(&env_path)
        .ok()
        .and_then(|body| {
            parse_session_env(&body)
                .into_iter()
                .find(|(name, _)| name == "AIDA_AGENT_TYPE")
                .map(|(_, value)| value)
        })
        .map(agent_registry::normalize_agent_type)
        .unwrap_or_else(|| "unknown".to_string())
}

pub(crate) fn lease_activity_timestamp(
    project_root: &std::path::Path,
    lease: &SessionLease,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let path = session_activity_path(project_root, &lease.id);
    let body = std::fs::read_to_string(path).ok()?;
    let record = toml::from_str::<SessionActivityLog>(&body).ok()?;
    record.entries.into_iter().map(|entry| entry.at).max()
}

pub(crate) fn collect_branch_facts(project_root: &std::path::Path) -> Option<BranchFacts> {
    let name = current_branch_at(project_root)?;
    let dirty = !working_tree_clean(project_root).unwrap_or(true);
    let (ahead_main, behind_main) = ahead_behind_vs_ref(project_root, &name, "origin/main")
        .or_else(|| ahead_behind_vs_ref(project_root, &name, "main"))
        .map(|(a, b)| (Some(a), Some(b)))
        .unwrap_or((None, None));
    let upstream = upstream_ref_for(project_root, &name);
    let (ahead_upstream, behind_upstream, has_upstream) = match upstream {
        Some(u) => {
            let (a, b) = ahead_behind_vs_ref(project_root, &name, &u).unwrap_or((0, 0));
            (Some(a), Some(b), true)
        }
        None => (None, None, false),
    };
    Some(BranchFacts {
        name,
        dirty,
        ahead_main,
        behind_main,
        ahead_upstream,
        behind_upstream,
        has_upstream,
    })
}

pub(crate) fn working_tree_clean(project_root: &std::path::Path) -> Option<bool> {
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    Some(o.stdout.iter().all(|b| b.is_ascii_whitespace()))
}

pub(crate) fn ahead_behind_vs_ref(
    project_root: &std::path::Path,
    branch: &str,
    target: &str,
) -> Option<(u32, u32)> {
    let exists = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--verify", "--quiet", target])
        .output()
        .ok()?
        .status
        .success();
    if !exists {
        return None;
    }
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-list",
            "--left-right",
            "--count",
            // trace:BUG-1622 | ai:claude
            git_arg_guard::END_OF_OPTIONS,
            &format!("{}...{}", branch, target),
        ])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&o.stdout);
    let mut parts = s.split_whitespace();
    let a: u32 = parts.next()?.parse().ok()?;
    let b: u32 = parts.next()?.parse().ok()?;
    Some((a, b))
}

pub(crate) fn upstream_ref_for(project_root: &std::path::Path, _branch: &str) -> Option<String> {
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Process-lifetime memo over [`collect_pr_facts_uncached`]. The rich
/// `aida status` view resolves the current-branch PR/CI facts from more than
/// one section (the user-context gather + the awaiting-you report), and a
/// `--full` run warms this concurrently with the other gh probes up front
/// (TASK-1055). PR state does not change within a single short-lived `status`
/// run, so memoize by (canonical root, branch) — same justification as the
/// BUG-613 open-PR memo.
// trace:TASK-1055
pub(crate) fn collect_pr_facts(project_root: &std::path::Path, branch: &str) -> PrFacts {
    use std::sync::Mutex;
    use std::sync::OnceLock;
    static CACHE: OnceLock<
        Mutex<std::collections::HashMap<(std::path::PathBuf, String), PrFacts>>,
    > = OnceLock::new();
    let key = (
        project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf()),
        branch.to_string(),
    );
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(hit) = guard.get(&key) {
            return hit.clone();
        }
    }
    let facts = collect_pr_facts_uncached(project_root, branch);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, facts.clone());
    }
    facts
}

pub(crate) fn collect_pr_facts_uncached(project_root: &std::path::Path, branch: &str) -> PrFacts {
    // external-prose-classifier: status_context::collect_pr_facts_uncached
    // BUG-560: `gh` is GitHub-only. On a GitLab / pure-git remote it fails with
    // a raw "none of the git remotes ... point to a known GitHub host" auth
    // error that we used to surface verbatim — telling a corporate GitLab
    // first-user to `gh auth login` a CLI they don't use. Detect the forge and
    // skip the spawn entirely for anything but GitHub; the section then renders
    // a clean forge-aware line. trace:BUG-560 | ai:claude
    let forge_kind = forge::resolve_forge_kind(project_root);
    if forge_kind != forge::ForgeKind::GitHub {
        return PrFacts {
            number: 0,
            title: String::new(),
            url: String::new(),
            state: String::new(),
            ci_rollup: None,
            gh_status: GhStatus::NotGitHub(forge_kind),
        };
    }
    let gh_bin = match resolve_gh_binary() {
        Some(p) => p,
        None => {
            return PrFacts {
                number: 0,
                title: String::new(),
                url: String::new(),
                state: String::new(),
                ci_rollup: None,
                gh_status: GhStatus::Missing,
            };
        }
    };
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "view",
            branch,
            "--json",
            "number,title,url,state,statusCheckRollup",
            "-q",
            r#"[(.number|tostring), .title, .url, .state, ([.statusCheckRollup[]?.conclusion] | unique | join(","))] | @tsv"#,
        ])
        .output_retrying_etxtbsy();
    let out = match out {
        Ok(o) => o,
        Err(e) => {
            return PrFacts {
                number: 0,
                title: String::new(),
                url: String::new(),
                state: String::new(),
                ci_rollup: None,
                gh_status: GhStatus::Failed(format!("{}", e)),
            };
        }
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // "no pull requests found" is the common no-PR-for-this-branch
        // path; treat it as a clean "no PR" rather than a failure.
        if aida_core::external_tool_output::contains_any_case_insensitive(
            &stderr,
            aida_core::external_tool_output::GH_NO_PULL_REQUEST,
        ) {
            return PrFacts {
                number: 0,
                title: String::new(),
                url: String::new(),
                state: "none".to_string(),
                ci_rollup: None,
                gh_status: GhStatus::Ok,
            };
        }
        return PrFacts {
            number: 0,
            title: String::new(),
            url: String::new(),
            state: String::new(),
            ci_rollup: None,
            gh_status: GhStatus::Failed(stderr),
        };
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let line = raw.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return PrFacts {
            number: 0,
            title: String::new(),
            url: String::new(),
            state: "none".to_string(),
            ci_rollup: None,
            gh_status: GhStatus::Ok,
        };
    }
    let mut fields = line.split('\t');
    let number: u64 = fields.next().unwrap_or("0").parse().unwrap_or(0);
    let title = fields.next().unwrap_or("").to_string();
    let url = fields.next().unwrap_or("").to_string();
    let state = fields.next().unwrap_or("").to_string();
    let rollup = fields.next().unwrap_or("").to_string();
    let ci_rollup = if rollup.is_empty() {
        None
    } else {
        Some(rollup)
    };
    PrFacts {
        number,
        title,
        url,
        state,
        ci_rollup,
        gh_status: GhStatus::Ok,
    }
}

pub(crate) fn collect_queue_snapshot(
    backend: &aida_core::CachedGitBackend,
    store: &aida_core::models::RequirementsStore,
    role: Option<&str>,
    leases: &[SessionLease],
) -> (Vec<QueueRow>, usize) {
    use aida_core::DatabaseBackend;
    let user_id = current_user_id(None);
    // Include the work peers routed to this role — it lives in THEIR queue
    // file, so an own-file-only read made the snapshot disagree with what is
    // actually routed to us. trace:BUG-774 | ai:claude
    let entries = match role {
        Some(r) => {
            queue_role_fallback::queue_list_with_role_fallback(backend, &user_id, Some(r), false)
                .unwrap_or_default()
        }
        None => backend.queue_list(&user_id, false).unwrap_or_default(),
    };

    // TASK-490: an at-keyboard operator's first question is "what is being
    // worked on right now?" — surface in-progress queue items unconditionally,
    // then fill the next-up display budget with the FIFO head of the rest.
    // The "X more" counter in the renderer is computed against next-up only
    // so the two sections sum to the visible totals.
    // trace:TASK-490 | ai:claude
    const NEXT_UP_BUDGET: usize = 5;
    let mut in_progress_rows: Vec<QueueRow> = Vec::new();
    let mut next_up_rows: Vec<QueueRow> = Vec::new();
    let mut total: usize = 0;
    for entry in entries {
        if let Some(r) = role {
            let mismatch = entry
                .for_role
                .as_deref()
                .map(|er: &str| !er.eq_ignore_ascii_case(r))
                .unwrap_or(false);
            if mismatch {
                continue;
            }
        }
        let Some(req) = store
            .requirements
            .iter()
            .find(|rq| rq.id == entry.requirement_id)
        else {
            continue;
        };
        if matches!(
            req.status,
            aida_core::RequirementStatus::Completed | aida_core::RequirementStatus::Rejected
        ) {
            continue;
        }
        total += 1;
        let is_in_progress = matches!(req.status, aida_core::RequirementStatus::InProgress);
        let spec_id = req.display_id();
        let (lease_id, lease_started_at) = if is_in_progress {
            leases
                .iter()
                .find(|l| l.scope.eq_ignore_ascii_case(&spec_id))
                .map(|l| (Some(l.id.clone()), Some(l.started_at)))
                .unwrap_or((None, None))
        } else {
            (None, None)
        };
        let row = QueueRow {
            spec_id,
            title: req.title.clone(),
            status: format!("{}", req.status),
            for_role: entry.for_role.clone(),
            in_progress: is_in_progress,
            lease_id,
            lease_started_at,
        };
        if is_in_progress {
            in_progress_rows.push(row);
        } else if next_up_rows.len() < NEXT_UP_BUDGET {
            next_up_rows.push(row);
        }
    }
    // In-progress first (all of them — typically few), then the FIFO head of
    // the remaining queue up to the display budget.
    in_progress_rows.extend(next_up_rows);
    (in_progress_rows, total)
}

/// TASK-756: a small additive presence line in `aida status`. Read-only —
/// surfaces the effective operator presence (home/away) without changing any
/// other section. Only the `away` state is loud enough to print; `home` is the
/// boring default and stays quiet so the line appearing IS the signal.
/// trace:TASK-756 | ai:claude
pub(crate) fn print_status_presence_line(project_root: &std::path::Path) {
    let now = chrono::Utc::now();
    if !matches!(presence::current_presence(now), presence::Presence::Away) {
        return;
    }
    let Some(file) = presence::read_presence_file() else {
        return;
    };
    let set_at = chrono::DateTime::parse_from_rfc3339(&file.set_at)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or(now);
    let _ = project_root; // reserved: per-project presence would key off this
    println!(
        "  {} {} (set {}, {})",
        crate::glyph(crate::glyphs::Glyph::Away).to_string(),
        "away".yellow().bold(),
        presence::since_label(set_at, now),
        presence::ttl_remaining_label(set_at, file.ttl_secs, now)
    );
    println!();
}

/// The keystone parking tag — work that is clear to build but reserved for the
/// at-keyboard `--zen` lane (the supervised-build cohort). Mirrors the
/// `needs-supervised-build` parking tag the burndown gate already excludes from
/// the autonomous ready set. trace:STORY-561 | ai:claude
pub(crate) const KEYSTONE_TAG: &str = "needs-supervised-build";

/// STORY-561 consumers (c) + (d) home-side: presence-gated surfacing in
/// `aida status`. When the operator is HOME and `[presence] consumers = on`,
/// surface (c) the decision inbox depth (`aida questions`) and — when
/// `home_offer = surface` — (d) the keystone set as "ready for `--zen`"
/// (presence is useful in BOTH directions). When AWAY these accumulate quietly
/// (the point of `away` is to NOT nag; the away line above already prints).
/// Display-only and advisory — changes no execution path. The away-side safety
/// floor (keystone NOT offered for autonomous pickup) is already structural:
/// `needs-supervised-build` is a parking tag, so the drain gate never picks it
/// up regardless of presence. trace:STORY-561 | ai:claude
pub(crate) fn print_status_presence_consumers(
    project_root: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
) {
    let cfg = presence::read_presence_config(&config_path_for_project(project_root));
    if cfg.consumers == presence::ConsumersMode::Off {
        return;
    }
    // Quiet accumulation when away — the away line is the only presence noise.
    if matches!(
        presence::current_presence(chrono::Utc::now()),
        presence::Presence::Away
    ) {
        return;
    }

    // (c) decision inbox — pending DecisionRequests awaiting the operator.
    // TASK-1065: read the count from the `has_pending_decision` cache column so
    // the rich `aida status` path no longer needs a full `backend.load()` to
    // surface it. Matches the prior store-based predicate exactly (non-archived +
    // `decision_request.is_pending()`); see `pending_decision_request_count` for
    // the reference oracle. trace:TASK-1065 (supersedes TASK-1061)
    let pending = backend.pending_decision_count().unwrap_or(0);
    if pending > 0 {
        println!(
            "  {} {} decision{} await your call — {}",
            "Decisions:".bold().yellow(),
            pending,
            if pending == 1 { "" } else { "s" },
            "aida questions".cyan()
        );
        println!();
    }

    // (d) home_offer = surface: the keystone cohort, ready for the at-keyboard
    // `--zen` lane. Open (non-terminal, triaged) specs carrying the keystone
    // parking tag. `dont-block` keeps home quiet here.
    if cfg.home_offer == presence::HomeOffer::Surface {
        if let Ok(rows) = backend.list_summaries(&aida_core::ListFilter {
            tags: vec![KEYSTONE_TAG.to_string()],
            ..Default::default()
        }) {
            let ready: Vec<String> = rows
                .iter()
                .filter(|r| {
                    matches!(
                        r.status.to_ascii_lowercase().as_str(),
                        "approved" | "planned" | "in-progress" | "inprogress"
                    )
                })
                .filter_map(|r| r.agreed_id.clone().or_else(|| r.spec_id.clone()))
                .collect();
            if !ready.is_empty() {
                const SHOW: usize = 5;
                let shown = ready
                    .iter()
                    .take(SHOW)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                let overflow = ready.len().saturating_sub(SHOW);
                let tail = if overflow > 0 {
                    format!(" (+{overflow} more)")
                } else {
                    String::new()
                };
                println!(
                    "  {} {} keystone spec{} ready for {} — {}{}",
                    "At keyboard:".bold().green(),
                    ready.len(),
                    if ready.len() == 1 { "" } else { "s" },
                    "--zen".cyan(),
                    shown,
                    tail
                );
                println!();
            }
        }
    }
}

pub(crate) fn print_status_session_section(ctx: &UserStatusContext) {
    println!("{}", "─── Session ───".bold());
    match &ctx.session {
        Some(l) => {
            let short_id = &l.id[..l.id.len().min(8)];
            println!(
                "  {} {}  · scope: {}",
                "id:".dimmed(),
                short_id.yellow(),
                l.scope.cyan()
            );
            println!("  {} {}", "branch:".dimmed(), l.branch.cyan());
            println!(
                "  {} {}",
                "worktree:".dimmed(),
                l.worktree_path.display().to_string().dimmed()
            );
            if let Some(role) = &l.role {
                println!("  {} {}", "role:".dimmed(), role.cyan());
            }
            println!(
                "  {} {}",
                "age:".dimmed(),
                humanize_relative(l.started_at).dimmed()
            );
        }
        None => {
            let extra = match &ctx.role {
                Some(r) => format!(" (active shell role: {})", r.cyan()),
                None => String::new(),
            };
            println!("  (no lease covers cwd){}", extra);
        }
    }
    println!();
}

pub(crate) fn print_status_branch_section(ctx: &UserStatusContext) {
    println!("{}", "─── Branch ───".bold());
    match &ctx.branch {
        Some(b) => {
            let dirty_chip = if b.dirty {
                " (dirty)".yellow().to_string()
            } else {
                " (clean)".green().to_string()
            };
            println!("  {} {}{}", "name:".dimmed(), b.name.cyan(), dirty_chip);
            if let (Some(a), Some(beh)) = (b.ahead_main, b.behind_main) {
                println!(
                    "  {} {} ahead, {} behind {}",
                    "vs main:".dimmed(),
                    a.to_string().green(),
                    if beh > 0 {
                        beh.to_string().yellow()
                    } else {
                        beh.to_string().normal()
                    },
                    "origin/main".dimmed()
                );
            }
            if b.has_upstream {
                if let (Some(a), Some(beh)) = (b.ahead_upstream, b.behind_upstream) {
                    println!(
                        "  {} {} ahead, {} behind",
                        "vs upstream:".dimmed(),
                        a.to_string().green(),
                        if beh > 0 {
                            beh.to_string().yellow()
                        } else {
                            beh.to_string().normal()
                        }
                    );
                }
            } else {
                println!("  {} (no upstream tracked)", "vs upstream:".dimmed());
            }
        }
        None => println!("  (not in a git repository)"),
    }
    println!();
}

pub(crate) fn print_status_pr_section(ctx: &UserStatusContext, focused: bool) {
    println!("{}", "─── PR / CI ───".bold());
    let Some(pr) = &ctx.pr else {
        println!("  (no branch context — can't query gh)");
        if focused {
            println!();
        }
        return;
    };
    match &pr.gh_status {
        GhStatus::Missing => {
            println!(
                "  (gh unavailable — install + auth `gh` to see CI status; \
                 `aida status --no-ci` to suppress this section)"
            );
        }
        GhStatus::Failed(reason) => {
            println!("  (gh failed: {})", reason.dimmed());
        }
        GhStatus::Skipped => {
            println!("  (skipped via --no-ci)");
        }
        // BUG-560: non-GitHub remote — name the right forge, never `gh`.
        GhStatus::NotGitHub(forge::ForgeKind::GitLab) => {
            println!(
                "  {}",
                "(GitLab remote — PR/CI status via `glab` is pending forge \
                 integration; the requirement store works fully)"
                    .dimmed()
            );
        }
        GhStatus::NotGitHub(_) => {
            println!(
                "  {}",
                "(no GitHub remote — the PR/CI section is GitHub-only today; \
                 the requirement store works fully)"
                    .dimmed()
            );
        }
        GhStatus::Ok if pr.state == "none" || pr.number == 0 => {
            println!("  (no open PR for this branch)");
        }
        GhStatus::Ok => {
            println!(
                "  {} {} {} {}",
                "PR:".dimmed(),
                format!("#{}", pr.number).cyan(),
                pr.state.dimmed(),
                pr.title
            );
            println!("  {} {}", "url:".dimmed(), pr.url.dimmed());
            match pr.ci_rollup.as_deref() {
                None => println!("  {} (no checks reported)", "ci:".dimmed()),
                Some("SUCCESS") => {
                    println!(
                        "  {} {}",
                        "ci:".dimmed(),
                        format!("{} SUCCESS", crate::glyph(crate::glyphs::Glyph::Check)).green()
                    )
                }
                Some(r) if r.contains("FAILURE") || r.contains("CANCELLED") => {
                    println!(
                        "  {} {}",
                        "ci:".dimmed(),
                        format!("{} {}", crate::glyph(crate::glyphs::Glyph::Cross), r).red()
                    )
                }
                Some(r) if r.contains("PENDING") || r.contains("IN_PROGRESS") => {
                    println!("  {} {}", "ci:".dimmed(), format!("⏱ {}", r).yellow())
                }
                Some(r) => println!("  {} {}", "ci:".dimmed(), r),
            }
        }
    }
    println!();
}

/// TASK-490: pure split of the queue head into the two display groups, with
/// the counter math the renderer needs ("X more" reflects only not-in-progress
/// items). Lifted out of the renderer so the layout invariants can be tested
/// without stdout capture. trace:TASK-490 | ai:claude
pub(crate) struct QueueViewSplit<'a> {
    pub(crate) in_progress_rows: Vec<&'a QueueRow>,
    pub(crate) next_up_rows: Vec<&'a QueueRow>,
    pub(crate) in_progress_total: usize,
    pub(crate) next_up_total: usize,
}

pub(crate) fn split_queue_view<'a>(head: &'a [QueueRow], queue_total: usize) -> QueueViewSplit<'a> {
    let (in_progress_rows, next_up_rows): (Vec<&QueueRow>, Vec<&QueueRow>) =
        head.iter().partition(|r| r.in_progress);
    let in_progress_total = in_progress_rows.len();
    let next_up_total = queue_total.saturating_sub(in_progress_total);
    QueueViewSplit {
        in_progress_rows,
        next_up_rows,
        in_progress_total,
        next_up_total,
    }
}

pub(crate) fn print_status_queue_section(ctx: &UserStatusContext, _focused: bool) {
    println!("{}", "─── Queue ───".bold());
    let role_label = ctx.role.as_deref().unwrap_or("(no active role)");
    if ctx.queue_total == 0 {
        println!("  (empty for role:{})", role_label.cyan());
    } else {
        println!(
            "  {} routed to role:{}",
            format!("{} item(s)", ctx.queue_total).cyan(),
            role_label.cyan()
        );
        // TASK-490: split queue display into "In progress" + "Next up". The
        // "what's being worked on right now?" signal is the highest-value
        // piece of queue info for an at-keyboard operator — surface it first
        // so it isn't buried in the FIFO tail. trace:TASK-490 | ai:claude
        let QueueViewSplit {
            in_progress_rows,
            next_up_rows,
            in_progress_total,
            next_up_total,
        } = split_queue_view(&ctx.queue_head, ctx.queue_total);

        if !in_progress_rows.is_empty() {
            println!(
                "  {} {}",
                crate::glyph(crate::glyphs::Glyph::InFlight).magenta(),
                format!("In progress ({}):", in_progress_total).magenta()
            );
            for row in &in_progress_rows {
                let lease_chip = match (&row.lease_id, &row.lease_started_at) {
                    (Some(id), Some(started)) => {
                        let short = &id[..id.len().min(8)];
                        format!(
                            "  [{}: {}, {}]",
                            "lease".dimmed(),
                            short.yellow(),
                            humanize_relative(*started).dimmed()
                        )
                    }
                    _ => String::new(),
                };
                println!(
                    "      {} [{}] {}{}",
                    row.spec_id.bold(),
                    status_display::status_badge(&row.status),
                    row.title,
                    lease_chip
                );
            }
        }

        if !next_up_rows.is_empty() {
            println!("  {}", "Next up:".dimmed());
            for (i, row) in next_up_rows.iter().enumerate() {
                // TASK-269: shared status badge. trace:TASK-269 | ai:claude
                println!(
                    "  {:>2}. {} [{}] {}",
                    i + 1,
                    row.spec_id.bold(),
                    status_display::status_badge(&row.status),
                    row.title
                );
            }
            if next_up_total > next_up_rows.len() {
                println!(
                    "    {} {} more (run `aida queue list`)",
                    "…".dimmed(),
                    (next_up_total - next_up_rows.len()).to_string().dimmed()
                );
            }
        }
    }
    println!();
}

/// BUG-609: partition an agent roster into (live, stale). An agent is "stale"
/// when its classified status is `Stale` — which `agent_registry::classify_status`
/// sets iff the registry pid failed the `process_probe::pid_is_alive` liveness
/// probe (or a lease-derived view's lease itself went stale). The headline
/// "Active agents (N)" must count only the live partition so dead-PID corpses
/// (observed 89 registrations where ~9 were live, the rest stale 8-16 days)
/// stop inflating the fleet roster. Reuses the already-probed verdict carried on
/// the view — no second per-agent scan, so the BUG-613 status-timing win holds.
// trace:BUG-609 | ai:claude
pub(crate) fn partition_agents_by_liveness(
    agents: &[agent_registry::AgentRegistryView],
) -> (
    Vec<agent_registry::AgentRegistryView>,
    Vec<agent_registry::AgentRegistryView>,
) {
    agents
        .iter()
        .cloned()
        .partition(|a| a.status != agent_registry::AgentStatus::Stale)
}

/// `show_stale` (from `aida status --all`/`--stale`) reveals the dead-PID
/// corpses the default view hides behind a footer count.
// trace:BUG-609 | ai:claude
pub(crate) fn print_status_agents_section(ctx: &UserStatusContext, show_stale: bool) {
    if ctx.agents.is_empty() {
        return;
    }
    let (live, stale) = partition_agents_by_liveness(&ctx.agents);
    // Even when every agent is stale, render the section (headline "(0)" plus
    // the footer) so the operator sees that the roster is all corpses rather
    // than the section silently vanishing.
    println!(
        "{}",
        format!("─── Active agents ({}) ───", live.len()).bold()
    );
    let shown: &[agent_registry::AgentRegistryView] = if show_stale { &ctx.agents } else { &live };
    for line in agent_registry::format_agent_status_lines(shown) {
        println!("{line}");
    }
    for line in agent_registry::mcp_authority_status_lines(shown) {
        println!("{line}");
    }
    if !stale.is_empty() && !show_stale {
        println!(
            "  {}",
            format!(
                "+{} stale (dead PID) — `aida status --all` to show, `aida doctor heal` to reap",
                stale.len()
            )
            .dimmed()
        );
    }
    println!();
}

/// SPIKE-30: query `claude agents --json` and cross-reference live Claude
/// Code sessions against AIDA's lease registry. Surfaces three signals the
/// existing hygiene scan can't:
///   - Linked: AIDA lease ↔ live Claude session pair (the healthy case)
///   - Untracked: Claude session running in a project worktree with no
///     matching AIDA lease (interactive shell, or a rogue launch)
///   - Section-wide counts: cross-substrate one-glance view
///
/// Skips silently when `claude` isn't on PATH (cross-tool, optional dep).
/// Dormant leases (lease present, no Claude session) are intentionally
/// NOT re-reported here — the Hygiene section already covers them via
/// process_probe.rs; duplicating would dilute both signals.
/// trace:SPIKE-30 | ai:claude
pub(crate) fn print_status_claude_code_section(project_root: &std::path::Path) {
    let Some(entries) = claude_agents::list_agents() else {
        return;
    };
    if entries.is_empty() {
        return;
    }
    let leases = list_leases(project_root);
    let worktree_paths: Vec<std::path::PathBuf> =
        leases.iter().map(|l| l.worktree_path.clone()).collect();
    let (in_scope, elsewhere) =
        claude_agents::partition_by_project(&entries, project_root, &worktree_paths);

    let manifests = session_manifest::list_all(project_root);

    // Build claude_session_id → lease lookup via manifest join, with cwd
    // fallback when the manifest doesn't carry the join key (legacy leases
    // pre-dating TASK-112's claude_session_id recording).
    let mut linked: Vec<(claude_agents::ClaudeAgentEntry, SessionLease)> = Vec::new();
    let mut untracked: Vec<claude_agents::ClaudeAgentEntry> = Vec::new();
    for entry in &in_scope {
        let manifest = manifests
            .iter()
            .find(|m| m.claude_session_id.as_deref() == Some(entry.session_id.as_str()));
        let lease = manifest
            .and_then(|m| {
                leases
                    .iter()
                    .find(|l| l.id.starts_with(&m.session_id) || m.session_id.starts_with(&l.id))
            })
            .or_else(|| leases.iter().find(|l| l.worktree_path == entry.cwd));
        match lease {
            Some(l) => linked.push((entry.clone(), l.clone())),
            None => untracked.push(entry.clone()),
        }
    }

    println!("{}", "─── Claude Code ───".bold());
    println!(
        "  Live: {} in project, {} elsewhere",
        in_scope.len().to_string().cyan(),
        elsewhere.len().to_string().dimmed(),
    );
    if !linked.is_empty() {
        println!(
            "  Linked (lease ↔ Claude session): {}",
            linked.len().to_string().green()
        );
        for (entry, lease) in &linked {
            let role = lease.role.as_deref().unwrap_or("?");
            println!(
                "    {}  {}  ({})  {}",
                entry.short_session_id().yellow(),
                lease.scope.cyan(),
                role.dimmed(),
                claude_status_chip(entry),
            );
        }
    }
    // Split untracked into:
    //   - cwd == project_root → interactive shells (the user's terminals);
    //     collapsed to a count because these are expected and high-volume
    //   - cwd in a worktree but no matching lease → drift (rogue launches,
    //     lease cleaned up while Claude still alive); always shown in full
    let (shell_count, drift): (usize, Vec<_>) =
        untracked
            .iter()
            .fold((0, Vec::new()), |(mut shells, mut drift), entry| {
                if entry.cwd == project_root {
                    shells += 1;
                } else {
                    drift.push(entry.clone());
                }
                (shells, drift)
            });
    if shell_count > 0 {
        println!(
            "  Interactive shells in project root: {}",
            shell_count.to_string().dimmed(),
        );
    }
    if !drift.is_empty() {
        println!(
            "  In worktree without lease ({}): {}",
            drift.len().to_string().yellow(),
            "rogue Claude or stale lease cleanup".dimmed(),
        );
        for entry in drift.iter().take(5) {
            println!(
                "    {}  {}  {}",
                entry.short_session_id().yellow(),
                entry.cwd.display().to_string().dimmed(),
                claude_status_chip(entry),
            );
        }
        if drift.len() > 5 {
            println!(
                "    {}",
                format!("... and {} more", drift.len() - 5).dimmed()
            );
        }
    }
    println!();
}

/// SPIKE-30 helper: render a compact status chip from a Claude agent entry.
/// Uses `status` when set; falls back to `kind` (e.g. "interactive") when
/// not — the live JSON for a freshly-spawned session typically omits status
/// for the first few seconds.
pub(crate) fn claude_status_chip(entry: &claude_agents::ClaudeAgentEntry) -> String {
    let label = entry.status.as_deref().unwrap_or(entry.kind.as_str());
    let painted = match label {
        "busy" | "working" | "Working" => label.green().to_string(),
        "idle" | "Idle" => label.dimmed().to_string(),
        "blocked" | "Needs input" => label.yellow().to_string(),
        "failed" | "Failed" => label.red().to_string(),
        _ => label.normal().to_string(),
    };
    format!("{} {}", crate::glyph(crate::glyphs::Glyph::Bullet), painted)
}

pub(crate) fn print_status_short(ctx: &UserStatusContext) {
    let role = ctx.role.as_deref().unwrap_or("-");
    let scope = ctx
        .session
        .as_ref()
        .map(|l| l.scope.as_str())
        .unwrap_or("-");
    let branch = ctx.branch.as_ref().map(|b| b.name.as_str()).unwrap_or("-");
    let dirty = ctx
        .branch
        .as_ref()
        .map(|b| if b.dirty { "*" } else { "" })
        .unwrap_or("");
    let queue = ctx.queue_total;
    let pr_chip = match &ctx.pr {
        Some(p) if matches!(p.gh_status, GhStatus::Ok) && p.number > 0 => {
            let ci = p
                .ci_rollup
                .as_deref()
                .map(|r| {
                    if r == "SUCCESS" {
                        format!("ci{}", crate::glyph(crate::glyphs::Glyph::Check))
                    } else if r.contains("FAIL") || r.contains("CANCEL") {
                        format!("ci{}", crate::glyph(crate::glyphs::Glyph::Cross))
                    } else if r.contains("PENDING") || r.contains("IN_PROGRESS") {
                        "ci⏱".to_string()
                    } else {
                        "ci?".to_string()
                    }
                })
                .unwrap_or_else(|| "ci?".to_string());
            format!(" PR#{} {}", p.number, ci)
        }
        _ => String::new(),
    };
    println!(
        "role:{} scope:{} branch:{}{} queue:{}{}",
        role, scope, branch, dirty, queue, pr_chip
    );
}

pub(crate) fn print_status_json(
    ctx: &UserStatusContext,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    project_root: &std::path::Path,
    queue_only: bool,
    ci_only: bool,
    awaiting: &awaiting_you::AwaitingReport,
    absence: Option<serde_json::Value>,
) -> Result<()> {
    use serde_json::json;

    let session = ctx.session.as_ref().map(|l| {
        json!({
            "id": l.id,
            "scope": l.scope,
            "branch": l.branch,
            "worktree": l.worktree_path.display().to_string(),
            "role": l.role,
            "started_at": l.started_at.to_rfc3339(),
        })
    });
    let branch = ctx.branch.as_ref().map(|b| {
        json!({
            "name": b.name,
            "dirty": b.dirty,
            "ahead_main": b.ahead_main,
            "behind_main": b.behind_main,
            "ahead_upstream": b.ahead_upstream,
            "behind_upstream": b.behind_upstream,
            "has_upstream": b.has_upstream,
        })
    });
    let pr = ctx.pr.as_ref().map(|p| match &p.gh_status {
        GhStatus::Ok if p.number > 0 => json!({
            "number": p.number,
            "title": p.title,
            "url": p.url,
            "state": p.state,
            "ci_rollup": p.ci_rollup,
        }),
        GhStatus::Ok => json!({ "state": "none" }),
        GhStatus::Missing => json!({ "error": "gh-missing" }),
        GhStatus::Failed(r) => json!({ "error": "gh-failed", "reason": r }),
        GhStatus::Skipped => json!({ "skipped": true }),
        // BUG-560: non-GitHub remote — report the forge, not a gh error.
        GhStatus::NotGitHub(kind) => json!({
            "forge": kind.config_token(),
            "note": "PR/CI section is GitHub-only today; the requirement store works fully",
        }),
    });
    let head_json = |r: &QueueRow| {
        let mut obj = json!({
            "spec_id": r.spec_id,
            "title": r.title,
            "status": r.status,
            "for_role": r.for_role,
            "in_progress": r.in_progress,
        });
        if r.in_progress {
            if let Some(id) = &r.lease_id {
                obj["lease_id"] = json!(id);
            }
            if let Some(started) = r.lease_started_at {
                obj["lease_started_at"] = json!(started.to_rfc3339());
            }
        }
        obj
    };
    let queue_split = split_queue_view(&ctx.queue_head, ctx.queue_total);
    let queue = json!({
        "role": ctx.role,
        "total": ctx.queue_total,
        "in_progress_count": queue_split.in_progress_total,
        "next_up_count": queue_split.next_up_total,
        "head": ctx.queue_head.iter().map(head_json).collect::<Vec<_>>(),
    });

    let mut out = serde_json::Map::new();
    if !queue_only && !ci_only {
        // STORY-465: lead the JSON the same way the text view leads —
        // human-gate items first. Always present (even when empty) so
        // consumers can detect "no items" without a key-absence guard.
        let absence_json = absence.unwrap_or_else(|| {
            json!({
                "absence_days": serde_json::Value::Null,
                "last_activity_at": serde_json::Value::Null,
            })
        });
        out.insert(
            "absence_days".to_string(),
            absence_json
                .get("absence_days")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        );
        out.insert(
            "last_activity_at".to_string(),
            absence_json
                .get("last_activity_at")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        );
        out.insert("absence".to_string(), absence_json);
        out.insert("awaiting".to_string(), awaiting.to_json());
        out.insert(
            "session".to_string(),
            session.unwrap_or(serde_json::Value::Null),
        );
        out.insert(
            "role".to_string(),
            serde_json::Value::from(ctx.role.clone()),
        );
        out.insert(
            "branch".to_string(),
            branch.unwrap_or(serde_json::Value::Null),
        );
    }
    if !queue_only {
        out.insert("pr".to_string(), pr.unwrap_or(serde_json::Value::Null));
    }
    if !ci_only {
        out.insert("queue".to_string(), queue);
    }
    if !queue_only && !ci_only {
        out.insert(
            "agents".to_string(),
            json!(ctx
                .agents
                .iter()
                .map(|a| {
                    json!({
                        "id": a.id,
                        "agent_type": a.agent_type,
                        "pid": a.pid,
                        "tty": a.tty,
                        "started_at": a.started_at,
                        "last_active_at": a.last_active_at,
                        "role": a.role,
                        "current_spec": a.current_spec,
                        "status": a.status.as_str(),
                        "worktree_path": a.worktree_path.display().to_string(),
                        "source": a.source,
                        "binary_version": a.binary_version,
                        "build_sha": a.build_sha,
                    })
                })
                .collect::<Vec<_>>()),
        );
        let cache = backend.cache();
        let recorded = cache.source_head_sha().ok().flatten().unwrap_or_default();
        let actual = aida_core::git_ops::head_sha(store_path).unwrap_or_default();
        let stale = recorded != actual || recorded.is_empty();
        out.insert(
            "cache".to_string(),
            json!({
                "fresh": !stale || actual.is_empty(),
                "rows": backend.requirement_count().unwrap_or(0),
            }),
        );

        // TASK-662: machine-readable findings detail (count + per-finding
        // rows), mirroring the text Findings section — draft-filtered so the
        // count matches `aida findings list`. trace:TASK-662 | ai:claude
        {
            let f_filter = aida_core::ListFilter {
                status: Some("draft".to_string()),
                ..Default::default()
            };
            if let Ok(summaries) = backend.list_summaries(&f_filter) {
                let sections = crate::findings::build_findings_view(
                    &summaries,
                    &crate::findings::FindingsFilter::default(),
                );
                let total = crate::findings::count_findings(&sections);
                let mut items = Vec::new();
                let mut current_ids: Vec<String> = Vec::new();
                for section in &sections {
                    let source = section.source.label();
                    for group in &section.groups {
                        for row in &group.rows {
                            current_ids.push(row.display_id.clone());
                            items.push(json!({
                                "id": row.display_id,
                                "source": source,
                                "origin": group.origin,
                                "severity": row.severity.label(),
                                "kind": row.kind,
                                "recurrence": row.recurrence,
                                "title": row.title,
                            }));
                        }
                    }
                }
                // TASK-662: delta-since-last-run. Diff the current finding IDs
                // against the snapshot persisted on the previous `aida status`
                // run, then re-baseline. `delta` is null on the very first run
                // (no prior snapshot to diff). trace:TASK-662 | ai:claude
                let previous = read_last_findings(project_root);
                let delta = compute_findings_delta(previous.as_deref(), &current_ids);
                write_last_findings(project_root, &current_ids);
                let delta_json = match &delta {
                    Some(d) => serde_json::to_value(d).unwrap_or(serde_json::Value::Null),
                    None => serde_json::Value::Null,
                };
                out.insert(
                    "findings".to_string(),
                    json!({ "pending": total, "items": items, "delta": delta_json }),
                );
            }
        }

        // SPIKE-30: emit the same Claude Code cross-substrate view machine-
        // readable when the operator passes `--json`. Absent when the
        // `claude` binary isn't on PATH — graceful no-op, not an error key.
        if let Some(value) = claude_code_status_json() {
            out.insert("claude_code".to_string(), value);
        }

        // STORY-456: unified worktrees + open-PRs + recently-merged panes,
        // mirroring the text sections so machine consumers get the same merge
        // (worktree + lease + liveness + ahead + PR + obsolescence verdict).
        // Always present (possibly empty arrays) so consumers needn't guard on
        // key-absence. trace:STORY-456 | ai:claude
        let main_root = main_worktree_root_from(project_root);
        let worktree_rows = collect_worktree_status_rows(&main_root);
        out.insert(
            "worktrees".to_string(),
            json!(worktree_rows
                .iter()
                .map(|r| json!({
                    "path": r.path.display().to_string(),
                    "branch": r.branch,
                    "tied_spec": r.tied_spec,
                    "lease_scope": r.lease_scope,
                    "live": r.has_live,
                    "dirty_count": r.dirty_count,
                    "ahead_main": r.ahead,
                    "pr_number": r.pr_number,
                    "pr_ci": r.pr_ci,
                    "pr_mergeable": r.pr_mergeable,
                    "obsolete": r.obsolete,
                }))
                .collect::<Vec<_>>()),
        );
        let open_pr_snapshot = collect_open_prs(&main_root);
        let mut open_prs: Vec<&status_cleanup::OpenPrItem> =
            open_pr_snapshot.by_branch.values().collect();
        open_prs.sort_by_key(|p| p.number);
        out.insert(
            "open_prs".to_string(),
            json!(open_prs
                .iter()
                .map(|p| {
                    let merge = p.mergeable.as_deref().unwrap_or("UNKNOWN");
                    json!({
                        "number": p.number,
                        "title": p.title,
                        "head_branch": p.head_branch,
                        "ci": p.ci_rollup,
                        "mergeable": p.mergeable,
                        "review_decision": p.review_decision,
                        "next_step": open_pr_next_step(
                            p.ci_rollup.as_deref().unwrap_or("?"),
                            &merge.to_ascii_uppercase(),
                            p.review_decision.as_deref(),
                        ),
                    })
                })
                .collect::<Vec<_>>()),
        );
        out.insert(
            "recently_merged".to_string(),
            json!(collect_recently_merged_prs(&main_root, 5)
                .iter()
                .map(|(number, title, when)| json!({
                    "number": number,
                    "title": title,
                    "merged_at": when,
                }))
                .collect::<Vec<_>>()),
        );
    }
    println!("{}", crate::cache_output::json_pretty(&out)?);
    Ok(())
}

/// SPIKE-30 JSON projection: same cross-substrate view as
/// `print_status_claude_code_section`, structured for machine consumers.
/// Returns `None` (caller omits the key) when `claude` isn't available.
pub(crate) fn claude_code_status_json() -> Option<serde_json::Value> {
    use serde_json::json;

    let entries = claude_agents::list_agents()?;
    let project_root = std::env::current_dir().ok()?;
    let leases = list_leases(&project_root);
    let worktree_paths: Vec<std::path::PathBuf> =
        leases.iter().map(|l| l.worktree_path.clone()).collect();
    let (in_scope, elsewhere) =
        claude_agents::partition_by_project(&entries, &project_root, &worktree_paths);
    let manifests = session_manifest::list_all(&project_root);

    let mut linked = Vec::new();
    let mut shells = 0usize;
    let mut drift = Vec::new();
    for entry in &in_scope {
        let manifest = manifests
            .iter()
            .find(|m| m.claude_session_id.as_deref() == Some(entry.session_id.as_str()));
        let lease = manifest
            .and_then(|m| {
                leases
                    .iter()
                    .find(|l| l.id.starts_with(&m.session_id) || m.session_id.starts_with(&l.id))
            })
            .or_else(|| leases.iter().find(|l| l.worktree_path == entry.cwd));
        match lease {
            Some(l) => linked.push(json!({
                "session_id": entry.session_id,
                "pid": entry.pid,
                "cwd": entry.cwd.display().to_string(),
                "kind": entry.kind,
                "status": entry.status,
                "started_at_ms": entry.started_at_ms,
                "lease_id": l.id,
                "scope": l.scope,
                "role": l.role,
                "branch": l.branch,
            })),
            None if entry.cwd == project_root => shells += 1,
            None => drift.push(json!({
                "session_id": entry.session_id,
                "pid": entry.pid,
                "cwd": entry.cwd.display().to_string(),
                "kind": entry.kind,
                "status": entry.status,
                "started_at_ms": entry.started_at_ms,
            })),
        }
    }

    Some(json!({
        "live_in_project": in_scope.len(),
        "live_elsewhere": elsewhere.len(),
        "linked": linked,
        "interactive_shells_in_root": shells,
        "drift_in_worktree": drift,
    }))
}

#[cfg(test)]
#[path = "tests/bug_560_status_forge_tests.rs"]
mod bug_560_status_forge_tests;

#[cfg(test)]
#[path = "tests/task_490_status_in_progress_tests.rs"]
mod task_490_status_in_progress_tests;

#[cfg(test)]
#[path = "tests/task_1750_liveness_primitive_guard_tests.rs"]
mod task_1750_liveness_primitive_guard_tests;
