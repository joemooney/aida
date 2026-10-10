/// BUG-1468: a PR branch's green check is evidence about the guards that
/// existed WHEN IT RAN. Nothing re-evaluates that green when a new required
/// guard lands on main, so a long-lived branch can present a green check
/// that no longer means what a reader assumes (observed on PR #1979 —
/// `aida-core/templates/.aida/discipline/session-discipline.md` grew a
/// content check 15h after the branch's own CI ran, and the branch still
/// showed green).
///
/// TWO TIERS (BUG-1468 follow-up — the single-tier version refused almost
/// every ship in this repo, because nearly every commit on main touches
/// `*/tests/` or `*_tests.rs`, making `--override-stale-check` routine):
/// - `definition_files`: a `.github/workflows/*` file whose `on:` triggers
///   include `pull_request` (a plain substring check on the file's content
///   at `base_ref` — nightly/cron-only, release-only, and dispatch-only
///   workflows don't gate a PR's own check, so they do NOT count), or a
///   `scripts/` file one of THOSE PR-triggered workflows invokes directly —
///   the check's own DEFINITION changed, so the green no longer means what
///   it looks like. `aida pr ship` REFUSES on this tier (override-able); the
///   drain's merge phase only WARNS and proceeds (BUG-1468 follow-up 2).
/// - `test_files`: WARN-only, everywhere. An ordinary test file changed on
///   base since divergence — the common, usually-harmless "base moved" case;
///   still worth surfacing (a reader may want to re-run), never worth
///   refusing.
// trace:BUG-1468 | ai:claude
pub(crate) struct StaleCheckWarning {
    pub(crate) behind_commits: u64,
    pub(crate) definition_files: Vec<String>,
    pub(crate) test_files: Vec<String>,
}

/// Pure classifier for the two tiers above. `workflow_is_pr_triggered`
/// decides whether a changed `.github/workflows/...` file's `on:` triggers
/// include `pull_request` (a non-PR-triggered workflow — cron/nightly,
/// release, manual dispatch — does NOT count as a definition change at
/// all: it lands in neither tier). `script_is_pr_workflow_invoked` decides
/// whether a `scripts/...` path is one a PR-triggered workflow calls
/// directly. Callers resolve both via git on `base_ref` (tests fake them
/// directly, no git needed).
// trace:BUG-1468 | ai:claude
pub(crate) fn classify_changed_files(
    changed_files: &[String],
    workflow_is_pr_triggered: &dyn Fn(&str) -> bool,
    script_is_pr_workflow_invoked: &dyn Fn(&str) -> bool,
) -> (Vec<String>, Vec<String>) {
    let mut definition = Vec::new();
    let mut test_only = Vec::new();
    for f in changed_files {
        if is_workflow_path(f) {
            if workflow_is_pr_triggered(f) {
                definition.push(f.clone());
            }
            // else: a nightly/cron/dispatch/release-only workflow — its
            // green never covered this PR's check to begin with, so it
            // doesn't count in either tier.
        } else if is_script_path(f) && script_is_pr_workflow_invoked(f) {
            definition.push(f.clone());
        } else if is_test_path(f) {
            test_only.push(f.clone());
        }
    }
    (definition, test_only)
}

// trace:BUG-1468 | ai:claude
pub(crate) fn is_workflow_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with(".github/workflows/") || lower.contains("/.github/workflows/")
}

// trace:BUG-1468 | ai:claude
pub(crate) fn is_script_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("scripts/") || lower.contains("/scripts/")
}

// trace:BUG-1468 | ai:claude
pub(crate) fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("tests/")
        || lower.contains("/tests/")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_tests.rs")
        || lower.ends_with("_test.py")
        || lower.ends_with("_test.sh")
}

/// True when a `.github/workflows/...` file's content, AT `base_ref`,
/// mentions `pull_request` — a plain substring check (no YAML parsing),
/// standing in for "this workflow's `on:` triggers include `pull_request`"
/// (BUG-1468 follow-up 2). A nightly/cron, release, or manual-dispatch-only
/// workflow never gates a PR's own check, so it must not count as a
/// definition change even though it lives under `.github/workflows/`.
/// False on any git error (fail-open — a git hiccup demotes a definition
/// change to "doesn't count" rather than blocking a ship on its own).
// trace:BUG-1468 | ai:claude
pub(crate) fn workflow_is_pr_triggered(
    repo: &std::path::Path,
    base_ref: &str,
    workflow_path: &str,
) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show", &format!("{base_ref}:{workflow_path}")])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("pull_request"))
        .unwrap_or(false)
}

/// The `.github/workflows/*` files, AT `base_ref`, whose content mentions
/// `pull_request` — the PR-triggered subset a `scripts/` change must be
/// invoked by to count as a definition change. Empty on any git error
/// (fail-open — see [`workflow_is_pr_triggered`]).
// trace:BUG-1468 | ai:claude
pub(crate) fn pr_triggered_workflow_files(repo: &std::path::Path, base_ref: &str) -> Vec<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "grep",
            "-l",
            "-F",
            "pull_request",
            base_ref,
            "--",
            ".github/workflows",
        ])
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    // `git grep -l <rev> -- <pathspec>` prints "<rev>:<path>" per match.
    let prefix = format!("{base_ref}:");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix(prefix.as_str()).map(|p| p.to_string()))
        .collect()
}

/// True when `script_path` (a `scripts/...` file that changed) is invoked
/// directly by one of the PR-TRIGGERED workflows on `base_ref` — a literal
/// substring match of the path inside those tracked `.github/workflows/*`
/// blobs. A workflow that invokes the same script but isn't itself
/// PR-triggered (nightly, release, dispatch-only) does not count (BUG-1468
/// follow-up 2). False on any git error (fail-open).
// trace:BUG-1468 | ai:claude
pub(crate) fn script_referenced_by_pr_workflows(
    repo: &std::path::Path,
    base_ref: &str,
    script_path: &str,
) -> bool {
    pr_triggered_workflow_files(repo, base_ref)
        .iter()
        .any(|wf| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["show", &format!("{base_ref}:{wf}")])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).contains(script_path))
                .unwrap_or(false)
        })
}

/// Resolve the ref to measure `branch`'s staleness FROM: `origin/<branch>`
/// when that remote-tracking ref exists (what a PR's own CI actually ran
/// against), else the local `branch` ref itself (BUG-1468 follow-up 3 — a
/// local checkout can be ahead or behind what's actually pushed/reviewed).
// trace:BUG-1468 | ai:claude
pub(crate) fn stale_check_branch_ref(repo: &std::path::Path, branch: &str) -> String {
    let remote_ref = format!("origin/{branch}");
    let exists = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", &remote_ref])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if exists {
        remote_ref
    } else {
        branch.to_string()
    }
}

/// git-IO wrapper: how far `branch` is behind `base_ref`, and the two-tier
/// classification (see [`StaleCheckWarning`]) of what changed on `base_ref`
/// since divergence. Staleness is measured from `origin/<branch>` when that
/// ref exists, else the local `branch` ref (see [`stale_check_branch_ref`]).
/// `None` when the branch is not behind base (nothing to warn about) or on
/// a git error — same fail-open convention as [`branch_behind_main`], so a
/// git hiccup never blocks a ship.
// trace:BUG-1468 | ai:claude
pub(crate) fn pr_stale_check_warning(
    repo: &std::path::Path,
    branch: &str,
    base_ref: &str,
) -> Option<StaleCheckWarning> {
    let branch_ref = stale_check_branch_ref(repo, branch);
    let count_out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        // trace:BUG-1622 | ai:claude
        .args([
            "rev-list",
            "--count",
            git_arg_guard::END_OF_OPTIONS,
            &format!("{branch_ref}..{base_ref}"),
        ])
        .output()
        .ok()?;
    if !count_out.status.success() {
        return None;
    }
    let behind_commits: u64 = String::from_utf8_lossy(&count_out.stdout)
        .trim()
        .parse()
        .ok()?;
    if behind_commits == 0 {
        return None;
    }
    let diff_out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        // trace:BUG-1622 | ai:claude
        .args([
            "diff",
            "--name-only",
            git_arg_guard::END_OF_OPTIONS,
            &format!("{branch_ref}...{base_ref}"),
            "--",
        ])
        .output()
        .ok()?;
    if !diff_out.status.success() {
        return None;
    }
    let changed: Vec<String> = String::from_utf8_lossy(&diff_out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();
    let (definition_files, test_files) = classify_changed_files(
        &changed,
        &|workflow_path| workflow_is_pr_triggered(repo, base_ref, workflow_path),
        &|script_path| script_referenced_by_pr_workflows(repo, base_ref, script_path),
    );
    Some(StaleCheckWarning {
        behind_commits,
        definition_files,
        test_files,
    })
}

#[cfg(test)]
#[path = "tests/bug_1468_stale_check_tests.rs"]
mod bug_1468_stale_check_tests;

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

/// BUG-61: SIGTERM each pid, sleep `grace_secs`, then SIGKILL any that
/// are still alive. trace:BUG-61 | ai:claude
/// Which of `pids` are running right now.
///
/// A ZOMBIE does not count. A terminated child stays in the process table until its parent
/// reaps it, and sysinfo still lists it — so treating "present" as "alive" would make
/// `agent stop` report that it had failed to kill something it had just killed.
// trace:BUG-1703 | ai:claude
pub(crate) fn live_pids(pids: &[u32]) -> Vec<u32> {
    use sysinfo::{ProcessRefreshKind, ProcessStatus, RefreshKind, System};
    let mut sys =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    sys.refresh_processes_specifics(ProcessRefreshKind::new());
    pids.iter()
        .copied()
        .filter(|&pid| {
            sys.process(sysinfo::Pid::from_u32(pid))
                .is_some_and(|p| !matches!(p.status(), ProcessStatus::Zombie | ProcessStatus::Dead))
        })
        .collect()
}

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

/// STORY-73: resolution chain for `aida session end` (no arg). Tries in
/// order, stopping at first hit:
///
///   1. cwd-based: lease whose worktree_path contains cwd. (existing flow)
///   2. AIDA_SESSION_ID env var: exported by `session start` when wrapped
///      via the shell helper; matches a live lease by id prefix.
///   3. ancestor PID: walks the calling shell's ancestors via sysinfo and
///      matches against any lease's `creator_pid`. Catches the common case
///      where the user ran `start` then `end` from the same shell, never
///      cd'd into the worktree, and AIDA_SESSION_ID isn't set (e.g., shell
///      helpers not installed).
///   4. single-active fallback: if exactly one lease is active for this
///      project, prompt y/N (or auto-accept with -y).
///   5. error: list active leases and ask for an explicit id.
///
// ----------------------------------------------------------------------------
// TASK-111: CI-aware `aida session end`.
//
// When a session has an associated PR (discovered via `gh pr list --head
// <branch>`), `aida session end` probes the PR's CI conclusion and chooses
// an action based on a small decision tree:
//
//   No PR / No gh         → Proceed silently (today's behavior preserved
//                            for non-PR sessions; gh-missing is graceful)
//   PR exists, no CI runs → Info: "PR opened, CI hasn't started" + proceed
//   PR exists, CI running → Prompt (default) / Wait (--wait-ci) / Cancel
//   PR exists, CI green   → Info: format!("{} CI green", crate::glyph(crate::glyphs::Glyph::Check)) + proceed
//   PR exists, CI red     → Warn + prompt to keep session for fixups
//
// Both `--skip-ci` and `--force` bypass the probe entirely. With `--yes`
// (non-interactive), running-CI defaults to proceed (you opted out of
// prompts; we don't strand the script). Red-CI with `--yes` still
// proceeds but the warning is loud.
//
// trace:TASK-111 | ai:claude
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CiProbe {
    /// No gh on PATH, or no PR for this branch, or gh call failed in a way
    /// we don't want to surface (network blip, auth issue) — degrade
    /// silently to today's behavior. The reason is included for the
    /// info-line when logging.
    NoSignal(String),
    /// PR exists but CI hasn't started (no workflow runs yet).
    PrNoChecks { pr_number: u32 },
    /// CI is in progress on the latest commit.
    InProgress { pr_number: u32 },
    /// All checks passed.
    Green { pr_number: u32 },
    /// At least one check failed.
    Red {
        pr_number: u32,
        failed_summary: String,
    },
}

// trace:BUG-1818 | ai:codex
pub(crate) fn ci_probe_change_id(probe: &CiProbe) -> Option<u32> {
    match probe {
        CiProbe::PrNoChecks { pr_number }
        | CiProbe::InProgress { pr_number }
        | CiProbe::Green { pr_number }
        | CiProbe::Red { pr_number, .. } => Some(*pr_number),
        CiProbe::NoSignal(_) => None,
    }
}

/// Pure policy for an unavailable CI probe while a wait is active.
/// Transient failures get the configured retry budget; permanent failures
/// close the wait immediately, and exhaustion returns `NoSignal` so the drain
/// gate can shelve with the typed `ci-unavailable` cause.
// trace:BUG-1250 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CiProbeFailureAction {
    Retry,
    Unavailable,
}

pub(crate) fn decide_ci_probe_failure(
    reason: &str,
    consecutive_failures: u32,
    max_attempts: u32,
    transient_patterns: &[String],
) -> CiProbeFailureAction {
    if crate::network_retry::classify_transient(reason, transient_patterns)
        && consecutive_failures < max_attempts.max(1)
    {
        CiProbeFailureAction::Retry
    } else {
        CiProbeFailureAction::Unavailable
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CiAction {
    /// Run the rest of session_end as today.
    Proceed,
    /// Block on a poll loop until CI reaches a terminal state, then
    /// re-decide.
    Wait,
    /// Refuse to end the session; the message includes the reason for
    /// the user.
    Cancel(String),
}

/// Pure decision function: given a probe result + flags, decide what to
/// do. Pure so the unit tests can pin every branch without spawning gh.
/// trace:TASK-111 | ai:claude
pub(crate) fn decide_ci_action(probe: &CiProbe, wait_ci: bool, yes: bool) -> CiAction {
    match probe {
        CiProbe::NoSignal(reason) => {
            // Stay quiet for the common "no PR yet" case; only surface
            // info when something interesting (gh missing, lookup failed)
            // would help the user notice.
            if !reason.is_empty() && !reason.contains("no open PR") {
                eprintln!(
                    "  {} {}",
                    "(ci-probe:".dimmed(),
                    format!("{})", reason).dimmed()
                );
            }
            CiAction::Proceed
        }
        CiProbe::PrNoChecks { pr_number } => {
            eprintln!(
                "  {} PR-{} opened, CI hasn't started yet — ending anyway.",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                pr_number,
            );
            CiAction::Proceed
        }
        CiProbe::Green { pr_number } => {
            eprintln!(
                "  {} CI green on PR-{}.",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                pr_number
            );
            CiAction::Proceed
        }
        CiProbe::InProgress { pr_number } => {
            if wait_ci {
                // TASK-233: flag-neutral wording — the caller passes
                // `--wait-ci || --watch-ci` and prints the variant-
                // specific intro line itself.
                eprintln!(
                    "  {} CI in progress on PR-{} — blocking until CI completes.",
                    crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
                    pr_number,
                );
                CiAction::Wait
            } else if yes {
                eprintln!(
                    "  {} CI in progress on PR-{} (proceeding — --yes set; pass --wait-ci to block).",
                    crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
                    pr_number,
                );
                CiAction::Proceed
            } else {
                CiAction::Cancel(format!(
                    "  {} CI is still in progress on PR-{}.\n  Options:\n    --wait-ci   block until CI completes\n    --skip-ci   release lease now (you'll have to push fixups in a new session if CI goes red)\n    --force     release lease unconditionally",
                    crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
                    pr_number,
                ))
            }
        }
        CiProbe::Red {
            pr_number,
            failed_summary,
        } => {
            if yes {
                eprintln!(
                    "  {} CI RED on PR-{}: {}\n  ({} set — ending anyway. Push fixups in a new session via `aida queue rework`.)",
                    crate::glyph(crate::glyphs::Glyph::Cross).red(),
                    pr_number,
                    failed_summary.dimmed(),
                    "--yes".dimmed(),
                );
                CiAction::Proceed
            } else {
                CiAction::Cancel(format!(
                    "  {} CI RED on PR-{}: {}\n  Recommended: keep this session alive and push fixups (the implementer's lease lets you commit without re-claiming).\n  To end anyway: pass --force or --yes.",
                    crate::glyph(crate::glyphs::Glyph::Cross).red(),
                    pr_number,
                    failed_summary,
                ))
            }
        }
    }
}

/// Side-effecting probe: shell out to gh and parse the result. Returns a
/// `CiProbe::NoSignal` for any failure path so callers don't have to
/// distinguish between "no PR" and "gh broken" — both degrade to "proceed
/// silently." trace:TASK-111 | ai:claude
/// STORY-516: forge-routed CI probe, so a GitLab / pure-git repo goes through
/// its own provider. `project_root` SELECTS the provider. TASK-1273: it is now
/// always supplied by the caller via `ci_probe_with_forge` — the old
/// `ci_probe_via_forge` wrapper resolved it from the process cwd, which routed
/// a GitLab branch through GitHub whenever the driven worktree was not the
/// agent's cwd. Converts the forge-neutral `CiProbeResult` back to `CiProbe` so
/// the existing match sites are unchanged; a provider Err collapses to
/// `NoSignal`.
/// STORY-516: reverse of `ci_probe_result_from_ci_probe` — convert a forge
/// `CiProbeResult` (incl. a provider `Err`) back to the orchestrator's
/// `CiProbe`, so the `*_via_forge` CI helpers stay a pure name-swap at their
/// call sites. trace:STORY-516 | ai:claude
// trace:TASK-1273 | ai:claude
pub(crate) fn ci_probe_from_ci_probe_result(r: Result<crate::forge::CiProbeResult>) -> CiProbe {
    match r {
        Ok(crate::forge::CiProbeResult::NoSignal(why)) => CiProbe::NoSignal(why),
        Ok(crate::forge::CiProbeResult::NoChecks { change }) if change > 0 => CiProbe::PrNoChecks {
            pr_number: change as u32,
        },
        Ok(crate::forge::CiProbeResult::InProgress { change }) if change > 0 => {
            CiProbe::InProgress {
                pr_number: change as u32,
            }
        }
        Ok(crate::forge::CiProbeResult::Green { change }) if change > 0 => CiProbe::Green {
            pr_number: change as u32,
        },
        Ok(crate::forge::CiProbeResult::Failed { change, summary }) if change > 0 => CiProbe::Red {
            pr_number: change as u32,
            failed_summary: summary,
        },
        Ok(_) => CiProbe::NoSignal("forge returned no open PR".to_string()),
        Err(e) => CiProbe::NoSignal(format!("{e:#}")),
    }
}

// trace:BUG-1037 | ai:codex
pub(crate) fn ci_probe_with_forge(
    project_root: &std::path::Path,
    forge_kind: crate::forge::ForgeKind,
    branch: &str,
) -> CiProbe {
    ci_probe_from_ci_probe_result(
        crate::forge::forge_for_kind(project_root, forge_kind).ci_probe_for_branch(branch),
    )
}

/// STORY-516: forge-routed orchestrator / `--watch-ci` CI watch — blocks until
/// the branch's workflow-run CI is terminal (streaming when interactive),
/// returning CiProbe. GitHubForge delegates to `watch_ci_for_context`.
/// `no_human_active` is the headless flag (inverted to `interactive`).
///
/// TASK-1165: takes the caller's `project_root` instead of re-deriving one from
/// the process cwd. Both callers already hold it, and the old
/// `unwrap_or(PathBuf::from("."))` fallback could hand the forge — and through
/// it the `CiTerminal` emit — a root of `.`, dropping a stray
/// `./.aida/events.jsonl` outside any project.
// trace:STORY-516 trace:TASK-1165 | ai:claude
pub(crate) fn watch_ci_for_context_via_forge(
    project_root: &std::path::Path,
    branch: &str,
    no_human_active: bool,
) -> CiProbe {
    watch_ci_for_context(Some(project_root), branch, no_human_active)
}

// trace:BUG-1037 | ai:codex
pub(crate) fn watch_ci_for_context_with_forge(
    project_root: &std::path::Path,
    forge_kind: crate::forge::ForgeKind,
    branch: &str,
    no_human_active: bool,
) -> CiProbe {
    ci_probe_from_ci_probe_result(
        crate::forge::forge_for_kind(project_root, forge_kind)
            .stream_ci_for_branch(branch, !no_human_active),
    )
}

/// Pure JSON-to-CiProbe parser. Extracted so we can unit-test it without
/// running gh. The input shape is `[{"number": N, "statusCheckRollup": [...]}]`
/// (a JSON array of PR objects from gh; we only ever look at the first).
///
/// BUG-1455: a concluded failure is reported as terminal `Red` only once
/// every other check on the rollup has also concluded. `gh`'s rollup carries
/// no `isRequired` flag, so this function cannot tell a required check from
/// an optional one — but it can always tell "concluded" from "still
/// running", and a check still `IN_PROGRESS`/`QUEUED` might be the one that
/// actually decides code health (e.g. the build), even while a fast-failing
/// gate check (e.g. a supervised merge-hold marker check, which fails by
/// construction) has already concluded. Ending the wait on the gate's
/// conclusion alone let a caller declare CI red — and emit a terminal wake —
/// before the real build had even reported in. Waiting for every check to
/// settle before returning Red or Green is the conservative, always-safe
/// reading: it never reports a verdict while something is still pending or
/// unknown (PRIN-5), at the cost of not fast-failing on an unrelated
/// optional check while something else is still running.
// trace:BUG-1455 | ai:claude
pub(crate) fn parse_ci_probe(stdout: &str) -> CiProbe {
    let trimmed = stdout.trim();
    if trimmed.is_empty() || trimmed == "[]" {
        return CiProbe::NoSignal("no open PR for branch".to_string());
    }
    let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(e) => return CiProbe::NoSignal(format!("gh json parse: {e}")),
    };
    let pr = match parsed.get(0).cloned() {
        Some(v) => v,
        None => return CiProbe::NoSignal("gh returned empty array".to_string()),
    };
    let pr_number = pr.get("number").and_then(|n| n.as_u64()).map(|n| n as u32);
    let pr_number = match pr_number {
        Some(n) if n > 0 => n,
        Some(_) => return CiProbe::NoSignal("gh json PR number was 0".to_string()),
        None => return CiProbe::NoSignal("gh json missing PR number".to_string()),
    };
    let raw_rollup = pr.get("statusCheckRollup").and_then(|v| v.as_array());
    let raw_rollup = match raw_rollup {
        Some(r) if !r.is_empty() => r,
        _ => return CiProbe::PrNoChecks { pr_number },
    };
    // TASK-1424 safety net: the GitLab-mirror-link status is informational
    // only (it always posts `success` — see
    // `gitlab_mirror_link::post_github_mirror_status` — so a real
    // failure/pending mirror entry should be unreachable in practice), but
    // excluding its context here too means a future change to that
    // invariant still can't shelve or stall a drain on GitLab-mirror
    // evidence, which is advisory, not a gate. Filtered out BEFORE the
    // empty-rollup check below (not skipped mid-loop): a PR carrying only
    // the mirror status must read as "no checks yet" (`PrNoChecks`), not as
    // a vacuously passing `Green` from an empty tally. Checked by context
    // (StatusContext shape) or name (CheckRun shape) — whichever the rollup
    // entry carries.
    // trace:TASK-1424 | ai:claude
    let rollup: Vec<&serde_json::Value> = raw_rollup
        .iter()
        .filter(|check| {
            let check_id = check
                .get("context")
                .and_then(|v| v.as_str())
                .or_else(|| check.get("name").and_then(|v| v.as_str()));
            check_id != Some(crate::gitlab_mirror_link::MIRROR_STATUS_CONTEXT)
        })
        .collect();
    if rollup.is_empty() {
        return CiProbe::PrNoChecks { pr_number };
    }
    // Tally check states. statusCheckRollup entries can be from
    // CheckRun (status=COMPLETED|IN_PROGRESS|QUEUED, conclusion=SUCCESS|FAILURE|...)
    // or StatusContext (state=SUCCESS|FAILURE|PENDING|ERROR). Handle both.
    let mut any_in_progress = false;
    let mut failed: Vec<String> = Vec::new();
    for check in rollup {
        // CheckRun shape
        if let Some(status) = check.get("status").and_then(|v| v.as_str()) {
            let conclusion = check
                .get("conclusion")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match status {
                "IN_PROGRESS" | "QUEUED" | "PENDING" | "WAITING" => any_in_progress = true,
                "COMPLETED" => {
                    if matches!(
                        conclusion,
                        "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STALE"
                    ) {
                        let name = check
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("check");
                        failed.push(name.to_string());
                    }
                }
                _ => {}
            }
        } else if let Some(state) = check.get("state").and_then(|v| v.as_str()) {
            // StatusContext shape
            match state {
                "PENDING" | "EXPECTED" => any_in_progress = true,
                "FAILURE" | "ERROR" => {
                    let name = check
                        .get("context")
                        .and_then(|v| v.as_str())
                        .unwrap_or("status");
                    failed.push(name.to_string());
                }
                _ => {}
            }
        }
    }
    // BUG-1455: a check still running always keeps the verdict open, even
    // when another check has already concluded a failure — a partial
    // rollup must never be read as terminal. Only once nothing is left
    // running do concluded failures decide Red vs Green.
    if any_in_progress {
        return CiProbe::InProgress { pr_number };
    }
    if !failed.is_empty() {
        let summary = if failed.len() <= 3 {
            failed.join(", ")
        } else {
            format!("{} and {} more", failed[..3].join(", "), failed.len() - 3)
        };
        return CiProbe::Red {
            pr_number,
            failed_summary: summary,
        };
    }
    CiProbe::Green { pr_number }
}

/// TASK-1453: at the CI wait's absolute ceiling, decide whether a stuck-forever
/// pending check should still be reported as a known `Red` rather than the
/// uninformative `NoSignal` the wait falls back to today. Parses the same
/// `gh pr list --json number,statusCheckRollup,headRefOid` rollup shape
/// `parse_ci_probe` consumes (it's the exact string `ci_progress_snapshot`
/// already fetched this poll for the idle fingerprint — no extra probe).
///
/// Returns `Some(Red)`, naming the concluded failure(s) AND listing the
/// still-pending check(s), only when at least one check has genuinely
/// concluded a failure. A rollup with nothing but pending/queued checks (the
/// honest "we truly don't know yet" case) returns `None` so the caller keeps
/// reporting `NoSignal` — stuck-pending alone must never be read as Red.
/// Pure + unit-tested; degrades to `None` on unparsable/foreign-shaped JSON
/// (e.g. a non-GitHub forge's progress snapshot), which is the safe fallback.
// trace:TASK-1453 | ai:claude
pub(crate) fn ci_ceiling_verdict_from_rollup(rollup_json: &str) -> Option<CiProbe> {
    let trimmed = rollup_json.trim();
    if trimmed.is_empty() || trimmed == "[]" {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let pr = parsed.get(0)?;
    let pr_number = pr
        .get("number")
        .and_then(|n| n.as_u64())
        .filter(|n| *n > 0)? as u32;
    let rollup = pr.get("statusCheckRollup").and_then(|v| v.as_array())?;
    let mut failed: Vec<String> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for check in rollup {
        if let Some(status) = check.get("status").and_then(|v| v.as_str()) {
            let conclusion = check
                .get("conclusion")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = check
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("check");
            match status {
                "IN_PROGRESS" | "QUEUED" | "PENDING" | "WAITING" => pending.push(name.to_string()),
                "COMPLETED" => {
                    if matches!(
                        conclusion,
                        "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STALE"
                    ) {
                        failed.push(name.to_string());
                    }
                }
                _ => {}
            }
        } else if let Some(state) = check.get("state").and_then(|v| v.as_str()) {
            let name = check
                .get("context")
                .and_then(|v| v.as_str())
                .unwrap_or("status");
            match state {
                "PENDING" | "EXPECTED" => pending.push(name.to_string()),
                "FAILURE" | "ERROR" => failed.push(name.to_string()),
                _ => {}
            }
        }
    }
    // Review fix: the merge-hold gate fails BY CONSTRUCTION while a hold is
    // active; it is never a real CI failure. At the ceiling it must not turn a
    // held PR with a stuck check into ci-red. trace:TASK-1453 | ai:claude
    failed.retain(|name| !name.eq_ignore_ascii_case(crate::ci_gate::HOLD_GATE_CHECK));
    if failed.is_empty() {
        return None;
    }
    let failed_summary = if failed.len() <= 3 {
        failed.join(", ")
    } else {
        format!("{} and {} more", failed[..3].join(", "), failed.len() - 3)
    };
    let summary = if pending.is_empty() {
        failed_summary
    } else {
        format!("{failed_summary} (still pending: {})", pending.join(", "))
    };
    Some(CiProbe::Red {
        pr_number,
        failed_summary: summary,
    })
}

/// Block until the branch's CI run reaches a terminal state. Polls every 30s.
///
/// TASK-968: the wait runs an IDLE timeout (re-arms on progress) with a SEPARATE
/// absolute ceiling, instead of the old flat absolute timeout. The idle deadline
/// resets whenever CI posts progress (a check appears/transitions, the check set
/// changes) OR the PR head / base tip advances (the PR got rebased), so a
/// legitimately-slow-but-moving CI keeps its monitor; only a genuine STALL dies
/// after the idle window. The absolute ceiling still bounds a forever-progressing
/// wait so it can't run unbounded. Returns NoSignal on either timeout so the
/// drain gate can shelve rather than hanging or proceeding unchecked. Ctrl+C
/// interrupts cleanly.
/// Windows: `AIDA_WORKER_CI_IDLE` / `[drain] ci_idle` (default 1200s) and
/// `AIDA_WORKER_CI_ABSOLUTE` / `[drain] ci_absolute` (default 5400s).
///
/// TASK-1165: `project_root` is INJECTED by the caller rather than resolved
/// here from the process cwd — the same rule BUG-770 established for the
/// escalation epilogue. Every caller already holds the root it is driving
/// (`session_end`'s `project_root`, `RealPhaseDriver`'s `self.project_root`,
/// `GitHubForge`'s `self.project_root`), so nothing has to guess. `None` skips
/// the terminal `CiTerminal` emit entirely instead of falling back to `.` and
/// dropping a stray `./.aida/events.jsonl` outside any project; the git probes
/// below still degrade to the cwd, which is what they always did.
// trace:TASK-111 trace:TASK-968 trace:TASK-1165 | ai:claude
pub(crate) fn wait_for_ci_terminal(
    project_root: Option<&std::path::Path>,
    branch: &str,
) -> CiProbe {
    // The git probes are read-only and harmless against the cwd; only the emit
    // is root-sensitive, so only the emit is gated on `Some`.
    let git_root = project_root.unwrap_or_else(|| std::path::Path::new("."));
    use crate::ci_idle_timeout::{
        ci_observation_rearms_idle, ci_progress_fingerprint, ci_wait_verdict, CiWaitVerdict,
    };
    const POLL_INTERVAL_SECS: u64 = 30;
    let idle_window = crate::ci_idle_timeout::ci_idle_window_secs(git_root);
    let absolute_ceiling = crate::ci_idle_timeout::ci_absolute_ceiling_secs(git_root);
    let default_ref = detect_default_branch_ref(git_root);

    let started = std::time::Instant::now();
    let mut last_progress = started;
    let mut last_fingerprint: Option<String> = None;
    let retry_config = crate::network_retry::RetryConfig::load(git_root);
    let mut consecutive_probe_failures = 0_u32;

    loop {
        std::thread::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS));
        // Keep every poll on the caller-injected repository. Re-discovering from
        // the process cwd can silently switch a GitLab wait to GitHub when the
        // driven worktree is not the agent's cwd. trace:TASK-1273 | ai:codex
        let forge_kind = crate::forge::resolve_forge_kind(git_root);
        let probe = ci_probe_with_forge(git_root, forge_kind, branch);
        let total_elapsed = started.elapsed().as_secs();

        // Progress detection: re-arm the idle deadline whenever the CI check set
        // transitions/appears OR the PR head / base tip advances (a rebase).
        let base_tip = default_ref
            .as_deref()
            .and_then(|r| git_rev_parse_quiet(git_root, r));
        // STORY-1166: the idle-progress fingerprint source is forge-routed —
        // GitHub check rollup, GitLab pipelines listing; pure-git has none.
        // trace:STORY-1166 | ai:claude
        let rollup = crate::forge::forge_for(git_root).ci_progress_snapshot(branch);
        let fingerprint = ci_progress_fingerprint(&rollup, base_tip.as_deref());
        let progressed = match &last_fingerprint {
            // The first observation is the baseline, not progress.
            Some(prev) => *prev != fingerprint,
            None => false,
        };
        // BUG-1275: a required check that is still queued/running proves the
        // wait is healthy even if GitHub's rollup is byte-for-byte unchanged.
        // Absence/NoSignal does not re-arm, preserving genuine-stall behavior.
        if progressed || ci_observation_rearms_idle(matches!(probe, CiProbe::InProgress { .. })) {
            last_progress = std::time::Instant::now();
        }
        last_fingerprint = Some(fingerprint);
        let idle_elapsed = last_progress.elapsed().as_secs();

        match &probe {
            CiProbe::NoSignal(reason) => {
                consecutive_probe_failures += 1;
                match decide_ci_probe_failure(
                    reason,
                    consecutive_probe_failures,
                    retry_config.max_attempts,
                    &retry_config.transient_patterns,
                ) {
                    CiProbeFailureAction::Retry => {
                        let backoff = retry_config.base_delay.saturating_mul(
                            retry_config
                                .factor
                                .saturating_pow(consecutive_probe_failures - 1),
                        );
                        eprintln!(
                            "  ↻ CI probe unavailable (attempt {}/{}) — retrying in {}ms: {}",
                            consecutive_probe_failures,
                            retry_config.max_attempts,
                            backoff.as_millis(),
                            reason
                        );
                        std::thread::sleep(backoff);
                        continue;
                    }
                    CiProbeFailureAction::Unavailable => return probe,
                }
            }
            CiProbe::InProgress { pr_number } => {
                consecutive_probe_failures = 0;
                match ci_wait_verdict(total_elapsed, idle_elapsed, idle_window, absolute_ceiling) {
                    CiWaitVerdict::Continue => {
                        eprintln!(
                            "  {} CI still running on PR-{} ({}m {}s elapsed, {}s since progress)",
                            crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
                            pr_number,
                            total_elapsed / 60,
                            total_elapsed % 60,
                            idle_elapsed,
                        );
                    }
                    CiWaitVerdict::IdleTimeout => {
                        return CiProbe::NoSignal(format!(
                            "CI idle for {idle_window}s with no progress — giving up"
                        ));
                    }
                    CiWaitVerdict::AbsoluteTimeout => {
                        // TASK-1453: a check stuck pending forever must not
                        // swallow a failure another check already concluded —
                        // report the known Red (stuck check listed as still
                        // pending) instead of the uninformative NoSignal.
                        // Stuck-pending alone (no concluded failure) keeps the
                        // honest NoSignal below.
                        if let Some(red) = ci_ceiling_verdict_from_rollup(&rollup) {
                            eprintln!(
                                "  {} CI wait hit absolute ceiling ({}m) with a known failure — reporting Red",
                                crate::glyph(crate::glyphs::Glyph::Cross).red(),
                                absolute_ceiling / 60,
                            );
                            emit_ci_terminal(project_root, false);
                            return red;
                        }
                        return CiProbe::NoSignal(format!(
                            "CI wait hit absolute ceiling ({}m) — giving up",
                            absolute_ceiling / 60
                        ));
                    }
                }
            }
            // Any non-InProgress probe is the terminal CI verdict.
            // STORY-712: emit the actionable CiTerminal wake before returning.
            // Best-effort, no control-flow change. trace:TASK-988 | ai:claude
            _ => {
                emit_ci_terminal(project_root, matches!(probe, CiProbe::Green { .. }));
                return probe;
            }
        }
    }
}

/// STORY-712: emit the actionable `CiTerminal` wake once a CI watch reaches a
/// terminal verdict. Best-effort, no control-flow effect.
///
/// TASK-1165: the root is INJECTED (see [`wait_for_ci_terminal`]). `None` — a
/// caller with no resolvable project — skips the emit rather than writing a
/// stray `./.aida/events.jsonl` into whatever directory the process happened to
/// be standing in. Split out of the poll loop so both arms of that decision are
/// unit-testable without a 30s `gh` poll.
// trace:TASK-1165 | ai:claude
pub(crate) fn emit_ci_terminal(project_root: Option<&std::path::Path>, green: bool) {
    let Some(root) = project_root else {
        return;
    };
    let (spec, run_uuid) = drain_state::current_context(root);
    events::emit(
        root,
        &events::Event::new(spec, run_uuid, events::EventKind::CiTerminal { green }),
    );
}

/// TASK-968: fetch the raw `statusCheckRollup` (+ `headRefOid`) JSON for the
/// branch's open PR, for the idle-timeout progress fingerprint. Mirrors
/// `probe_ci_state_for_branch`'s `gh pr list` call but returns the raw stdout
/// instead of a parsed verdict. Empty string on any failure (gh missing, no PR,
/// network blip) — the fingerprint then degrades to the base tip only, which
/// still re-arms on a rebase.
// trace:TASK-968 | ai:claude
pub(crate) fn ci_rollup_json_for_branch(branch: &str) -> String {
    let Some(gh) = resolve_forge_cli(crate::forge::ForgeKind::GitHub) else {
        return String::new();
    };
    let output = std::process::Command::new(&gh)
        .args([
            "pr",
            "list",
            "--head",
            branch,
            "--state",
            "open",
            "--json",
            "number,statusCheckRollup,headRefOid",
            "--limit",
            "1",
        ])
        .output_retrying_etxtbsy();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => String::new(),
    }
}

/// TASK-968: resolve a ref to its commit SHA via `git rev-parse`, quietly.
/// `None` on any failure — the caller folds it into the progress fingerprint,
/// so a missing base tip just drops that one progress signal.
// trace:TASK-968
pub(crate) fn git_rev_parse_quiet(project_root: &std::path::Path, refname: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            git_arg_guard::END_OF_OPTIONS,
            refname,
        ]) // trace:BUG-1622 | ai:claude
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// BUG-273: `gh run watch` is an interactive terminal renderer. When stdout
/// is piped to tee, or when an auto-complete drain is explicitly headless, its
/// redraw frames become hundreds of repeated log blocks. Stream only for the
/// true interactive case; otherwise use the quiet poller.
/// trace:BUG-273
pub(crate) fn should_stream_ci_watch(stdout_is_tty: bool, no_human_active: bool) -> bool {
    stdout_is_tty && !no_human_active
}

/// TASK-1165: `project_root` is threaded straight through to the emit site in
/// [`wait_for_ci_terminal`] — this layer only picks stream-vs-poll.
// trace:TASK-1165 | ai:claude
pub(crate) fn watch_ci_for_context(
    project_root: Option<&std::path::Path>,
    branch: &str,
    no_human_active: bool,
) -> CiProbe {
    if let Some(project_root) = project_root {
        let forge_kind = crate::forge::resolve_forge_kind(project_root);
        return ci_probe_from_ci_probe_result(
            crate::forge::forge_for_kind(project_root, forge_kind)
                .stream_ci_for_branch(branch, !no_human_active),
        );
    }
    watch_ci_for_context_github(project_root, branch, no_human_active)
}

// STORY-1163: raw GitHub watcher used by GitHubForge after the public helper
// became forge-dispatched. Keeping the old implementation here preserves the
// GitHub stream/poll behavior while allowing GitLab callers to route to glab.
// trace:STORY-1163 | ai:codex
pub(crate) fn watch_ci_for_context_github(
    project_root: Option<&std::path::Path>,
    branch: &str,
    no_human_active: bool,
) -> CiProbe {
    if should_stream_ci_watch(std::io::stdout().is_terminal(), no_human_active) {
        watch_ci_terminal(project_root, branch)
    } else {
        eprintln!(
            "  {} stdout is non-interactive or headless mode is active — using quiet CI polling.",
            crate::glyph(crate::glyphs::Glyph::Info).cyan()
        );
        wait_for_ci_terminal(project_root, branch)
    }
}

/// TASK-233: extract the most-recent workflow run id from `gh run list
/// --json databaseId` JSON output. Pure — unit-testable independent of
/// the `gh` subprocess. trace:TASK-233 | ai:claude
pub(crate) fn first_run_id_from_gh_json(json: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(json).ok()?;
    let id = parsed.as_array()?.first()?.get("databaseId")?;
    id.as_u64().map(|n| n.to_string())
}

#[cfg(test)]
#[path = "tests/task1165_events_root_tests.rs"]
mod task1165_events_root_tests;

/// TASK-233: `--watch-ci` — stream live CI progress via `gh run watch`
/// rather than silently polling, then re-probe for the terminal state so
/// the end-session decision tree (green proceeds / red prompts) runs
/// exactly as it does for `--wait-ci`. Falls back to the silent poll
/// loop when `gh` is missing or no run id resolves. trace:TASK-233
// trace:TASK-1165 | ai:claude
pub(crate) fn watch_ci_terminal(project_root: Option<&std::path::Path>, branch: &str) -> CiProbe {
    let Some(gh) = resolve_forge_cli(crate::forge::ForgeKind::GitHub) else {
        return wait_for_ci_terminal(project_root, branch);
    };
    // Resolve the latest workflow run id for this branch.
    let run_id = std::process::Command::new(&gh)
        .args([
            "run",
            "list",
            "--branch",
            branch,
            "--limit",
            "1",
            "--json",
            "databaseId",
        ])
        .output_retrying_etxtbsy()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| first_run_id_from_gh_json(&String::from_utf8_lossy(&o.stdout)));
    let Some(run_id) = run_id else {
        // No resolvable run — degrade to the silent poll loop rather
        // than failing the session-end.
        return wait_for_ci_terminal(project_root, branch);
    };
    eprintln!(
        "  {} streaming `gh run watch {}`",
        crate::glyph(crate::glyphs::Glyph::FlowActive).cyan(),
        run_id.dimmed()
    );
    // Inherit stdio so gh's live-updating display renders straight to
    // the user's terminal. `gh run watch` returns when the run reaches
    // a terminal state.
    let _ = std::process::Command::new(&gh)
        .args(["run", "watch", &run_id])
        .status_retrying_etxtbsy();
    // Re-probe to classify Green/Red for the end-session decision tree.
    // Keep the re-probe on the caller-injected root. Re-deriving from the
    // process cwd can silently route a GitLab branch through GitHub when the
    // driven worktree is not the agent's cwd. Only the `None` arm — where the
    // caller supplied no root — keeps the historical cwd discovery.
    // trace:TASK-1273 | ai:claude
    let probe_root = project_root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| find_project_root().unwrap_or_else(|_| std::path::PathBuf::from(".")));
    let forge_kind = crate::forge::resolve_forge_kind(&probe_root);
    ci_probe_with_forge(&probe_root, forge_kind, branch) // STORY-516: forge-routed
}

/// trace:STORY-73 | ai:claude
/// TASK-243: format the `Claude session: <id>… (last active …)` line for
/// `aida session end`'s ambiguity prompt. Pure — `age_secs` is resolved
/// by the caller. trace:TASK-243 | ai:claude
pub(crate) fn format_claude_session_line(claude_id: &str, age_secs: Option<u64>) -> String {
    let short: String = claude_id.chars().take(8).collect();
    match age_secs {
        Some(s) => format!(
            "Claude session: {}… (last active {} ago)",
            short,
            humanize_age_secs(s)
        ),
        None => format!("Claude session: {}…", short),
    }
}

/// TASK-243: one-line ambiguity-resolution summary for `aida session
/// end` — `(headline, optional Claude-session line)`. Pure formatter so
/// the role / Claude-id surfacing is unit-testable independent of the
/// interactive prompt. trace:TASK-243 | ai:claude
pub(crate) fn format_session_end_summary(
    id: &str,
    role: Option<&str>,
    scope: &str,
    branch: &str,
    claude_id: Option<&str>,
    age_secs: Option<u64>,
) -> (String, Option<String>) {
    let headline = format!(
        "{} (role:{}, scope {}, branch {})",
        &id[..id.len().min(8)],
        role.unwrap_or("unset"),
        scope,
        branch
    );
    let claude_line = claude_id.map(|cid| format_claude_session_line(cid, age_secs));
    (headline, claude_line)
}

/// TASK-243: resolve the Claude conversation linked to a lease — the
/// `claude_session_id` recorded in the session manifest (TASK-112) plus
/// a best-effort "last active" age from the conversation JSONL's mtime.
/// Returns `(None, None)` when there's no manifest or no recorded id.
/// trace:TASK-243 | ai:claude
pub(crate) fn resolve_lease_claude_session(lease: &SessionLease) -> (Option<String>, Option<u64>) {
    let project_root = find_main_worktree_root()
        .ok()
        .or_else(|| lease.parent_project_root.clone());
    let Some(root) = project_root else {
        return (None, None);
    };
    let manifest_path = session_manifest::manifest_path(&root, &lease.id);
    let Some(claude_id) = session_manifest::load(&manifest_path)
        .ok()
        .and_then(|m| m.claude_session_id)
    else {
        return (None, None);
    };
    let age_secs = session::claude_project_dir(&lease.worktree_path)
        .ok()
        .map(|dir| dir.join(format!("{}.jsonl", claude_id)))
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs());
    (Some(claude_id), age_secs)
}

pub(crate) fn resolve_session_to_end(
    id_query: Option<&str>,
    spec_query: Option<&str>,
    branch_query: Option<&str>,
    leases: &[SessionLease],
    yes: bool,
) -> Result<SessionLease> {
    // TASK-489: explicit --spec / --branch take precedence over the
    // positional id resolution. Both are scoped lookups against the
    // lease list rather than the id-prefix lookup.
    // trace:TASK-489 | ai:claude
    if let Some(q) = spec_query {
        return find_lease_by_spec(q, leases);
    }
    if let Some(q) = branch_query {
        return find_lease_by_branch(q, leases);
    }

    if let Some(q) = id_query {
        // TASK-489: if the positional looks like a SPEC-ID (uppercase
        // alpha-dash-digits — `TASK-489`, `STORY-86`, `FR-1-001`), the
        // user almost certainly means the spec, not the lease id. Skip
        // the hex-prefix path and route to the spec lookup so a 0/many
        // mismatch surfaces a spec-shaped error instead of "no session
        // matching `TASK-489`". The shared `looks_like_spec_id` accepts
        // lowercase too — `task-489` should route to the branch path —
        // so apply an extra uppercase-prefix guard here.
        // trace:TASK-489 | ai:claude
        if looks_like_spec_id(q) && positional_has_uppercase_spec_prefix(q) {
            return find_lease_by_spec(q, leases);
        }
        // Otherwise, prefer the existing 8-char lease-id-prefix lookup
        // (back-compat), and fall back to branch-name lookup on miss.
        // The branch fallback lets `aida session end task-489` resolve
        // without the user reaching for `--branch`. When both miss we
        // surface the branch error — it points the user at
        // `aida session leases`, the right next step regardless of
        // whether the user thought they were typing an id or a branch.
        // trace:TASK-489 | ai:claude
        return match find_lease_by_id_prefix(q, leases) {
            Ok(l) => Ok(l),
            Err(id_err) => {
                // An "ambiguous id" miss is a real prefix collision —
                // surface that error so the operator picks a longer
                // prefix. Only treat a "no session matching" miss as a
                // signal to fall through to branch resolution.
                let msg = id_err.to_string();
                // prose-ok: classifies our own find_lease_by_id_prefix error (ambiguous-prefix vs no-match), not external text
                if msg.contains("ambiguous") {
                    return Err(id_err);
                }
                find_lease_by_branch(q, leases)
            }
        };
    }

    // 1. cwd
    if let Ok(cwd) = std::env::current_dir() {
        let canon = cwd.canonicalize().unwrap_or(cwd);
        if let Some(l) = leases.iter().find(|&l| lease_covers_cwd(l, &canon)) {
            return Ok(l.clone());
        }
    }

    // 2. AIDA_SESSION_ID env var
    if let Ok(env_id) = std::env::var("AIDA_SESSION_ID") {
        if !env_id.trim().is_empty() {
            if let Ok(l) = find_lease_by_id_prefix(&env_id, leases) {
                eprintln!(
                    "{} resolved session via {} env var",
                    "→".dimmed(),
                    "AIDA_SESSION_ID".cyan()
                );
                return Ok(l);
            }
        }
    }

    // 3. ancestor PID
    let my_chain = process_probe::walk_ancestor_pids(std::process::id());
    let chain_set: std::collections::HashSet<u32> = my_chain.into_iter().collect();
    let ancestor_match: Vec<&SessionLease> = leases
        .iter()
        .filter(|l| {
            l.creator_pid
                .map(|p| chain_set.contains(&p))
                .unwrap_or(false)
        })
        .collect();
    match ancestor_match.len() {
        0 => {}
        1 => {
            eprintln!(
                "{} resolved session via ancestor PID match (creator_pid {})",
                "→".dimmed(),
                ancestor_match[0].creator_pid.unwrap()
            );
            return Ok(ancestor_match[0].clone());
        }
        n => {
            eprintln!(
                "{} {} active leases all have an ancestor PID match — pass an id explicitly",
                "Warning:".yellow().bold(),
                n
            );
        }
    }

    // 4. single-active fallback
    if leases.len() == 1 {
        let only = &leases[0];
        // TASK-243: surface role + the linked Claude session so the
        // user can tell which session they're about to end without a
        // separate `aida session leases` cross-reference.
        let (claude_id, age_secs) = resolve_lease_claude_session(only);
        let (headline, claude_line) = format_session_end_summary(
            &only.id,
            only.role.as_deref(),
            &only.scope,
            &only.branch,
            claude_id.as_deref(),
            age_secs,
        );
        if yes {
            eprintln!(
                "{} only one active session ({}); -y given, ending it",
                "→".dimmed(),
                headline
            );
            return Ok(only.clone());
        }
        eprintln!("Only one active session: {}", headline);
        if let Some(line) = &claude_line {
            eprintln!("  {}", line.dimmed());
        }
        eprint!("End it? [y/N] ");
        use std::io::Write;
        let _ = std::io::stderr().flush();
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            return Ok(only.clone());
        }
        anyhow::bail!("aborted by user");
    }

    // 5. error with listing
    let mut msg = String::from(
        "no active session resolvable from this shell — pass an id or `aida session leases`\n\nActive sessions:",
    );
    for l in leases {
        // TASK-243: include role here too so the multi-session listing
        // is consistent with the single-active prompt and `aida session
        // leases`. trace:TASK-243 | ai:claude
        msg.push_str(&format!(
            "\n  {} role:{} {} ({})",
            &l.id[..8],
            l.role.as_deref().unwrap_or("unset"),
            l.scope,
            l.worktree_path.display()
        ));
    }
    anyhow::bail!(msg)
}

#[cfg(test)]
#[path = "tests/session_end_summary_tests.rs"]
mod session_end_summary_tests;

/// Look up a lease by id prefix (case-insensitive). Errors on no-match
/// or ambiguous prefix. trace:STORY-73 | ai:claude
pub(crate) fn find_lease_by_id_prefix(
    query: &str,
    leases: &[SessionLease],
) -> Result<SessionLease> {
    let q = query.to_lowercase();
    let matches: Vec<&SessionLease> = leases
        .iter()
        .filter(|l| l.id.to_lowercase().starts_with(&q))
        .collect();
    match matches.len() {
        0 => anyhow::bail!("no session matching `{}`", query),
        1 => Ok(matches[0].clone()),
        n => {
            // BUG-312: an honest "use a longer prefix" left the operator
            // grepping `.aida/sessions/` to find HOW long was enough. List
            // every match's full id + scope so the operator can pick.
            // trace:BUG-312 | ai:claude
            let listing: String = matches
                .iter()
                .map(|l| format!("  {}  ({})", l.id, l.scope))
                .collect::<Vec<_>>()
                .join("\n");
            anyhow::bail!(
                "ambiguous session id `{}` — matches {} sessions:\n{}",
                query,
                n,
                listing
            )
        }
    }
}

/// TASK-489: is the alpha prefix of `s` (the chars before the first `-`)
/// all uppercase? Used to disambiguate the positional resolution between
/// SPEC-ID (`TASK-489` → spec lookup) and branch name (`task-489` →
/// branch lookup) when the shared `looks_like_spec_id` would accept both.
/// trace:TASK-489 | ai:claude
pub(crate) fn positional_has_uppercase_spec_prefix(s: &str) -> bool {
    let bytes = s.trim().as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        if !bytes[i].is_ascii_uppercase() {
            return false;
        }
        i += 1;
    }
    i > 0
}

/// TASK-489: look up a lease by spec ID (case-insensitive equality against
/// `lease.scope`). Errors with the message shape the user friction asked
/// for: zero matches surfaces `aida session leases` as the diagnostic;
/// many matches lists every candidate plus its lease id so the operator
/// can disambiguate. The lease's `scope` field holds the raw `--owns`
/// argument from session start — for normal pickups that's the SPEC-ID,
/// matching the operator's mental model exactly.
/// trace:TASK-489 | ai:claude
pub(crate) fn find_lease_by_spec(query: &str, leases: &[SessionLease]) -> Result<SessionLease> {
    let q = query.trim().to_lowercase();
    let matches: Vec<&SessionLease> = leases
        .iter()
        .filter(|l| l.scope.to_lowercase() == q)
        .collect();
    match matches.len() {
        0 => anyhow::bail!(
            "No lease found for spec `{}` — run `aida session leases` to see active leases",
            query
        ),
        1 => Ok(matches[0].clone()),
        n => {
            let listing: String = matches
                .iter()
                .map(|l| format!("  {}  scope:{}  branch:{}", l.id, l.scope, l.branch))
                .collect::<Vec<_>>()
                .join("\n");
            anyhow::bail!(
                "{} leases own spec `{}` — disambiguate via lease id:\n{}",
                n,
                query,
                listing
            )
        }
    }
}

/// TASK-489: look up a lease by branch name (case-sensitive — git refs
/// are case-sensitive). Same error shape as `find_lease_by_spec`.
/// trace:TASK-489 | ai:claude
pub(crate) fn find_lease_by_branch(query: &str, leases: &[SessionLease]) -> Result<SessionLease> {
    let q = query.trim();
    let matches: Vec<&SessionLease> = leases.iter().filter(|l| l.branch == q).collect();
    match matches.len() {
        0 => anyhow::bail!(
            "No lease found for branch `{}` — run `aida session leases` to see active leases",
            query
        ),
        1 => Ok(matches[0].clone()),
        n => {
            let listing: String = matches
                .iter()
                .map(|l| format!("  {}  scope:{}  branch:{}", l.id, l.scope, l.branch))
                .collect::<Vec<_>>()
                .join("\n");
            anyhow::bail!(
                "{} leases ride branch `{}` — disambiguate via --spec or lease id:\n{}",
                n,
                query,
                listing
            )
        }
    }
}

/// BUG-312: shortest prefix of `target_id` that does not collide with any
/// other id in `all_ids`, floored at `min_len`. Used by `aida session
/// leases` to render an id that the operator can paste straight back into
/// `aida session end`. HLC-derived UUIDs put the timestamp at the start,
/// so two leases created in the same generation window collide on the
/// historical 8-char prefix; this function bumps just those colliding
/// rows wider while leaving solitary ids at the short form.
/// trace:BUG-312 | ai:claude
pub(crate) fn unique_prefix_len(target_id: &str, all_ids: &[&str], min_len: usize) -> usize {
    let max_len = target_id.len();
    let floor = min_len.min(max_len);
    for len in floor..=max_len {
        let prefix = &target_id[..len];
        let collides = all_ids
            .iter()
            .any(|other| *other != target_id && other.starts_with(prefix));
        if !collides {
            return len;
        }
    }
    max_len
}

/// STORY-52: locate the parent project's cargo `target/` directory so a
/// session worktree can reuse its build cache. Returns the canonicalized
/// path when `target/` exists (Rust project that has been built), `None`
/// otherwise. Pure-function over the filesystem so callers can test the
/// session-start flow with a temp dir.
/// trace:STORY-52 | ai:claude
pub(crate) fn detect_cargo_target_dir(
    project_root: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let target = project_root.join("target");
    if !target.is_dir() {
        return None;
    }
    Some(target.canonicalize().unwrap_or(target))
}

/// BUG-1783: Isolate a shared target dir to a worktree-specific path.
/// If the worktree path lacks a file name component, falls back to the parent
/// target dir with a warning so we don't silently collapse back into the
/// same collision hazard one directory deeper.
/// trace:BUG-1783 | ai:codex
pub(crate) fn isolate_cargo_target_dir(
    parent_target: &std::path::Path,
    worktree_path: &std::path::Path,
) -> std::path::PathBuf {
    use colored::Colorize;
    match worktree_path.file_name() {
        Some(name) if !name.is_empty() => parent_target.join("worktrees").join(name),
        _ => {
            eprintln!(
                "  {} worktree path {} lacks a valid dir name; falling back to unisolated parent target {}",
                "warning:".yellow(),
                worktree_path.display(),
                parent_target.display()
            );
            parent_target.to_path_buf()
        }
    }
}

/// STORY-52: write the worktree-local `.aida/session-env.sh` that the user
/// sources after `cd`-ing into the session worktree. Sourcing it sets
/// `CARGO_TARGET_DIR` to the parent's `target/worktrees/<worktree-dir-name>` (BUG-1783) so
/// cargo reuses the build cache without colliding with main. The file is written
/// into the worktree's `.aida/` (created here if it doesn't already exist), which
/// lives alongside the symlinked runtime subdirs (sessions/, roles/,
/// cache.db, etc.) that `session_start` set up moments earlier.
/// trace:STORY-52 | ai:claude
/// trace:BUG-1783 | ai:codex
pub(crate) fn write_session_env_file(
    worktree_path: &std::path::Path,
    cargo_target_dir: &std::path::Path,
) -> Result<()> {
    let aida_dir = worktree_path.join(".aida");
    std::fs::create_dir_all(&aida_dir)?;
    let env_path = aida_dir.join("session-env.sh");
    let agent_type = std::env::var("AIDA_AGENT_TYPE")
        .ok()
        .filter(|s| !s.trim().is_empty());
    // BUG-766: bake the coordinating binary into the shim so sourcing it in
    // any shell resolves `aida` to the build that created this worktree,
    // never a stale installed binary earlier on the raw PATH.
    let aida_bin = resolve_aida_exe();
    let aida_bin = (aida_bin.is_absolute() && aida_bin.exists()).then_some(aida_bin);
    let body =
        render_session_env_file(cargo_target_dir, agent_type.as_deref(), aida_bin.as_deref());
    std::fs::write(&env_path, body)?;
    Ok(())
}

/// STORY-52: build the body of `.aida/session-env.sh`. Split out so unit
/// tests can assert the export shape without touching the filesystem.
/// trace:STORY-52 | ai:claude
pub(crate) fn render_session_env_file(
    cargo_target_dir: &std::path::Path,
    agent_type: Option<&str>,
    aida_bin: Option<&std::path::Path>,
) -> String {
    let mut body = format!(
        "# Generated by `aida session start` — source after cd-ing into\n\
         # this worktree to share the parent project's cargo build cache.\n\
         # trace:STORY-52 | ai:claude\n\
         export CARGO_TARGET_DIR={}\n",
        shell_single_quote(&cargo_target_dir.display().to_string())
    );
    if let Some(agent_type) = agent_type {
        body.push_str(&format!(
            "export AIDA_AGENT_TYPE={}\n",
            shell_single_quote(agent_type)
        ));
    }
    // BUG-766: pin the coordinating binary. The PATH line is a plain
    // assignment (PATH is already exported in any shell) so the narrow
    // `export VAR='value'` parser in `parse_session_env` skips it instead
    // of mangling the `"$PATH"` reference; the in-process launch path gets
    // the same prepend from `export_coordinating_bin_env` at startup.
    if let Some(bin) = aida_bin {
        body.push_str(&format!(
            "export AIDA_BIN={}\n",
            shell_single_quote(&bin.display().to_string())
        ));
        if let Some(dir) = bin.parent() {
            body.push_str(&format!(
                "PATH={}:\"$PATH\"\n",
                shell_single_quote(&dir.display().to_string())
            ));
        }
    }
    body
}

/// TASK-63: parse a `.aida/session-env.sh` body into `(name, value)`
/// pairs. Pure: no env mutation, so unit tests can exercise the parser
/// without racing the live process env across parallel test threads.
///
/// Parser is intentionally narrow: it understands only the shape that
/// `render_session_env_file` writes — `export VAR='single-quoted-value'`
/// with `'\''` close-reopen escaping. Lines we don't recognize are
/// skipped silently, so a manually-edited shim with extra noise doesn't
/// poison the env. trace:TASK-63 | ai:claude
pub(crate) fn parse_session_env(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix("export ") else {
            continue;
        };
        let Some((name, raw_value)) = rest.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let mut chars = name.chars();
        let first = chars.next();
        let valid_first = matches!(first, Some(c) if c.is_ascii_alphabetic() || c == '_');
        let valid_rest = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid_first || !valid_rest {
            continue;
        }
        let value = unquote_shell_single_quoted(raw_value.trim());
        out.push((name.to_string(), value));
    }
    out
}

// trace:TASK-1577 | ai:codex
pub(crate) fn is_session_cargo_target_dir(
    target_dir: &std::path::Path,
    worktree_path: &std::path::Path,
) -> bool {
    let Some(worktree_name) = worktree_path.file_name() else {
        return false;
    };
    let mut components = target_dir.components().rev();
    matches!(components.next(), Some(std::path::Component::Normal(name)) if name == worktree_name)
        && matches!(components.next(), Some(std::path::Component::Normal(name)) if name == "worktrees")
}

// trace:TASK-1577 | ai:codex
pub(crate) fn session_cargo_target_dir(
    worktree_path: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let body = std::fs::read_to_string(worktree_path.join(".aida/session-env.sh")).ok()?;
    parse_session_env(&body)
        .into_iter()
        .find(|(name, _)| name == "CARGO_TARGET_DIR")
        .map(|(_, value)| std::path::PathBuf::from(value))
}

// trace:TASK-1577 | ai:codex
pub(crate) fn remove_session_cargo_target_dir(
    target_dir: &std::path::Path,
    worktree_path: &std::path::Path,
) {
    use colored::Colorize;
    if !is_session_cargo_target_dir(target_dir, worktree_path) || !target_dir.exists() {
        return;
    }
    if let Err(error) = std::fs::remove_dir_all(target_dir) {
        eprintln!(
            "  {} could not remove session cargo target {}: {}",
            "warning:".yellow(),
            target_dir.display(),
            error
        );
    }
}

/// TASK-63: apply parsed env pairs to the current process so the
/// subsequent `exec claude` inherits them. Returns the names that were
/// applied so the caller can echo them to the user. Calls
/// `std::env::set_var` for each pair — the wrapping `unsafe { }` is
/// forward-compatibility with Edition 2024 where `set_var` is marked
/// unsafe; safe here because `session_start --launch` is single-threaded
/// between parse and exec. trace:TASK-63 | ai:claude
pub(crate) fn apply_session_env_to_process(body: &str) -> Vec<String> {
    // Only the allowlisted names, never PATH / LD_PRELOAD / BASH_ENV from a
    // branch-committed file. trace:BUG-1624 | ai:claude
    let pairs = trusted_session_env(body, &resolve_aida_exe());
    let mut applied = Vec::with_capacity(pairs.len());
    for (name, value) in pairs {
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(&name, &value);
        }
        applied.push(name);
    }
    applied
}

/// The only names [`render_session_env_file`] writes, and so the only names
/// taken from a `.aida/session-env.sh`. The file lives in the worktree, where
/// a branch can commit its own copy, so anything else (`PATH`,
/// `PROMPT_COMMAND`, `BASH_ENV`, `LD_PRELOAD`, ...) is dropped.
// trace:BUG-1624 | ai:claude
pub(crate) const SESSION_ENV_NAMES: [&str; 3] = ["CARGO_TARGET_DIR", "AIDA_AGENT_TYPE", "AIDA_BIN"];

/// The trustworthy subset of a `.aida/session-env.sh` body: allowlisted
/// names only, `CARGO_TARGET_DIR` only when absolute, and `AIDA_BIN`
/// never taken from the file. When the file pins a binary, the pin is
/// replaced by `running_exe` (this process's own binary) if that is an
/// absolute, existing path, and dropped otherwise.
// trace:BUG-1624 | ai:claude
pub(crate) fn trusted_session_env(
    body: &str,
    running_exe: &std::path::Path,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (name, value) in parse_session_env(body) {
        if !SESSION_ENV_NAMES.contains(&name.as_str()) || out.iter().any(|(n, _)| *n == name) {
            continue;
        }
        // A NUL byte can't be carried by an env var (`set_var` panics), so a
        // value containing one is dropped. trace:BUG-1627 | ai:claude
        if value.contains('\0') {
            continue;
        }
        let value = match name.as_str() {
            "CARGO_TARGET_DIR" if !std::path::Path::new(&value).is_absolute() => continue,
            // Any non-empty AIDA_AGENT_TYPE turns on the advisor code-gate
            // agent carve-out and adds a type mailbox identity, and the session
            // lease does not record the type. So only a known agent type is
            // taken, in its canonical spelling. trace:BUG-1627 | ai:claude
            "AIDA_AGENT_TYPE" => match trusted_agent_type(&value) {
                Some(canonical) => canonical,
                None => continue,
            },
            "AIDA_BIN" => {
                if !(running_exe.is_absolute() && running_exe.is_file()) {
                    continue;
                }
                running_exe.display().to_string()
            }
            _ => value,
        };
        out.push((name, value));
    }
    out
}

/// The canonical spelling of a known agent type (`claude`, `codex`,
/// `antigravity`, `shell`, `web`), or `None` for anything
/// [`agent_registry::normalize_agent_type`] does not recognize.
// trace:BUG-1627 | ai:claude
pub(crate) fn trusted_agent_type(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let canonical = agent_registry::normalize_agent_type(raw.to_string());
    (canonical != "other").then_some(canonical)
}

/// Inverse of `shell_single_quote` for the narrow shape we write.
/// `'X'` → `X`. `'a'\''b'` → `a'b`. Bare (unquoted) values are returned
/// as-is — POSIX-y enough for the shim's purposes. trace:TASK-63 | ai:claude
pub(crate) fn unquote_shell_single_quoted(raw: &str) -> String {
    let bytes = raw.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'\'' || bytes[bytes.len() - 1] != b'\'' {
        return raw.to_string();
    }
    let inner = &raw[1..raw.len() - 1];
    inner.replace("'\\''", "'")
}

/// Wrap a string in POSIX single quotes for safe inclusion in shell source.
/// `'` inside the value is escaped via the standard `'\''` close-reopen trick.
/// trace:STORY-52 | ai:claude
pub(crate) fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Render an argv slice as a copy-pasteable shell command line: an
/// element with shell-special characters (whitespace, quotes, globs, …)
/// is wrapped in POSIX single quotes; a plain word is left bare so the
/// output stays readable. trace:BUG-225 | ai:claude
pub(crate) fn shell_join_display(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            let bare = !a.is_empty()
                && a.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_./=:-+@".contains(c));
            if bare {
                a.clone()
            } else {
                shell_single_quote(a)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionEndUnshippedWork {
    pub(crate) lease_id: String,
    pub(crate) scope: String,
    pub(crate) branch: String,
    pub(crate) commits_ahead: u32,
}

pub(crate) fn classify_session_end_unshipped_work(
    lease_id: &str,
    scope: &str,
    branch: &str,
    commits_ahead: Option<u32>,
    pr_state: workflow_hints::PrState,
) -> Option<SessionEndUnshippedWork> {
    let commits = commits_ahead.unwrap_or(0);
    if commits == 0 || !matches!(pr_state, workflow_hints::PrState::Absent) {
        return None;
    }
    Some(SessionEndUnshippedWork {
        lease_id: lease_id.to_string(),
        scope: scope.to_string(),
        branch: branch.to_string(),
        commits_ahead: commits,
    })
}

pub(crate) fn session_end_pr_state_for_scope_or_branch(
    project_root: &std::path::Path,
    scope: &str,
    branch: &str,
) -> workflow_hints::PrState {
    // trace:BUG-853 | ai:codex
    // Reviewer/fixup sessions may use a local alias branch like `pr-1648`
    // while the forge PR head is still the implementation branch. In that
    // shape, branch-keyed lookup misses; the PR/MR scope is the durable id.
    match change_lookup_for_branch(project_root, branch) {
        crate::forge::ChangeLookup::Found(c) => return workflow_hints::PrState::Open(c.id),
        crate::forge::ChangeLookup::NoChange => {}
        crate::forge::ChangeLookup::CliMissing
        | crate::forge::ChangeLookup::CliFailed(_)
        | crate::forge::ChangeLookup::Unreachable(_) => return workflow_hints::PrState::Unknown,
    }

    let Some(pr_number) = pr_number_from_scope(scope) else {
        return workflow_hints::PrState::Absent;
    };
    let mut sink = network_retry::NoopSink;
    match crate::forge::forge_for(project_root).change_metadata(pr_number, &mut sink) {
        Ok(metadata) => match metadata.state {
            crate::forge::ChangeState::Open => workflow_hints::PrState::Open(pr_number),
            crate::forge::ChangeState::Merged => workflow_hints::PrState::Open(pr_number),
            crate::forge::ChangeState::Closed => workflow_hints::PrState::Absent,
        },
        Err(_) => workflow_hints::PrState::Absent,
    }
}

pub(crate) fn session_end_unshipped_warning_lines(work: &SessionEndUnshippedWork) -> Vec<String> {
    vec![
        format!(
            "session end detected {} unshipped commit{} for {} on branch `{}` with no open PR.",
            work.commits_ahead,
            if work.commits_ahead == 1 { "" } else { "s" },
            work.scope,
            work.branch
        ),
        format!(
            "Lease {} will be removed, but the branch is still local/unshipped; run `git push` + `aida pr ship` or recover manually.",
            work.lease_id
        ),
    ]
}

pub(crate) fn session_end_unshipped_punt_record(
    work: &SessionEndUnshippedWork,
    now: chrono::DateTime<chrono::Utc>,
) -> punt::PuntRecord {
    punt::PuntRecord {
        timestamp: now,
        spec: work.scope.clone(),
        category: aida_core::PuntCategory::Other,
        detail: format!(
            "session end observed {} commit{} ahead on branch `{}` with no open PR; work may need manual push/PR recovery",
            work.commits_ahead,
            if work.commits_ahead == 1 { "" } else { "s" },
            work.branch
        ),
        lean: Some("recover by pushing the branch and opening/shipping a PR".to_string()),
        raised_by: Some("session-end".to_string()),
        resolution_path: "punted".to_string(),
        classification: Some("UNSHIPPED-SESSION-END".to_string()),
        escalation_reason: None,
        answer: None,
        answered_by: None,
        decision: Some("visibility-warning".to_string()),
        principle_link: None,
        calibration_pair: None,
        paused_at: Some(now),
        resolved_at: None,
    }
}

pub(crate) fn emit_session_end_unshipped_warning(work: &SessionEndUnshippedWork) {
    for (idx, line) in session_end_unshipped_warning_lines(work).iter().enumerate() {
        if idx == 0 {
            eprintln!("{} {}", "warning:".yellow().bold(), line);
        } else {
            eprintln!("  {}", line);
        }
    }
}

pub(crate) fn record_session_end_unshipped_work(
    project_root: &std::path::Path,
    work: &SessionEndUnshippedWork,
) {
    let record = session_end_unshipped_punt_record(work, chrono::Utc::now());
    if let Err(e) = punt::append_to_ledger(project_root, &record) {
        eprintln!(
            "{} could not write session-end punt ledger record: {e}",
            "warning:".yellow().bold()
        );
    }
}

/// BUG-422: should `aida session end` auto-skip the CI/PR probe because we're
/// in a non-interactive context? The probe shells out to `gh` with no timeout;
/// with no TTY to Ctrl+C or answer a prompt it can hang the lease teardown
/// forever — which breaks the multi-agent handoff (tearing down a finished
/// worktree from outside the agent). True only when the probe WOULD otherwise
/// run (`!skip_ci && !force`), the caller did NOT explicitly ask to wait for CI
/// (`--wait-ci`/`--watch-ci` is honored even headless — that's an explicit
/// choice), and stdin is not a TTY. trace:BUG-422 | ai:claude
pub(crate) fn non_tty_skips_ci_probe(
    skip_ci: bool,
    force: bool,
    explicit_ci_wait: bool,
    stdin_is_tty: bool,
) -> bool {
    !skip_ci && !force && !explicit_ci_wait && !stdin_is_tty
}

#[cfg(test)]
#[path = "tests/bug422_ci_probe_tests.rs"]
mod bug422_ci_probe_tests;

/// What `aida session end`'s confirmation gate should do.
// trace:BUG-776 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionEndConfirm {
    /// `--yes` / `--force` supplied — end the session without asking.
    Skip,
    /// Real TTY, no skip flag — ask `Continue? [y/N]`.
    Prompt,
    /// No TTY and no skip flag — nobody can answer. Fail loudly instead of
    /// no-op'ing: a silent exit that leaves the lease behind looks like the
    /// command ran and is how leases get orphaned.
    RefuseNonInteractive,
}

/// Pure decision for the confirmation gate, so the "never silently no-op"
/// contract is unit-testable.
// trace:BUG-776 | ai:claude
pub(crate) fn session_end_confirm_action(
    yes: bool,
    force: bool,
    stdin_is_tty: bool,
) -> SessionEndConfirm {
    if yes || force {
        SessionEndConfirm::Skip
    } else if stdin_is_tty {
        SessionEndConfirm::Prompt
    } else {
        SessionEndConfirm::RefuseNonInteractive
    }
}

/// The refusal text for [`SessionEndConfirm::RefuseNonInteractive`]. Names
/// the surviving lease explicitly so the caller can't read the exit as "it
/// ran".
// trace:BUG-776 | ai:claude
pub(crate) fn session_end_confirm_refusal(
    session_id: &str,
    lease_path: &std::path::Path,
) -> String {
    format!(
        "session {} was NOT ended — confirmation is required and stdin is not a terminal, \
         so nobody can answer the prompt. The lease at {} is still held. \
         Re-run with `--yes` (or `--force`) to confirm non-interactively.",
        session_id,
        lease_path.display()
    )
}

#[cfg(test)]
#[path = "tests/bug776_session_end_confirm_tests.rs"]
mod bug776_session_end_confirm_tests;

/// BUG-779: did this `session end` invocation name a specific session? With
/// zero active leases that distinction decides the exit code, and the exit code
/// is what the `aida()` shell wrapper branches on: a BARE `session end` with
/// nothing to end is a benign no-op (exit 0, empty stdout, eval is a no-op),
/// while an EXPLICIT target that cannot exist is a failed request that must
/// exit non-zero so its error text is never eval'd as shell.
///
/// Returns the named target (first of id / --spec / --branch), ignoring blanks.
// trace:BUG-779 | ai:claude
pub(crate) fn session_end_explicit_target(
    id_query: Option<&str>,
    spec_query: Option<&str>,
    branch_query: Option<&str>,
) -> Option<String> {
    [id_query, spec_query, branch_query]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|q| !q.is_empty())
        .map(str::to_string)
}

// The shell matrix (bash + zsh-when-present) the wrapper tests below drive.
// trace:TASK-1174 | ai:claude
#[cfg(test)]
#[path = "tests/shell_wrapper_harness.rs"]
mod shell_wrapper_harness;

#[cfg(test)]
#[path = "tests/bug_779_wrapper_eval_tests.rs"]
mod bug_779_wrapper_eval_tests;

// trace:TASK-1171 | ai:claude
#[cfg(test)]
#[path = "tests/task_1171_eval_channel_tests.rs"]
mod task_1171_eval_channel_tests;

// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn session_end(
    id_query: Option<&str>,
    spec_query: Option<&str>,
    branch_query: Option<&str>,
    yes: bool,
    force: bool,
    purge_cc: bool,
    wait_ci: bool,
    watch_ci: bool,
    skip_ci: bool,
    // STORY-714: explicit --return (back-compat; pooled trees now return by
    // default). TASK-985: --remove forces deletion instead of returning.
    return_to_pool: bool,
    remove: bool,
) -> Result<()> {
    let _ = return_to_pool; // returning is now the default; flag kept for compat
    let project_root = find_project_root()?;
    let leases = list_leases(&project_root);
    if leases.is_empty() {
        // BUG-779: a BARE `session end` with nothing to end is a benign no-op
        // (exit 0 — the shell wrapper evals empty stdout and moves on). But an
        // EXPLICIT target that cannot exist is a failed request; exiting 0 there
        // let the wrapper treat the error text as shell code.
        // trace:BUG-779 | ai:claude
        if let Some(q) = session_end_explicit_target(id_query, spec_query, branch_query) {
            anyhow::bail!(
                "No lease found for `{}` — there are no active sessions in this project. \
                 Run `aida session leases` to see active leases",
                q
            );
        }
        eprintln!("(no active sessions)");
        return Ok(());
    }

    let target = resolve_session_to_end(id_query, spec_query, branch_query, &leases, yes)?;

    // BUG-361: visibility-first guard for the ceiling variant where an agent
    // commits locally and exits without any lifecycle command (`queue done`,
    // `/aida-pr`, or `aida pr ship`). `session end` is not a hard blocker here:
    // it emits a BUG-360-style warning and writes a STORY-325 ledger record so
    // the operator's next sweep sees the unshipped branch. This only fires when
    // GH confirms no open PR; unknown GH states never assert "missing PR".
    // trace:BUG-361
    let commits_ahead = branch_unshipped_patch_count_default(&project_root, &target.branch)
        .or_else(|| branch_commits_ahead_main(&project_root, &target.branch));
    let pr_state =
        session_end_pr_state_for_scope_or_branch(&project_root, &target.scope, &target.branch);
    // BUG-367: if the spec the lease owns has auto-bumped to Completed, or if the PR
    // for this branch has already been squash-merged / merged, the work is shipped
    // regardless of what the local branch looks like. Suppress warning.
    // trace:BUG-367 | ai:antigravity
    let mut is_completed = false;
    let store_path = project_root.join(".aida-store");
    if store_path.exists() {
        if let Ok(storage) = Storage::new(store_path).load() {
            if let Some(req) = storage.get_requirement_by_spec_id(&target.scope) {
                if req.status == RequirementStatus::Completed {
                    is_completed = true;
                }
            }
        }
    }
    if !is_completed {
        if let PrLookup::Found(_) =
            detect_merged_pr_for_branch_via_forge(&project_root, &target.branch)
        {
            is_completed = true;
        }
    }
    let session_end_unshipped_work = if is_completed {
        None
    } else {
        classify_session_end_unshipped_work(
            &target.id,
            &target.scope,
            &target.branch,
            commits_ahead,
            pr_state,
        )
    };
    if let Some(work) = &session_end_unshipped_work {
        emit_session_end_unshipped_warning(work);
    }

    // STORY-127 detector (4): leaving this role's session while a DIFFERENT
    // role's queue has work waiting. Warn (don't block) so the user can
    // switch role first instead of walking away from queued work.
    // trace:STORY-127 | ai:claude
    {
        let role_counts = read_queue_role_counts(&project_root);
        let waiting = cross_role_queue_waiting(target.role.as_deref(), &role_counts);
        if let Some(msg) = session_end_cross_role_warning(&waiting) {
            eprintln!("  {} {}", "Warning:".yellow().bold(), msg);
        }
    }

    // TASK-111: CI awareness. Probe the branch for an open PR's CI state
    // and surface info / prompt / warn based on the state. --skip-ci and
    // --force both bypass the probe (force already gets a noisy override
    // path; --skip-ci is the quieter explicit "don't bother"). Probe
    // happens BEFORE the BUG-61 live-claude check so the user can decide
    // whether to bail-and-keep-fixing without first wading through the
    // claude-process disclosure. trace:TASK-111 | ai:claude
    if non_tty_skips_ci_probe(
        skip_ci,
        force,
        wait_ci || watch_ci,
        std::io::stdin().is_terminal(),
    ) {
        // BUG-422: non-interactive shell — the `gh` CI/PR probe has no timeout
        // and would hang the lease teardown with no TTY to interrupt it. Skip
        // it (equivalent to the proven `--skip-ci` workaround). trace:BUG-422
        eprintln!(
            "  {} non-interactive shell — skipping the CI/PR probe (it would hang the lease \
             teardown with no TTY). Pass --wait-ci from a TTY to force it, or --skip-ci to silence.",
            "Note:".dimmed()
        );
    } else if !skip_ci && !force {
        // STORY-516: forge-routed. Probe on the session's resolved
        // `project_root` rather than the process cwd, so the forge that answers
        // is the driven repository's. trace:TASK-1273 | ai:claude
        let forge_kind = crate::forge::resolve_forge_kind(&project_root);
        let probe = ci_probe_with_forge(&project_root, forge_kind, &target.branch);
        // TASK-233: --watch-ci blocks like --wait-ci, so it produces the
        // same `CiAction::Wait`; the difference is the live display.
        match decide_ci_action(&probe, wait_ci || watch_ci, yes) {
            CiAction::Proceed => {}
            CiAction::Wait => {
                let final_probe = if watch_ci {
                    eprintln!(
                        "{} CI in progress — watching until terminal state (Ctrl+C to stop watching)",
                        "→".cyan()
                    );
                    // TASK-1165: hand the already-resolved session root down so
                    // the CiTerminal emit can't re-derive one from the cwd.
                    watch_ci_for_context_via_forge(&project_root, &target.branch, false)
                // STORY-516
                } else {
                    eprintln!(
                        "{} CI in progress — waiting for terminal state (poll every 30s; Ctrl+C to skip)",
                        "→".cyan()
                    );
                    wait_for_ci_terminal(Some(&project_root), &target.branch)
                };
                match decide_ci_action(&final_probe, false, yes) {
                    CiAction::Proceed => {}
                    CiAction::Wait => {} // can't loop on Wait again
                    CiAction::Cancel(msg) => {
                        eprintln!("{}", msg);
                        return Ok(());
                    }
                }
            }
            CiAction::Cancel(msg) => {
                eprintln!("{}", msg);
                return Ok(());
            }
        }
    }

    // BUG-734: a lease with no worktree (empty `worktree_path` — e.g. a
    // reviewer lease that never created one) has nothing on disk to tear
    // down. An empty path must never reach the worktree scans below: the
    // live-claude probe, the dirty gate, and `git worktree remove` would
    // all evaluate the caller's cwd (or every path) instead of a real
    // worktree, blocking the end of a lease that only needs its record
    // deleted. trace:BUG-734 | ai:claude
    let has_worktree = !target.worktree_path.as_os_str().is_empty();
    // Capture before `git worktree remove` deletes the shim. The pure guard
    // prevents malformed or legacy fallback values from targeting parent `target/`.
    // trace:TASK-1577 | ai:codex
    let session_target_dir = has_worktree
        .then(|| session_cargo_target_dir(&target.worktree_path))
        .flatten()
        .filter(|path| is_session_cargo_target_dir(path, &target.worktree_path));

    // BUG-61: detect live `claude` processes whose cwd is under the
    // worktree we're about to remove. If we don't, `git worktree remove`
    // succeeds (cwd is just a soft handle), the dir is unlinked, and the
    // claude keeps running with `(deleted)` as its cwd — leaking memory,
    // confusing future liveness probes, and potentially firing hooks at
    // paths that no longer exist. Default refuses with a clear message;
    // `--force` SIGTERMs them with a 5s grace, then SIGKILLs.
    // trace:BUG-61 | ai:claude
    // BUG-734: worktree-less lease ⇒ no process-leak check — nothing to
    // remove, so nothing can leak. trace:BUG-734 | ai:claude
    let leaked = if has_worktree {
        probe_live_claudes_in_worktree(&target.worktree_path)
    } else {
        Vec::new()
    };
    if !leaked.is_empty() {
        if !force {
            let mut msg = format!(
                "{} live claude process{} inside worktree {} — would leak with `(deleted)` cwd if we removed the worktree:",
                leaked.len(),
                if leaked.len() == 1 { "" } else { "es" },
                target.worktree_path.display()
            );
            for p in &leaked {
                msg.push_str(&format!("\n  pid {} cwd {}", p.pid, p.cwd.display()));
            }
            msg.push_str("\n\nExit those claude(s) first, or pass --force to SIGTERM them.");
            anyhow::bail!(msg)
        }
        eprintln!(
            "{} {} live claude process(es) inside worktree — sending SIGTERM, then SIGKILL after 5s",
            "→".dimmed(),
            leaked.len()
        );
        let _ = terminate_pids_with_grace(&leaked.iter().map(|p| p.pid).collect::<Vec<_>>(), 5);
    }

    // STORY-73: human output to stderr, eval-friendly `unset` to stdout
    // when wrapped — same shape as session_start's `export`. Stdin/stderr
    // for the prompt still works inside `eval "$(...)"` because $(...)
    // captures stdout only. trace:STORY-73 | ai:claude
    eprintln!("About to end session {}:", target.id.yellow());
    eprintln!("  scope:    {}", target.scope);
    eprintln!("  branch:   {}", target.branch);
    eprintln!("  worktree: {}", target.worktree_path.display());
    eprintln!();
    eprintln!("Effects:");
    eprintln!(
        "  - delete lease at {}",
        lease_path(&project_root, &target.id).display()
    );
    // BUG-734: only promise a worktree removal when there is one.
    if has_worktree {
        eprintln!(
            "  - run `git worktree remove {}` (branch {} kept; merge/discard manually)",
            target.worktree_path.display(),
            target.branch
        );
        if let Some(path) = session_target_dir.as_ref().filter(|path| path.is_dir()) {
            eprintln!("  - remove session cargo target dir {}", path.display());
        }
    } else {
        eprintln!("  - no worktree attached — nothing on disk to remove");
    }

    // BUG-706: `--yes` and `--force` both skip the confirmation. On a
    // non-terminal stdin (a headless drain reclaiming a dead lease) there is
    // nobody to answer, so the old unconditional prompt read EOF and always
    // aborted — blocking unattended cleanup. Now: prompt only on a real TTY
    // without a skip flag; non-TTY without `--yes`/`--force` fails with a
    // clear, actionable error instead of a silent "Aborted."
    // BUG-776: route the gate through a pure decision fn, and make the
    // non-interactive refusal say the session was NOT ended and the lease
    // survives — the preamble above otherwise reads like the teardown ran.
    // trace:BUG-776 | ai:claude
    match session_end_confirm_action(yes, force, std::io::stdin().is_terminal()) {
        SessionEndConfirm::Skip => {}
        SessionEndConfirm::RefuseNonInteractive => {
            anyhow::bail!(session_end_confirm_refusal(
                &target.id,
                &lease_path(&project_root, &target.id)
            ));
        }
        SessionEndConfirm::Prompt => {
            use std::io::Write;
            eprint!("\nContinue? [y/N] ");
            std::io::stderr().flush()?;
            let mut ans = String::new();
            std::io::stdin().read_line(&mut ans)?;
            if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                eprintln!("Aborted — session {} is still open.", target.id);
                return Ok(());
            }
        }
    }

    if let Some(work) = &session_end_unshipped_work {
        record_session_end_unshipped_work(&project_root, work);
    }

    // STORY-56: flatten the session's activity log into each
    // participating role's project-level activity stream BEFORE we delete
    // the lease — once the lease is gone the activity file is orphaned.
    // For each role, take the newest entry per spec_id from the session
    // log, merge into the project role's `activity` (newest first, dedupe
    // by spec_id, truncate to ACTIVITY_MAX) so post-session views like
    // `aida role show` still surface what was worked on under the closed
    // session. trace:STORY-56 | ai:claude
    aggregate_session_activity_into_roles(&project_root, &target.id);
    let activity_file = session_activity_path(&project_root, &target.id);
    let canonical_activity = activity_file.canonicalize().ok();
    let activity_target: std::path::PathBuf = canonical_activity
        .clone()
        .unwrap_or_else(|| activity_file.clone());

    // Snapshot the lease file's authoritative on-disk location BEFORE we
    // touch the worktree's symlinks. When session_end runs from inside the
    // worktree (the natural flow), `project_root` IS the worktree path —
    // and `<worktree>/.aida/sessions/<id>.toml` traverses the symlink
    // installed at session_start to reach the parent's real file. Once we
    // strip that symlink (or git removes the whole worktree), the
    // worktree-relative path stops resolving. canonicalize() follows the
    // symlink now and gives us a stable target for the unlink later.
    // trace:BUG-56 | ai:claude
    let lease_file_via_symlink = lease_path(&project_root, &target.id);
    let canonical_lease = lease_file_via_symlink.canonicalize().ok();

    // Clean the symlinks we created at session start before git tries to
    // remove the worktree — they count as untracked files and `git worktree
    // remove` would otherwise refuse without --force.
    //   - .aida-store/ is a whole-directory symlink (top-level)
    //   - .aida/ itself is a real dir (with tracked content) — leave it
    //     alone, but strip the runtime symlinks inside it that
    //     session_start created. trace:BUG-52 | ai:claude
    // BUG-734: no worktree ⇒ no session symlinks to strip (an empty path
    // would resolve the checks against the caller's cwd). trace:BUG-734 | ai:claude
    if has_worktree {
        let store_link = target.worktree_path.join(".aida-store");
        if store_link.is_symlink() {
            let _ = std::fs::remove_file(&store_link);
        }
        unlink_worktree_aida_runtime(&target.worktree_path.join(".aida"));
    }

    // BUG-67: refuse to nuke a worktree that has real uncommitted work.
    // `git status --porcelain` excludes ignored entries by default, so
    // `target/`, `.aida/cache.db`, etc. don't trip this check — those are
    // by definition disposable. Tracked-but-modified or untracked-and-
    // unignored files DO trip it and require `--force`. We run this AFTER
    // stripping the runtime symlinks (which would otherwise show up as
    // untracked) and BEFORE deleting the lease, so a refusal leaves the
    // session intact and recoverable. trace:BUG-67 | ai:claude
    // BUG-734: no worktree ⇒ no dirty gate — `git -C ""` would report the
    // caller's cwd, refusing on unrelated dirt. trace:BUG-734 | ai:claude
    let dirty_entries = if has_worktree {
        worktree_dirty_entries(&target.worktree_path)
    } else {
        Vec::new()
    };
    // BUG-652: `--return` resets the worktree (reset --hard + clean -fd), so a
    // dirty pooled tree must NOT be refused here — refusing leaves the tree
    // leased and silently breaks reuse (the next acquire creates a fresh tree).
    // Instead salvage a patch (so no work is lost) and let it through to the
    // return path. Non-pool trees / non-return still refuse without --force.
    // TASK-985: a pooled tree returns by DEFAULT now — `--remove` opts out.
    // trace:BUG-652 trace:STORY-714 trace:TASK-985 | ai:claude
    let returning_pool_tree = !remove
        && has_worktree
        && aida_core::worktree_pool::is_pool_worktree(&project_root, &target.worktree_path);
    match dirty_gate_outcome(!dirty_entries.is_empty(), force, returning_pool_tree) {
        DirtyGateOutcome::Proceed => {}
        DirtyGateOutcome::Salvage => {
            match salvage_worktree_patch(&project_root, "pool-return", None, &target.worktree_path) {
                Ok(Some(p)) => eprintln!(
                    "{} salvaged uncommitted changes to {} before returning the pooled worktree (it will be reset)",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    p.display().to_string().dimmed()
                ),
                Ok(None) => {}
                Err(e) => eprintln!(
                    "{} could not salvage uncommitted changes before return: {} — proceeding (they will be reset away)",
                    "Warning:".yellow().bold(),
                    e
                ),
            }
        }
        DirtyGateOutcome::Refuse => {
            let mut msg = format!(
                "worktree {} has uncommitted changes:\n",
                target.worktree_path.display()
            );
            for line in &dirty_entries {
                msg.push_str(&format!("  {}\n", line));
            }
            msg.push_str(
                "\nPass `--force` to `aida session end` to discard these and remove the worktree.\n\
                 (Gitignored files like target/ and .aida/cache.db never require --force.)",
            );
            anyhow::bail!(msg);
        }
    }

    // Delete the lease file BEFORE removing the worktree. The canonical
    // path resolved above points at the parent's sessions dir, so the
    // unlink succeeds regardless of whether the worktree's symlink chain
    // is still intact. Reporting is honest: success printed only on
    // actual success; missing file is a quiet no-op (already-released
    // lease from a previous partial run is fine); other errors warn so
    // the user can clean up manually instead of trusting a stale check mark.
    // trace:BUG-56 | ai:claude
    let lease_target: &std::path::Path = canonical_lease
        .as_deref()
        .unwrap_or(&lease_file_via_symlink);
    match std::fs::remove_file(lease_target) {
        Ok(_) => eprintln!(
            "{} lease deleted",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Already gone — keep quiet.
        }
        Err(e) => {
            eprintln!(
                "{} could not delete lease at {}: {} — remove manually",
                "Warning:".yellow().bold(),
                lease_target.display(),
                e
            );
        }
    }

    // BUG-694: reap the sibling MCP work-claim for this scope. An MCP client
    // that claimed the spec via `claim_task` drops `mcp-claim.<spec>.toml`
    // alongside the session lease; without this, `session end` released the
    // session lease but left the mcp-claim behind, so the spec kept surfacing
    // as stale running work long after the worktree was gone (TASK-702's claim
    // lingered 24 days). Best-effort + NotFound-quiet, mirroring the lease
    // unlink above. trace:BUG-694 | ai:claude
    let mcp_claim = mcp::mcp_claim_path(&leases_dir(&project_root), &target.scope);
    match std::fs::remove_file(&mcp_claim) {
        Ok(_) => eprintln!(
            "{} mcp-claim reaped ({})",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            target.scope
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No MCP claim for this scope — the common case. Quiet.
        }
        Err(e) => {
            eprintln!(
                "{} could not reap mcp-claim {}: {} — remove manually",
                "Warning:".yellow().bold(),
                mcp_claim.display(),
                e
            );
        }
    }

    // STORY-637: release the cross-clone lease claim on the shared store so a
    // peer clone can take this scope. Best-effort by design — staleness (pid /
    // TTL) reclaims a never-released claim, so a failed release never
    // deadlocks. trace:STORY-637 | ai:claude
    coordination::release_claim(
        &project_root.join(".aida-store"),
        &target.scope,
        &project_root,
    );

    // STORY-564: drop this session's `--zen` needs-human marker, if any, so a
    // future session that reuses the (time-ordered, effectively-unique) id
    // never inherits a stale "pause at finish" signal. Best-effort, quiet.
    // trace:STORY-564 | ai:claude
    zen::clear_needs_human_marker(&project_root, &target.id);

    // BUG-80: drop the planned-cluster manifest for this session, if any.
    // Manifests are runtime per-session state — pinning them around past
    // a closed lease leaks files into `.aida/sessions/`. Quiet on missing.
    // trace:BUG-80 | ai:claude
    let manifest_target = session_manifest::manifest_path(&project_root, &target.id);
    if manifest_target.exists() {
        match std::fs::remove_file(&manifest_target) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!(
                    "{} could not delete session manifest at {}: {}",
                    "Warning:".yellow().bold(),
                    manifest_target.display(),
                    e
                );
            }
        }
    }

    // STORY-56: drop the session activity log. Quiet on missing — short
    // sessions that never recorded any activity won't have one. We've
    // already aggregated entries into the project-level role(s) above.
    // trace:STORY-56 | ai:claude
    if activity_target.exists() {
        match std::fs::remove_file(&activity_target) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!(
                    "{} could not delete session activity log at {}: {}",
                    "Warning:".yellow().bold(),
                    activity_target.display(),
                    e
                );
            }
        }
    }

    // TASK-68: detect whether the user's shell was sitting inside the
    // worktree we're about to nuke. We capture cwd BEFORE we chdir away
    // for the git ops, so we can later emit a `cd <parent>` for the
    // shell wrapper (Layer 2 of the fix). Canonicalize both sides so
    // symlink chains don't mask the match. trace:TASK-68 | ai:claude
    let starting_cwd = std::env::current_dir().ok();
    let canonical_worktree = target
        .worktree_path
        .canonicalize()
        .unwrap_or_else(|_| target.worktree_path.clone());
    // BUG-734: `starts_with` on an empty canonical path matches every cwd —
    // a worktree-less lease must never trigger the cd-out. trace:BUG-734 | ai:claude
    let cwd_was_inside_worktree = has_worktree
        && starting_cwd
            .as_ref()
            .and_then(|c| c.canonicalize().ok())
            .map(|c| c.starts_with(&canonical_worktree))
            .unwrap_or(false);

    // TASK-68 Layer 1: chdir to the parent project before running git.
    // `git worktree remove` refuses outright when the calling process's
    // cwd is inside the worktree being removed. Switching the Rust
    // process's cwd here doesn't affect the parent shell — that's
    // handled separately by Layer 2's eval'd `cd`. trace:TASK-68
    let _ = std::env::set_current_dir(&project_root);

    // BUG-483: before force-removing the worktree, check whether another
    // registered lease shares this same `worktree_path`. `aida agent new`
    // can land two sessions in one worktree (BUG-416); force-removing it
    // out from under the peer strands them (TASK-0396 cross-worktree
    // fingerprint hazard + BUG-108 `(deleted)` cwd). The target lease was
    // already deleted above, so the live lease set is peers-only, but we
    // still filter by id for robustness. Be conservative: any peer ⇒ skip
    // the removal entirely (leaving a worktree is recoverable; deleting a
    // shared one is not). We still ended THIS lease/registry entry above.
    // trace:BUG-483 | ai:claude
    let remaining_leases = list_leases(&project_root);
    use std::io::Write;
    // BUG-734: two worktree-less leases share the same empty path — don't let
    // them count as worktree peers. trace:BUG-734 | ai:claude
    let shared_peer = if has_worktree {
        peer_lease_sharing_worktree(&remaining_leases, &target.id, &target.worktree_path)
    } else {
        None
    };
    // STORY-714: when `--return` is set and this is a registered warm-pool
    // worktree, hand it back to the pool (reset to a clean detached base, mark
    // idle) instead of deleting it. The directory persists so the next acquire
    // reuses it warm. trace:STORY-714 | ai:claude
    let mut returned_to_pool = false;
    if !has_worktree {
        // BUG-734: worktree-less lease — the record is gone; nothing on disk
        // to remove or return. trace:BUG-734 | ai:claude
        eprintln!(
            "{} no worktree attached to this session — lease record removed; nothing on disk to clean up",
            "→".dimmed()
        );
    } else if let Some(peer) = shared_peer {
        // Skip the removal; this lease/registry entry is already gone, so the
        // session has ended — we just leave the shared worktree standing.
        eprintln!(
            "{} worktree {} is shared by lease {} ({}) — ending this session but leaving the shared worktree in place",
            "→".dimmed(),
            target.worktree_path.display(),
            peer.id.yellow(),
            peer.scope
        );
    } else if !remove
        && aida_core::worktree_pool::is_pool_worktree(&project_root, &target.worktree_path)
    {
        // TASK-985: returning a pooled tree is the default; `--remove` opts out.
        match aida_core::worktree_pool::return_to_pool(&project_root, &target.worktree_path) {
            Ok(_) => {
                returned_to_pool = true;
                eprintln!(
                    "{} worktree returned to the warm-pool (reset to a clean base, marked idle — directory + build cache kept)",
                    crate::glyph(crate::glyphs::Glyph::Check).green()
                );
            }
            Err(e) => eprintln!(
                "{} could not return worktree to the pool: {} — left in place",
                "Warning:".yellow().bold(),
                e
            ),
        }
    } else {
        // BUG-67: always pass `--force`. We've already gated on the dirty
        // check above, so by this point the only non-clean state is
        // gitignored (target/, .aida/cache.db, etc.) — which git still
        // refuses to remove without `--force`. trace:BUG-67 | ai:claude
        let res = std::process::Command::new("git")
            .arg("-C")
            .arg(&project_root)
            .args([
                "worktree",
                "remove",
                "--force",
                "--", // trace:BUG-1622 | ai:claude
                target.worktree_path.to_str().unwrap_or_default(),
            ])
            .output();
        match res {
            Ok(o) if o.status.success() => {
                let _ = std::io::stderr().write_all(&o.stdout);
                let _ = std::io::stderr().write_all(&o.stderr);
                eprintln!(
                    "{} worktree removed",
                    crate::glyph(crate::glyphs::Glyph::Check).green()
                );
                if let Some(path) = &session_target_dir {
                    remove_session_cargo_target_dir(path, &target.worktree_path);
                }
            }
            Ok(o) => {
                let _ = std::io::stderr().write_all(&o.stdout);
                let _ = std::io::stderr().write_all(&o.stderr);
                eprintln!(
                    "{} `git worktree remove` failed; you may need to run it manually with --force",
                    "Warning:".yellow().bold()
                );
            }
            Err(_) => {
                eprintln!(
                    "{} `git worktree remove` failed to spawn; you may need to run it manually",
                    "Warning:".yellow().bold()
                );
            }
        }
    }
    // BUG-734: the branch-retained hint only makes sense when a worktree
    // (with its checked-out branch) was actually removed. trace:BUG-734 | ai:claude
    if has_worktree {
        eprintln!(
            "  branch {} retained — merge or `git branch -D {}` when ready",
            target.branch.cyan(),
            target.branch
        );
    }

    // BUG-614: lossless prune of administrative worktree entries whose
    // directories are already gone. `git worktree prune` ONLY removes the
    // bookkeeping for worktrees whose on-disk dir no longer exists — it never
    // touches a live worktree — so it is always safe and cheap. Running it on
    // session-end keeps `.git/worktrees/` from accumulating stale entries
    // (which scale the per-worktree git spawns `aida status` makes). Quiet,
    // best-effort: a failure here never fails the session teardown.
    // trace:BUG-614 | ai:claude
    lossless_prune_worktrees(&project_root);

    // TASK-70: handle the Claude Code project dir orphaned by the
    // worktree removal. Claude Code stores per-session jsonls under
    // `~/.claude/projects/<encoded-cwd>/`; once the cwd vanishes, the
    // jsonls are stranded — `claude --resume <id>` may fail or behave
    // unexpectedly. Default: warn + point at `session prune --orphans`.
    // With `--purge-cc`: remove the dir atomically here.
    // BUG-483: skip entirely when we kept a shared worktree — the dir is
    // still live (the peer is using it), so it's neither orphaned nor safe
    // to purge. STORY-714: also skip when we returned the tree to the pool —
    // the directory persists, so its cwd is not orphaned.
    // BUG-734: no worktree was ever created, so no Claude Code project dir
    // can have been orphaned by its removal. trace:BUG-734 | ai:claude
    // trace:TASK-70 BUG-483 STORY-714 | ai:claude
    if has_worktree && shared_peer.is_none() && !returned_to_pool {
        if let Ok(cc_dir) = session::claude_project_dir(&canonical_worktree) {
            if cc_dir.is_dir() {
                if purge_cc {
                    match std::fs::remove_dir_all(&cc_dir) {
                        Ok(_) => eprintln!(
                            "{} purged Claude Code project dir {}",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            cc_dir.display().to_string().dimmed()
                        ),
                        Err(e) => eprintln!(
                            "{} could not purge {}: {} — remove manually if you don't want it",
                            "Warning:".yellow().bold(),
                            cc_dir.display(),
                            e
                        ),
                    }
                } else {
                    eprintln!(
                        "{} Claude Code project dir {} is now orphaned (cwd gone). \
                         Run `aida session prune --orphans` to clean up, or pass \
                         `--purge-cc` on `aida session end` next time.",
                        crate::glyph(crate::glyphs::Glyph::Info).dimmed(),
                        cc_dir.display().to_string().dimmed()
                    );
                }
            }
        }
    }

    // STORY-66: when the just-ended session's branch has an open PR, file a
    // Story-typed review item routed to the `reviewer` role with `implements`
    // relations to every spec referenced in the PR's commit messages. Best
    // effort: any failure here logs a warning and is otherwise silent — we
    // never fail `session_end` because of a queue side-effect.
    //
    // BUG-72: always print the outcome (filed / skipped + reason), so the
    // user sees whether the hook fired. The silent-skip case was painful in
    // PR-7 wrap-up: STORY-66 silently no-op'd and the reviewer queue stayed
    // empty with no explanation. trace:STORY-66 BUG-72 | ai:claude
    //
    // STORY-90: this is the backup trigger now — primary fires from
    // /aida-pr right after `gh pr create`. The idempotency check inside
    // `try_auto_queue_pr_review` (Review PR-N story already exists) means
    // both firing the same PR is safe; the second one renders as
    // `AlreadyExists`. trace:STORY-90 | ai:claude
    // BUG-107: the auto-queue side-effect shells out to `gh`, `git`, and
    // `aida`, all of which need a working directory that still exists.
    // When `aida session end` is run from inside the worktree it just
    // removed, `project_root` (resolved from cwd at entry) now points at
    // a deleted directory and every spawn fails with a misleading
    // ENOENT blamed on the gh binary. Prefer the lease's recorded parent
    // project root — the main worktree, which `session end` never
    // removes. trace:BUG-107 | ai:claude
    let outcome = match auto_queue_working_dir(target.parent_project_root.as_deref(), &project_root)
    {
        Some(root) => try_auto_queue_pr_review(
            &root,
            &target.branch,
            &target.id,
            AutoQueueOrigin::SessionEnd,
        ),
        None => AutoQueueOutcome::skipped_needs_attention(format!(
            "auto-queue: no surviving project directory to query `gh` from \
                 (worktree removed, parent project root unrecorded) — reviewer \
                 story for branch `{}` not filed; run `aida pr auto-queue-review \
                 --branch {}` from the main repo",
            target.branch, target.branch
        )),
    };
    render_auto_queue_outcome(&outcome);

    // STORY-106: workflow hint when a review story is now queued for this
    // session's PR. Fires for both Filed (just minted) and AlreadyExists
    // (idempotent re-fire) — either way, the reviewer story is in the
    // queue and the user's next action is `aida queue work`.
    // trace:STORY-106 | ai:claude
    if matches!(
        outcome.status,
        AutoQueueStatus::Filed | AutoQueueStatus::AlreadyExists
    ) {
        // BUG-776: never render the `PR #0 next: ...` chain. A synthetic id-0
        // change ref names no reviewable PR, so every step of the hint
        // (`aida queue work PR-0`, merge PR-0) points at nothing.
        // trace:BUG-776 | ai:claude
        if let Some(pr_n) = outcome.pr_number.filter(|n| *n > 0) {
            workflow_hints::after_session_end_with_pr(
                Some(&project_root),
                pr_n,
                &outcome.covered_specs,
            );
        }
    }

    if let Some(store_path) = detect_distributed_store_from(&project_root) {
        // STORY-493: durably digest this session's local mailbox traffic into
        // the git-canonical orphan store before the store is pushed, so a
        // session's inter-agent messages are replayable/shareable once it
        // closes. Best-effort + non-fatal — a digest failure must never abort
        // session cleanup. Runs before the push so the digest commit rides the
        // same push. trace:STORY-493 | ai:claude
        maybe_digest_mailbox_best_effort(&store_path, "session-end");
        maybe_auto_push_store(&store_path, StoreAutoPushMode::SessionEnd, "session-end");
    }

    // STORY-73: emit `unset AIDA_SESSION_ID` to stdout when wrapped via the
    // shell helper's `eval "$(...)"`, so the calling shell's env var
    // doesn't go stale after the lease it pointed at is gone.
    // trace:STORY-73 | ai:claude
    //
    // TASK-68 Layer 2: if the user's shell was sitting inside the
    // worktree we just removed, their cwd is now a dangling (deleted)
    // inode — every path-relative command would fail until they
    // manually cd out. Emit a `cd <parent>` to stdout so the shell
    // wrapper's `eval` lands them in the parent project naturally.
    // No-op for users running `aida session end` from outside the
    // worktree (and for direct (non-wrapped) invocation, since stdout
    // is a TTY then). trace:TASK-68 | ai:claude
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        // trace:TASK-1171 | ai:claude — stdout inside this scope is shell code
        // (the interleaved `eprintln!` below is stderr, so it stays prose).
        let _eval = crate::shell_eval::EvalBlock::open();
        println!("unset AIDA_SESSION_ID");
        // BUG-483: only emit a `cd` out when we actually removed the worktree.
        // When a peer lease shares it we left it standing, so the shell's cwd
        // is still valid — yanking the user out of it would be a surprise.
        // trace:BUG-483 | ai:claude
        if cwd_was_inside_worktree && shared_peer.is_none() {
            let escaped = project_root.display().to_string().replace('\'', "'\\''");
            println!("cd '{}'", escaped);
            eprintln!(
                "  {} cd'd back to parent project {}",
                "↩".dimmed(),
                project_root.display().to_string().cyan()
            );
        }
    } else if cwd_was_inside_worktree && shared_peer.is_none() {
        // BUG-59: direct (non-wrapped) invocation can't `eval` a `cd` into
        // the parent shell, so the caller's cwd is left dangling on the
        // now-deleted worktree inode. Any subsequent process spawn from that
        // shell — notably Claude Code's Stop hook firing `/bin/sh` — then
        // fails with a noisy `ENOENT ... posix_spawn '/bin/sh'`. We can't
        // mutate the parent shell from here, so surface a clear `cd` hint
        // and let the caller move out before their next command.
        // trace:BUG-59 | ai:claude
        for line in session_end_stale_cwd_hint_lines(&project_root) {
            eprintln!("{}", line);
        }
    }
    Ok(())
}

/// BUG-59: ready-to-print hint lines telling the caller to leave the deleted
/// worktree cwd before running anything else (so a follow-on `/bin/sh` spawn —
/// e.g. Claude Code's Stop hook — doesn't ENOENT on the dangling inode). The
/// emitted `cd` targets the parent project root that survived the removal.
pub(crate) fn session_end_stale_cwd_hint_lines(project_root: &std::path::Path) -> Vec<String> {
    let escaped = project_root.display().to_string().replace('\'', "'\\''");
    vec![
        format!(
            "  {} your shell is still inside the removed worktree — run this before your next command:",
            "↩".dimmed()
        ),
        format!("      cd '{}'", escaped),
    ]
}

/// Open-PR metadata captured by `gh pr list` for a session's branch. Just
/// the fields the auto-queue side-effect needs to brief a reviewer.
/// trace:STORY-66 | ai:claude
pub(crate) struct OpenPrInfo {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) head_branch: Option<String>,
}

/// Why `detect_open_pr_for_branch` returned no PR — so the caller (and the
/// user reading `aida session end` output) can tell "no PR yet" from "gh
/// isn't installed" from "gh blew up". The old None-everything return type
/// hid the difference and made BUG-72 hard to diagnose.
///
/// BUG-257 split `GhFailed` further: a *transient* `GhUnreachable` (the GH
/// API was unreachable) is distinct from a `GhFailed` (auth, parse, gh
/// itself errored). The orchestrator's phase-1 reports the first as
/// *Inconclusive* (drain pauses, retry later) and the second as a phase
/// failure — same as before. Conflating the two made every network blip
/// look like a "no PR" failure with a misleading recovery hint.
/// trace:BUG-72 BUG-257 | ai:claude
pub(crate) enum PrLookup {
    Found(OpenPrInfo),
    /// `gh` ran cleanly but reported no open PR for this branch.
    NoOpenPr,
    /// `gh` is not on $PATH (binary missing).
    GhMissing,
    /// `gh` was found but its invocation failed (auth, parse, ...). The
    /// String carries the trimmed stderr / parse error so the user sees
    /// the actual cause instead of a silent no-op.
    GhFailed(String),
    /// `gh` ran but could not reach the GitHub API — a *transient* network
    /// error (DNS, TCP, TLS, githubstatus pointer). The orchestrator treats
    /// this as Inconclusive (the API outage means we cannot tell whether a
    /// PR exists), not as a phase failure. trace:BUG-257 | ai:claude
    GhUnreachable(String),
}

/// BUG-1288: the wall-clock ceiling a single forge-CLI subprocess (`gh`/`glab`)
/// may run before [`command_output_with_timeout`] gives up on it. Measured
/// cause of this spec's `aida status --full` / `aida awaiting --json` stall:
/// one `gh api .../branches/main/protection` call took 10.5s in this
/// repository while every other `gh` call in the same run finished in under a
/// second — `gh` itself has no request-timeout flag, so an occasional slow
/// endpoint or rate-limit backoff can otherwise consume the whole machine-
/// readable budget on ONE subprocess.
// trace:BUG-1288 | ai:claude
pub(crate) const FORGE_CLI_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Review follow-up (BUG-1288): on unix, kill the CHILD'S WHOLE PROCESS
/// GROUP rather than only the direct child. `command_output_with_timeout`
/// spawns with `process_group(0)` below, which makes the child's own pid its
/// process group id, so any grandchild it forks (a credential-manager
/// helper, a background `git` op) inherits that same group — `killpg` reaps
/// the group in one signal instead of leaving a grandchild alive to hold the
/// stdout/stderr pipe write ends open past the timeout. `pid` must be a pid
/// this process spawned with `process_group(0)` (the only caller), which is
/// what makes it both safe to signal and a valid process-group id.
// trace:BUG-1288 | ai:claude
#[cfg(unix)]
pub(crate) fn kill_process_group(pid: u32) {
    // SAFETY: `pid` is this process's own child (see the doc comment above),
    // and `libc::killpg` is a plain signal-delivery syscall — no pointers,
    // no aliasing concerns.
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
}

// trace:TASK-1535 | ai:codex
pub(crate) enum BoundedCommandOutput {
    Completed(std::process::Output),
    SpawnFailed,
    TimedOut,
}

pub(crate) fn command_output_with_timeout_detail(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
) -> BoundedCommandOutput {
    use std::io::Read;

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = match cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // BUG-1735: a bare spawn here turns a transient ETXTBSY into
        // `SpawnFailed`, which discards the errno and reads as "could not
        // start". Every bounded-run caller inherits that, so the retry
        // belongs in the wrapper, not at each call site.
        // trace:BUG-1735 | ai:claude
        .spawn_retrying_etxtbsy()
    {
        Ok(child) => child,
        Err(_) => return BoundedCommandOutput::SpawnFailed,
    };
    // Only used to target `killpg` below; on non-unix targets nothing reads
    // it, so it is cfg-gated too rather than left as a dead binding.
    #[cfg(unix)]
    let pid = child.id();
    let mut stdout_pipe = child.stdout.take().expect("stdout was piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr was piped");
    let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        let _ = stdout_tx.send(buf);
    });
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        let _ = stderr_tx.send(buf);
    });
    let start = std::time::Instant::now();
    let mut killed = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    killed = true;
                    #[cfg(unix)]
                    kill_process_group(pid);
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            // BUG-1288 fix-up: a `try_wait` error leaves the child un-reaped
            // exactly like the timeout branch above — kill and wait it here
            // too, or it leaks as an orphan/zombie every time this arm is
            // hit instead of only on the timeout path.
            // trace:BUG-1288 | ai:claude
            Err(_) => {
                killed = true;
                #[cfg(unix)]
                kill_process_group(pid);
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    // Bounded on the kill path (short — the group is already dead or dying,
    // this is only a safety net for a descendant that escaped it), generous
    // on the normal-exit path (the child already closed its own pipe ends;
    // this bound exists so a hypothetical stuck reader still can't hang the
    // caller forever, not because it is expected to be hit).
    let read_wait = if killed {
        std::time::Duration::from_millis(500)
    } else {
        std::time::Duration::from_secs(30)
    };
    let stdout = stdout_rx.recv_timeout(read_wait).unwrap_or_default();
    let stderr = stderr_rx.recv_timeout(read_wait).unwrap_or_default();
    match status {
        Some(status) => BoundedCommandOutput::Completed(std::process::Output {
            status,
            stdout,
            stderr,
        }),
        None => BoundedCommandOutput::TimedOut,
    }
}

/// BUG-1594: the message a forge lookup carries when the scoped
/// [`with_forge_lookup_timeout`] ceiling killed it. Rendered by `aida show`
/// as "timed out" rather than a generic unreachable so the output says
/// exactly why the PR/MR state is unknown (PRIN-5).
// trace:BUG-1594 | ai:claude
pub(crate) const FORGE_LOOKUP_TIMED_OUT: &str = "forge lookup timed out";

thread_local! {
    // trace:BUG-1594 | ai:claude
    static FORGE_LOOKUP_TIMEOUT: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

/// BUG-1594: run `f` with every open-change forge lookup it makes (`gh pr
/// list` / `glab api`) bounded by `timeout`. Scoped rather than global so
/// interactive read surfaces (`aida show`) get a hard ceiling while the
/// orchestrator's phase probes keep their existing unbounded semantics.
// trace:BUG-1594 | ai:claude
pub(crate) fn with_forge_lookup_timeout<T>(
    timeout: std::time::Duration,
    f: impl FnOnce() -> T,
) -> T {
    let prev = FORGE_LOOKUP_TIMEOUT.with(|c| c.replace(Some(timeout)));
    struct Restore(Option<std::time::Duration>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prev = self.0;
            FORGE_LOOKUP_TIMEOUT.with(|c| c.set(prev));
        }
    }
    let _restore = Restore(prev);
    f()
}

/// BUG-1594: spawn a forge-CLI lookup honoring the scoped
/// [`with_forge_lookup_timeout`] ceiling. `Ok(None)` means the call was
/// killed at the ceiling; `Err` is a spawn failure; with no scoped ceiling
/// this is exactly `Command::output()`.
// trace:BUG-1594 | ai:claude
pub(crate) fn forge_lookup_output(
    mut cmd: std::process::Command,
) -> std::io::Result<Option<std::process::Output>> {
    match FORGE_LOOKUP_TIMEOUT.with(|c| c.get()) {
        // trace:BUG-1735 | ai:claude
        None => cmd.output_retrying_etxtbsy().map(Some),
        Some(timeout) => {
            let started = std::time::Instant::now();
            match command_output_with_timeout(cmd, timeout) {
                Some(out) => Ok(Some(out)),
                None if started.elapsed() >= timeout => Ok(None),
                None => Err(std::io::Error::other("could not spawn the forge CLI")),
            }
        }
    }
}

/// Resolve the `glab` (GitLab CLI) binary, mirroring `resolve_gh_binary`'s
/// PATH-walk + sanity-spawn (BUG-74/79). Wired into the forge call sites in
/// follow-on STORY-621 slices. trace:STORY-621 | ai:claude
#[allow(dead_code)] // wired into call sites in follow-on STORY-621 slices
pub(crate) fn resolve_glab_binary() -> Option<std::path::PathBuf> {
    resolve_forge_binary("glab", "AIDA_TEST_GLAB_BINARY", "AIDA_DEBUG_GLAB")
}

#[cfg(test)]
#[path = "tests/forge_binary_resolution_tests.rs"]
mod forge_binary_resolution_tests;

#[cfg(all(test, unix))]
mod story1163_forge_dispatch_tests {
    use super::*;
    use tempfile::TempDir;

    fn gitlab_project() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let aida = tmp.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        std::fs::write(aida.join("config.toml"), "[forge]\nprovider = \"gitlab\"\n").unwrap();
        tmp
    }

    #[test]
    fn detect_open_pr_for_branch_dispatches_gitlab_to_glab() {
        let project = gitlab_project();
        let bin_dir = TempDir::new().unwrap();
        let log = bin_dir.path().join("glab.log");
        let glab = bin_dir.path().join("glab");
        crate::test_exec::write_executable(
            &glab,
            &format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"--version\" ]; then echo glab fake; exit 0; fi\n\
                 printf '%s\\n' \"$*\" >> '{}'\n\
                 printf '%s\\n' '[{{\"iid\":7,\"web_url\":\"https://gitlab.example.com/g/p/-/merge_requests/7\",\"source_branch\":\"feature/x\",\"target_branch\":\"main\",\"title\":\"GitLab MR\"}}]'\n",
                log.display()
            ),
        );
        let _g = crate::test_env::EnvVarGuard::set("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap());

        let found = match detect_open_pr_for_branch(project.path(), "feature/x") {
            PrLookup::Found(pr) => pr,
            _ => panic!("expected GitLab MR lookup"),
        };

        assert_eq!(found.number, 7);
        assert_eq!(found.head_branch.as_deref(), Some("feature/x"));
        let logged = std::fs::read_to_string(log).unwrap();
        assert!(logged.contains("api -X GET projects/:id/merge_requests"));
        assert!(logged.contains("-f source_branch=feature/x"));
        assert!(logged.contains("-f state=opened"));
    }

    #[test]
    fn diff_change_dispatches_gitlab_to_glab() {
        let project = gitlab_project();
        let bin_dir = TempDir::new().unwrap();
        let log = bin_dir.path().join("glab.log");
        let glab = bin_dir.path().join("glab");
        crate::test_exec::write_executable(
            &glab,
            &format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"--version\" ]; then echo glab fake; exit 0; fi\n\
                 printf '%s\\n' \"$*\" >> '{}'\n\
                 exit 0\n",
                log.display()
            ),
        );
        let _g = crate::test_env::EnvVarGuard::set("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap());

        crate::forge::forge_for_kind(project.path(), crate::forge::ForgeKind::GitLab)
            .diff_change(7)
            .expect("GitLab diff should dispatch through glab");

        let logged = std::fs::read_to_string(log).unwrap();
        assert!(logged.contains("mr diff 7"));
    }
}

/// True when `path` exists as a file and is executable by the current
/// process. On Windows we just check existence (the PATHEXT-aware Rust
/// spawn handles the rest); on Unix we check the executable bit on the
/// metadata. trace:BUG-74 | ai:claude
pub(crate) fn is_executable(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        true
    }
}

/// Format a `gh` spawn failure for a `PrLookup::GhFailed`. The OS reports
/// ENOENT against the *binary* when the spawn's working directory has been
/// removed — even though the binary is perfectly fine. `aida session end`
/// removes the session worktree, so this case is common; say so plainly
/// rather than letting the message read as "gh is missing".
/// trace:BUG-107 | ai:claude
pub(crate) fn gh_spawn_error(
    gh_bin: &std::path::Path,
    cwd: &std::path::Path,
    e: &std::io::Error,
) -> String {
    if !cwd.is_dir() {
        format!(
            "working directory `{}` no longer exists — the `{}` binary is fine \
             (os reports the missing cwd as ENOENT on the spawn): {}",
            cwd.display(),
            gh_bin.display(),
            e
        )
    } else {
        format!(
            "spawn `{}` (cwd `{}`): {}",
            gh_bin.display(),
            cwd.display(),
            e
        )
    }
}

/// Run `gh pr list <filter> --limit 1 --json number,title,url,headRefName` and parse the
/// single result line into a [`PrLookup`]. The shared core behind the
/// branch-keyed and spec-keyed PR lookups — each caller supplies only its
/// distinguishing `<filter>` args (`--head <branch>` / `--search <query>`
/// plus `--state`).
///
/// BUG-74: resolves `gh` via an explicit PATH walk + absolute-path fallback
/// rather than `Command::new("gh")`, because a child Rust process can see a
/// PATH that omits the user's gh install dir. BUG-107: a spawn `NotFound`
/// after the binary resolved means a removed working directory far more
/// often than a missing binary — `gh_spawn_error` names the real culprit.
/// AIDA_DEBUG_GH=1 surfaces the binary search.
/// trace:STORY-66 BUG-72 BUG-74 BUG-107 BUG-223 | ai:claude
pub(crate) fn gh_pr_list_first(project_root: &std::path::Path, filter: &[&str]) -> PrLookup {
    let gh_bin = match resolve_forge_cli(crate::forge::ForgeKind::GitHub) {
        Some(p) => p,
        None => return PrLookup::GhMissing,
    };
    let mut args: Vec<&str> = vec!["pr", "list"];
    args.extend_from_slice(filter);
    args.extend_from_slice(&[
        "--limit",
        "1",
        "--json",
        // trace:BUG-1812 | ai:antigravity
        "number,title,url,headRefName,isDraft",
        "-q",
        r#".[] | "\(.number)\t\(.title)\t\(.url)\t\(.headRefName)""#,
    ]);
    // BUG-1594: honor a caller-scoped ceiling (`aida show`) so one slow
    // `gh` call cannot stall an interactive read. trace:BUG-1594 | ai:claude
    let mut gh_cmd = std::process::Command::new(&gh_bin);
    gh_cmd.current_dir(project_root).args(&args);
    let out = match forge_lookup_output(gh_cmd) {
        Ok(Some(o)) => o,
        Ok(None) => return PrLookup::GhUnreachable(FORGE_LOOKUP_TIMED_OUT.to_string()),
        Err(e) => return PrLookup::GhFailed(gh_spawn_error(&gh_bin, project_root, &e)),
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if gh_stderr_is_network_error(&stderr) {
            return PrLookup::GhUnreachable(if stderr.is_empty() {
                format!("gh exited {}", out.status)
            } else {
                stderr
            });
        }
        return PrLookup::GhFailed(if stderr.is_empty() {
            format!("gh exited {}", out.status)
        } else {
            stderr
        });
    }
    parse_gh_pr_line(&String::from_utf8_lossy(&out.stdout))
}

/// Classify a `gh` stderr line as a transient network error (the GH API
/// was unreachable) rather than an auth/parse/other failure. Used by
/// [`gh_pr_list_first`] to split `PrLookup::GhFailed` into the
/// `GhUnreachable` variant so the orchestrator can report *Inconclusive*
/// instead of conflating "no PR" with "can't reach the API".
///
/// Patterns are taken from real-world `gh` output (the Go HTTP client +
/// gh's own diagnostic suffixes). The match is conservative —
/// case-insensitive substring matching against a small allow-list — so an
/// auth/parse failure never gets re-classified as a transient network
/// blip. trace:BUG-257 | ai:claude
pub(crate) fn gh_stderr_is_network_error(stderr: &str) -> bool {
    // external-prose-classifier: gh_stderr_is_network_error
    aida_core::external_tool_output::contains_any_case_insensitive(
        stderr,
        aida_core::external_tool_output::GH_NETWORK_TRANSIENT,
    )
}

/// BUG-266: classify a headless `claude -p` JSONL log as evidence the
/// upstream Anthropic API took the session out (a transient outage), not
/// that the implementer attempted the work and failed. The orchestrator
/// uses the returned reason to flip phase 1 from *failed* to *Inconclusive*
/// (the BUG-257 path), so a 529 / 5xx / stream-timeout no longer marks a
/// spec as failed-by-implementer.
///
/// Patterns are taken from real Anthropic error text plus the proxy /
/// upstream connectivity families that surface in `claude -p`'s output.
/// Match is anchored and conservative — `Overloaded` and `API Error: 5\d\d`
/// are Anthropic's own wording, `upstream connect error` is the Envoy /
/// proxy convention, `stream timeout` is the SSE-stream variant. Returns
/// the first matching diagnostic line so the orchestrator's epilogue can
/// echo what the substrate said. trace:BUG-266 | ai:claude
pub(crate) fn claude_log_indicates_api_outage(content: &str) -> Option<String> {
    // external-prose-classifier: claude_log_indicates_api_outage
    if content.is_empty() {
        return None;
    }
    for line in content.lines() {
        // The log is JSON stream-output; the human-readable phrase appears
        // verbatim inside an event payload, so substring matching on each
        // line works without parsing JSON. Skip lines that have no error
        // shape — keeps the false-positive surface small.
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        // Anthropic's own 5xx wording — `API Error: 5\d\d` covers 500-599.
        if let Some(idx) = lower.find(aida_core::external_tool_output::CLAUDE_API_5XX_PREFIX) {
            let tail = &lower[idx + aida_core::external_tool_output::CLAUDE_API_5XX_PREFIX.len()..];
            let digits_after = tail.chars().take(2).filter(|c| c.is_ascii_digit()).count();
            if digits_after == 2 {
                return Some(reason_excerpt(trimmed, idx));
            }
        }
        // Anthropic's explicit overload signal — emitted with 529s and the
        // capacity-shed envelope. Conservative: anchor on the full word.
        if let Some(idx) = aida_core::external_tool_output::CLAUDE_API_OUTAGE
            .iter()
            .find_map(|marker| lower.find(marker))
        {
            return Some(reason_excerpt(trimmed, idx));
        }
    }
    None
}

/// Capture a short diagnostic excerpt centered on the matched phrase — the
/// orchestrator's epilogue is one line, so we cap at ~160 chars rather
/// than echoing a full assistant turn. Pure helper for the classifier.
pub(crate) fn reason_excerpt(line: &str, match_start: usize) -> String {
    const MAX: usize = 160;
    if line.len() <= MAX {
        return line.to_string();
    }
    // Anchor the excerpt window around the match so the matched phrase is
    // always visible, then trim to char boundaries to avoid splitting UTF-8.
    let begin = match_start.saturating_sub(20);
    let mut end = (begin + MAX).min(line.len());
    while !line.is_char_boundary(end) && end > begin {
        end -= 1;
    }
    let mut start = begin;
    while !line.is_char_boundary(start) && start < end {
        start += 1;
    }
    format!("…{}…", &line[start..end])
}

/// BUG-266: locate the headless implementer's JSONL log by the session UUID
/// the orchestrator minted (the filename pattern is
/// `<branch>-<session-uuid>.jsonl` under `.aida/headless-logs/`). Glob by
/// suffix because the branch isn't known at the failure point in
/// `run_implementer` (it is discovered AFTER the implementer exits, via the
/// session lease — but a failed-to-spawn or early-crashed implementer may
/// not have a lease yet, so we can't depend on the branch). Returns the log
/// contents on success; `None` if the directory doesn't exist, no matching
/// file is found, or the file can't be read. Best-effort by design —
/// classification is a refinement, not a gate. trace:BUG-266 | ai:claude
pub(crate) fn read_headless_log_for_session(
    project_root: &std::path::Path,
    session_uuid: &str,
) -> Option<String> {
    let path = headless_log_path_for_session(project_root, session_uuid)?;
    std::fs::read_to_string(&path).ok()
}

// trace:STORY-998 | ai:codex
pub(crate) fn headless_log_path_for_session(
    project_root: &std::path::Path,
    session_uuid: &str,
) -> Option<std::path::PathBuf> {
    let dir = project_root.join(".aida").join("headless-logs");
    let suffix = format!("-{session_uuid}.jsonl");
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(&suffix))
            .unwrap_or(false)
        {
            return Some(path);
        }
    }
    None
}

/// BUG-453: byte length of the headless session's JSONL log (the newest file
/// matching `-<session_id>.jsonl` under `.aida/headless-logs/`), or `None`. A
/// *growing* log means the session is alive and emitting events — reading,
/// thinking, or editing — even when it hasn't yet changed a worktree file. The
/// phase watchdog folds this into its progress signal so a productive-but-
/// currently-reading implementer is not false-killed as "no progress" (the
/// TASK-673 symptom: it wrote 503 lines, then spent its final window reading
/// tests in a 20k-line file and the worktree-only signature went static).
/// trace:BUG-453 | ai:claude
pub(crate) fn headless_log_len(project_root: &std::path::Path, session_id: &str) -> Option<u64> {
    let dir = project_root.join(".aida").join("headless-logs");
    let suffix = format!("-{session_id}.jsonl");
    let mut newest_len: Option<u64> = None;
    let mut newest_mtime = std::time::UNIX_EPOCH;
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let is_match = entry
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(&suffix))
            .unwrap_or(false);
        if !is_match {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            let m = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            if m >= newest_mtime {
                newest_mtime = m;
                newest_len = Some(meta.len());
            }
        }
    }
    newest_len
}

/// BUG-1716: the two log shapes that witness a headless vendor which emitted
/// nothing. `ZeroBytes` is BUG-826's original signature — the vendor started,
/// created its JSONL log, and died before writing a single stream event.
/// `Missing` is its never-started sibling: the vendor rejected its own launch
/// (an argv/usage error, e.g. the duplicated bypass flag that killed the
/// TASK-204 drive in 2.7s) and exited before even creating the log file. The
/// zero-byte check alone let the `Missing` shape fall through to the
/// work-failure path, where it was classified `NoPr`/`tool-exit`, burned the
/// whole STORY-975 transient budget, and parked a healthy spec
/// `NeedsAttention` behind the live empty lease.
// trace:BUG-1716 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmptyLaunchLog {
    /// No session log file exists at all.
    Missing,
    /// A session log file exists and is zero bytes.
    ZeroBytes,
}

/// BUG-1716: classify the session log's evidence about the launch. `None`
/// means the log has content — the vendor demonstrably ran.
// trace:BUG-1716 | ai:claude
pub(crate) fn headless_launch_log_evidence(
    project_root: &std::path::Path,
    session_id: &str,
) -> Option<EmptyLaunchLog> {
    match headless_log_len(project_root, session_id) {
        None => Some(EmptyLaunchLog::Missing),
        Some(0) => Some(EmptyLaunchLog::ZeroBytes),
        Some(_) => None,
    }
}

/// BUG-1716: is a non-zero headless vendor exit an *empty launch* (the agent
/// never started) rather than implementer work that failed?
///
/// A zero-byte log is taken at face value, exactly as BUG-826 shipped it. A
/// MISSING log claims only the TASK-204 shape — the child DID claim this
/// session's lease (`session_lease` is `Some`), so without this lane the
/// lease discovery below would succeed and the never-started vendor would be
/// classified as implementer work that failed. Two boundaries keep it narrow:
///
/// - `session_lease == None` (no lease for this session) is NOT an empty
///   launch: the child never even claimed, and the BUG-1629/BUG-1769
///   lost-child recovery is the authoritative owner of that shape — it reads
///   the child's own recorded refusal, substrate-verifies a spec that
///   advanced during the run, and spends exactly one pinned replacement
///   launch. Swallowing it here is what broke the whole
///   `bug_1629_phase1_recovery_tests` contract on the first round of this
///   spec (and BUG-1776 is that same misrouting seen from macOS).
/// - `session_lease == Some(true)` (the lease branch carries commits) is NOT
///   an empty launch: log routing could be broken while a real implementer
///   worked, and the failure must keep falling through to the substrate
///   verification (BUG-1140), never into the lease-releasing lane.
// trace:BUG-1716 | ai:claude
pub(crate) fn empty_launch_decision(
    evidence: Option<EmptyLaunchLog>,
    session_lease: Option<bool>,
) -> bool {
    match evidence {
        Some(EmptyLaunchLog::ZeroBytes) => true,
        Some(EmptyLaunchLog::Missing) => session_lease == Some(false),
        None => false,
    }
}

/// BUG-826: bounded launch-retry schedule for empty-log vendor death. Kept
/// separate from the generic transient-work retry: this retry happens before
/// phase 1 returns a failure and before the spec is parked.
// trace:BUG-826 | ai:codex
pub(crate) fn phase1_empty_launch_retry_delay(attempts_used: usize) -> Option<std::time::Duration> {
    [30_u64, 60]
        .get(attempts_used)
        .copied()
        .map(std::time::Duration::from_secs)
}

#[cfg(not(test))]
pub(crate) fn phase1_empty_launch_retry_sleep(delay: std::time::Duration) {
    std::thread::sleep(delay);
}

#[cfg(test)]
pub(crate) fn phase1_empty_launch_retry_sleep(_delay: std::time::Duration) {}

/// TASK-298: scan one headless `claude -p --output-format stream-json` JSONL
/// *line* for a hard "the session bailed at a gate" signal and, when present,
/// return a specific human-readable reason. SPIKE-7 found `claude -p`'s exit
/// code is unreliable (it exits 0 even when it abandoned the work at a
/// permission prompt), so the real completion signal lives in the stream:
///
///   - a non-empty `permission_denials` array → the model wanted a tool the
///     non-interactive run could not grant; in `--no-human` mode there is no
///     human to approve it, so the run is silently stuck. We surface the first
///     denied tool name.
///   - a top-level `is_error: true` (on any event, including the terminal
///     `result`) → the run reported an error envelope; we surface its
///     `result`/`subtype` detail.
///
/// Pure and FULLY ISOLATED: takes a single JSONL line, returns
/// `Some(reason)` when the line carries a stall signal, `None` otherwise.
/// Blank or non-JSON lines are `None` (the orchestrator's log is well-formed
/// JSONL, but a partially-flushed final line must never trip a false stall).
/// The watchdog folds this over the log so a `--no-human` drain wedged at a
/// permission gate is detected and failed instead of hanging until the
/// no-progress / ceiling timeout. trace:TASK-298 | ai:claude
pub(crate) fn headless_line_permission_stall(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(trimmed).ok()?;

    // Permission denials are the precise SPIKE-7 signal — surface them first
    // with the offending tool name(s).
    if let Some(denials) = parsed
        .get("permission_denials")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
    {
        let tool = denials
            .iter()
            .find_map(|d| {
                d.get("tool_name")
                    .or_else(|| d.get("tool"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("an unknown tool");
        let extra = if denials.len() > 1 {
            format!(" (+{} more)", denials.len() - 1)
        } else {
            String::new()
        };
        return Some(format!(
            "headless Claude bailed at the permission gate for tool {tool}{extra}"
        ));
    }

    // An error envelope — `is_error: true` on any event (the terminal
    // `result` event is the common carrier, but be liberal). Exit code 0
    // cannot be trusted, so this is the authoritative failure flag.
    if parsed.get("is_error").and_then(|v| v.as_bool()) == Some(true) {
        let detail = parsed
            .get("result")
            .and_then(|v| v.as_str())
            .map(first_nonempty_line)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                parsed
                    .get("subtype")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "error".to_string());
        return Some(format!("headless Claude reported is_error: {detail}"));
    }

    None
}

/// TASK-298: fold [`headless_line_permission_stall`] over a whole headless log
/// body, returning the first line's stall reason or `None`. Best-effort, like
/// the sibling [`claude_log_indicates_api_outage`]: the classification is a
/// refinement that lets the watchdog fail fast, not a gate.
/// trace:TASK-298 | ai:claude
pub(crate) fn headless_log_permission_stall(content: &str) -> Option<String> {
    content.lines().find_map(headless_line_permission_stall)
}

/// First non-empty trimmed line of a string, or the empty string. Small pure
/// helper so a multi-line `result` detail collapses to one log-friendly row.
/// trace:TASK-298 | ai:claude
pub(crate) fn first_nonempty_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

pub(crate) struct TextQuestionPunt {
    pub(crate) detail: String,
    pub(crate) lean: String,
}

/// BUG-354 / BUG-374: classify the headless implementer's final `result` text
/// as the plain-markdown variant of AskUserQuestion: the model asks the
/// operator to choose/confirm a path, calls no tool, exits success, and opens
/// no PR. We only scan the terminal result text and require both a question
/// mark and decision-fork phrasing so normal implementation summaries that
/// mention questions in passing keep the existing NoPr failure path.
pub(crate) fn pending_text_question_from_headless_log(content: &str) -> Option<TextQuestionPunt> {
    let result = reviewer_summary::parse_result_event(content)?;
    if result.is_error {
        return None;
    }
    let text = result.result_text?;
    pending_text_question_from_result_text(&text)
}

pub(crate) fn pending_text_question_from_result_text(text: &str) -> Option<TextQuestionPunt> {
    let trimmed = text.trim();
    if !trimmed.contains('?') {
        return None;
    }
    // BUG-462: scan EVERY question sentence (not just the first `?`) for
    // decision-fork phrasing. A confident headless implementer phrases its
    // "which path?" infinitely many ways and routinely buries the operative
    // question after an options block, so the first `?` in the text is often a
    // rhetorical aside. An exact-phrase allowlist checked against only the first
    // question is the wrong shape — it let "Which way do you want to go?" (the
    // TASK-457 drain) fall through to a misleading phase-1 NoPr failure instead
    // of advisor-tier routing. trace:BUG-462 | ai:claude
    let questions = all_question_sentences(trimmed);
    let question = questions
        .iter()
        .find(|q| question_has_fork_marker(&q.to_ascii_lowercase()))
        .cloned()
        // Options-block fallback: even when no phrase marker matches, an
        // enumerated multi-option block (`A)`/`B)`, `1.`/`2.`, `Option A/B`)
        // paired with a trailing question is an unambiguous decision fork. Bias
        // toward punt here: a spec parked and routed to the headless advisor
        // beats a hard phase-1 NoPr failure with a misleading `/aida-pr` hint —
        // "a paused spec beats a guessed one". trace:BUG-462 | ai:claude
        .or_else(|| {
            if has_options_block(trimmed) {
                questions.last().cloned()
            } else {
                None
            }
        })?;
    let recommendation = recommendation_line(trimmed).unwrap_or_else(|| {
        "review the punted analysis and choose the implementation path".to_string()
    });
    Some(TextQuestionPunt {
        detail: format!(
            "headless implementer exited without opening a PR after asking a plain-text decision question: {}",
            bounded_text(&question, 220),
        ),
        lean: bounded_text(&recommendation, 220),
    })
}

/// Decision-fork phrasing markers for a single (already lowercased) question
/// sentence. Broadened under BUG-462 beyond the original BUG-354/BUG-374 set so
/// novel phrasings ("which way", "do you want", "would you like") still route to
/// the advisor tier. Kept narrow enough that an ordinary summary question
/// ("does the parser handle aliases?") does not match. trace:BUG-462 | ai:claude
pub(crate) fn question_has_fork_marker(question_lower: &str) -> bool {
    const QUESTION_FORK_MARKERS: &[&str] = &[
        "which path",
        "which option",
        "which approach",
        "which one",
        "which way",
        "which direction",
        "do you want",
        "would you like",
        "want me to",
        "should i",
        "should we",
        "how would you like",
        "how should",
        "need you to choose",
        "go with",
        "do you prefer",
        "your call",
        "please confirm",
        "confirm and i'll proceed",
        "confirm and i’ll proceed",
        "confirm and i will proceed",
        "confirm this path",
        "confirm the path",
        "confirm option",
        "confirm approach",
    ];
    QUESTION_FORK_MARKERS
        .iter()
        .any(|marker| question_lower.contains(marker))
}

/// Every `?`-terminated sentence in `text`, whitespace-collapsed. Splits on
/// `.`/`!`/`?` so a fork question buried after rhetorical asides is still
/// surfaced (BUG-462 scans all of them, not just the first). trace:BUG-462
pub(crate) fn all_question_sentences(text: &str) -> Vec<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = Vec::new();
    let mut start = 0usize;
    // `?`/`.`/`!` are ASCII, so every index used here is a char boundary.
    for (i, b) in collapsed.bytes().enumerate() {
        match b {
            b'?' => {
                let sentence = collapsed[start..=i]
                    .trim_start_matches(['.', '!', '?', ' '])
                    .trim();
                if !sentence.is_empty() {
                    out.push(sentence.to_string());
                }
                start = i + 1;
            }
            b'.' | b'!' => start = i + 1,
            _ => {}
        }
    }
    out
}

/// True when `text` presents an enumerated *choice* — two or more lines that
/// start with a letter-labelled option (`Option A`, `A)`, `B:`). Deliberately
/// LETTER-only (not `1.`/`2.`): digit labels far more often enumerate completed
/// *steps* in a summary than mutually-exclusive choices, and the fallback fires
/// only when no phrase marker matched, so a numbered accomplishments list with a
/// trailing rhetorical question must not read as a fork. Line-anchored so inline
/// version strings like `v1.2.0` don't count. trace:BUG-462 | ai:claude
pub(crate) fn has_options_block(text: &str) -> bool {
    let mut enumerated = 0usize;
    for raw in text.lines() {
        // strip a leading markdown bold/bullet so "- **A)** ..." still counts
        let line = raw
            .trim_start()
            .trim_start_matches(['-', '*', ' '])
            .trim_start_matches("**");
        if line.to_ascii_lowercase().starts_with("option ") || starts_with_letter_label(line) {
            enumerated += 1;
        }
    }
    enumerated >= 2
}

/// A single letter (`A`–`H`, any case) followed by `)`, `.`, or `:` — the start
/// of a letter-labelled option line. trace:BUG-462 | ai:claude
pub(crate) fn starts_with_letter_label(line: &str) -> bool {
    let mut chars = line.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let Some(sep) = chars.next() else {
        return false;
    };
    matches!(first, 'A'..='H' | 'a'..='h') && matches!(sep, ')' | '.' | ':')
}

pub(crate) fn recommendation_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("recommend") || lower.starts_with("lean:") || lower.contains("my lean")
        })
        .map(str::to_string)
}

pub(crate) fn bounded_text(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.len() <= max {
        return text;
    }
    let mut end = max.saturating_sub(1).min(text.len());
    while !text.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// STORY-516: reverse of `change_lookup_from_pr_lookup` — convert a forge
/// `ChangeLookup` back to the orchestrator's `PrLookup` so the `*_via_forge`
/// helpers are a pure name-swap at their call sites (match arms stay PrLookup).
/// trace:STORY-516 | ai:claude
pub(crate) fn pr_lookup_from_change_lookup(c: crate::forge::ChangeLookup) -> PrLookup {
    match c {
        crate::forge::ChangeLookup::Found(r) => PrLookup::Found(OpenPrInfo {
            number: r.id,
            title: r.title.unwrap_or_default(),
            url: r.url,
            head_branch: (!r.branch.is_empty()).then_some(r.branch),
        }),
        crate::forge::ChangeLookup::NoChange => PrLookup::NoOpenPr,
        crate::forge::ChangeLookup::CliMissing => PrLookup::GhMissing,
        crate::forge::ChangeLookup::CliFailed(s) => PrLookup::GhFailed(s),
        crate::forge::ChangeLookup::Unreachable(s) => PrLookup::GhUnreachable(s),
    }
}

// STORY-1163: inverse adapter for legacy callers that still consume
// `ChangeLookup` while the central PR helper now dispatches by forge.
// trace:STORY-1163 | ai:codex
pub(crate) fn change_lookup_from_pr_lookup_for_branch(
    p: PrLookup,
    branch_hint: &str,
) -> crate::forge::ChangeLookup {
    match p {
        PrLookup::Found(pr) => crate::forge::ChangeLookup::Found(crate::forge::ChangeRef {
            id: pr.number,
            url: pr.url,
            branch: pr
                .head_branch
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| branch_hint.to_string()),
            base: String::new(),
            title: Some(pr.title),
        }),
        PrLookup::NoOpenPr => crate::forge::ChangeLookup::NoChange,
        PrLookup::GhMissing => crate::forge::ChangeLookup::CliMissing,
        PrLookup::GhFailed(s) => crate::forge::ChangeLookup::CliFailed(s),
        PrLookup::GhUnreachable(s) => crate::forge::ChangeLookup::Unreachable(s),
    }
}

/// STORY-516: forge-routed spec-search open-PR lookup (BUG-223 fallback). GitHub
/// delegates to `detect_open_pr_for_spec`. trace:STORY-516 | ai:claude
pub(crate) fn detect_open_pr_for_spec_via_forge(
    project_root: &std::path::Path,
    spec: &str,
) -> PrLookup {
    detect_open_pr_for_spec(project_root, spec)
}

/// STORY-516: forge-routed merged-PR-for-branch lookup. GitHub delegates to
/// `detect_merged_pr_for_branch`. trace:STORY-516 | ai:claude
pub(crate) fn detect_merged_pr_for_branch_via_forge(
    project_root: &std::path::Path,
    branch: &str,
) -> PrLookup {
    detect_merged_pr_for_branch(project_root, branch)
}

/// STORY-516: forge-routed branch lookup — the view-op entry point the call
/// sites use instead of `detect_open_pr_for_branch` directly, so a GitLab /
/// pure-git repo goes through its own provider. GitHubForge delegates back to
/// `detect_open_pr_for_branch`, so behaviour on GitHub is unchanged. The
/// provider returns `Result<ChangeLookup>`; GitHub never errs (it adapts a
/// PrLookup), and a provider Err (GitLab stub) collapses to `CliFailed` so
/// callers keep a single 5-state match. trace:STORY-516 | ai:claude
pub(crate) fn change_lookup_for_branch(
    project_root: &std::path::Path,
    branch: &str,
) -> crate::forge::ChangeLookup {
    change_lookup_from_pr_lookup_for_branch(detect_open_pr_for_branch(project_root, branch), branch)
}

// trace:BUG-876 | ai:codex
pub(crate) fn change_lookup_for_spec(
    project_root: &std::path::Path,
    spec: &str,
) -> crate::forge::ChangeLookup {
    crate::forge::forge_for(project_root)
        .change_for_spec(spec)
        .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}")))
}

/// Look up a single open PR keyed on `branch`. The richer [`PrLookup`]
/// return shape lets callers print an honest skip reason (no PR yet / gh
/// missing / gh failed) instead of collapsing every case to `None`.
/// trace:STORY-66 BUG-72 BUG-74 | ai:claude
pub(crate) fn detect_open_pr_for_branch(project_root: &std::path::Path, branch: &str) -> PrLookup {
    let forge_kind = crate::forge::resolve_forge_kind(project_root);
    pr_lookup_from_change_lookup(
        crate::forge::forge_for_kind(project_root, forge_kind)
            .change_for_branch(branch)
            .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}"))),
    )
}

// STORY-1163: raw GitHub lookup used by GitHubForge after the public helper
// became forge-dispatched. The `gh pr list` argv/parsing stay unchanged.
// trace:STORY-1163 | ai:codex
pub(crate) fn detect_open_pr_for_branch_github(
    project_root: &std::path::Path,
    branch: &str,
) -> PrLookup {
    gh_pr_list_first(project_root, &["--head", branch, "--state", "open"])
}

/// BUG-257: the three states `probe_branch_on_origin` can settle on when the
/// orchestrator narrows a `PrLookup::GhUnreachable` with `git ls-remote`. The
/// existing `aida_core::git_ops::remote_branch_exists` collapses `Absent` and
/// `LsRemoteFailed` into one `false` — exactly the conflation BUG-257 must
/// avoid here, so we keep this enum and the probe local to the orchestrator
/// caller. trace:BUG-257 | ai:claude
pub(crate) enum BranchOriginProbe {
    /// `git ls-remote` reported the branch is on origin — a PR may exist.
    Present,
    /// `git ls-remote` ran cleanly and reported the branch is absent — no
    /// PR can exist regardless of GH-API state.
    Absent,
    /// `git ls-remote` itself failed (network down end-to-end, auth, ...).
    /// Cannot narrow the diagnosis at all.
    LsRemoteFailed,
}

/// BUG-257: narrow a `PrLookup::GhUnreachable` outcome with `git ls-remote`
/// against `origin`. The git protocol is HTTPS-API-independent, so when the
/// GH API is down `ls-remote` often still works — and a missing branch
/// proves no PR can exist (the implementer never pushed). When the branch
/// IS present we still cannot tell whether a PR was opened against it
/// without the API; that case becomes Inconclusive. When `ls-remote` also
/// fails, the orchestrator has no way to tell and stays Inconclusive.
/// Best-effort: short timeout so a hung connection never blocks phase 1.
/// trace:BUG-257 | ai:claude
pub(crate) fn probe_branch_on_origin(
    project_root: &std::path::Path,
    branch: &str,
) -> BranchOriginProbe {
    let out = std::process::Command::new("git")
        .current_dir(project_root)
        .args([
            "-c",
            // Keep the probe bounded — a hung HTTPS dial against origin
            // would otherwise re-introduce the freeze BUG-257 is fixing.
            "http.lowSpeedLimit=1000",
            "-c",
            "http.lowSpeedTime=10",
            "ls-remote",
            "--exit-code",
            "--heads",
            "origin",
            &format!("refs/heads/{branch}"),
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => BranchOriginProbe::Present,
        // `git ls-remote --exit-code` exits 2 when the ref is missing on a
        // remote it could otherwise reach. Distinguish that from a true
        // ls-remote failure (network down, auth) by checking the exit code.
        Ok(o) => match o.status.code() {
            Some(2) => BranchOriginProbe::Absent,
            _ => BranchOriginProbe::LsRemoteFailed,
        },
        Err(_) => BranchOriginProbe::LsRemoteFailed,
    }
}

pub(crate) fn spec_branch_slug(spec: &str) -> String {
    spec.trim().to_ascii_lowercase().replace([' ', '_'], "-")
}

pub(crate) fn branch_name_references_spec(branch: &str, spec: &str) -> bool {
    let slug = spec_branch_slug(spec);
    let branch = branch
        .trim()
        .trim_start_matches("origin/")
        .to_ascii_lowercase();
    if slug.is_empty() {
        return false;
    }
    // BUG-1525 review fix: the slug must be a whole segment of the branch
    // name. A raw substring let BUG-15 match `bug-150-work` and BUG-152
    // match `claude/bug-1525`. A boundary is start/end or one of `/-._`,
    // and the character after the slug must not be a digit or letter.
    // trace:BUG-1525 | ai:claude
    let is_sep = |c: char| matches!(c, '/' | '-' | '.' | '_');
    branch.match_indices(&slug).any(|(at, m)| {
        let before_ok = branch[..at].chars().next_back().is_none_or(is_sep);
        let after_ok = branch[at + m.len()..].chars().next().is_none_or(is_sep);
        before_ok && after_ok
    })
}

#[cfg(test)]
mod bug_1525_branch_match_tests {
    use super::branch_name_references_spec as m;

    // trace:BUG-1525 | ai:claude
    #[test]
    fn branch_match_is_whole_segment_only() {
        assert!(m("bug-15-work", "BUG-15"));
        assert!(m("claude/bug-15", "BUG-15"));
        assert!(m("origin/bug-15", "BUG-15"));
        assert!(m("bug-15", "BUG-15"));
        assert!(!m("bug-150-work", "BUG-15"));
        assert!(!m("claude/bug-1525", "BUG-152"));
        assert!(!m("xbug-15", "BUG-15"));
    }
}

pub(crate) fn open_pr_commit_headlines_reference_spec(
    project_root: &std::path::Path,
    pr: u64,
    spec: &str,
) -> bool {
    let Some(gh_bin) = resolve_forge_cli(crate::forge::ForgeKind::GitHub) else {
        return false;
    };
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "view",
            &pr.to_string(),
            "--json",
            "commits",
            "-q",
            ".commits[].messageHeadline",
        ])
        .output_retrying_etxtbsy();
    let Ok(out) = out else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let wanted = spec.to_ascii_uppercase();
    String::from_utf8_lossy(&out.stdout).lines().any(|line| {
        extract_spec_ids_from_commit(line)
            .into_iter()
            .any(|id| id.eq_ignore_ascii_case(&wanted))
    })
}

pub(crate) fn detect_open_pr_for_spec_by_head_or_commit(
    project_root: &std::path::Path,
    spec: &str,
) -> PrLookup {
    let gh_bin = match resolve_forge_cli(crate::forge::ForgeKind::GitHub) {
        Some(p) => p,
        None => return PrLookup::GhMissing,
    };
    let spawned = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "100",
            "--json",
            "number,title,url,headRefName,isDraft",
            "-q",
            r#".[] | "\(.number)\t\(.title)\t\(.url)\t\(.headRefName)""#,
        ])
        .output_retrying_etxtbsy();
    let out = match spawned {
        Ok(o) => o,
        Err(e) => return PrLookup::GhFailed(gh_spawn_error(&gh_bin, project_root, &e)),
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if gh_stderr_is_network_error(&stderr) {
            return PrLookup::GhUnreachable(if stderr.is_empty() {
                format!("gh exited {}", out.status)
            } else {
                stderr
            });
        }
        return PrLookup::GhFailed(if stderr.is_empty() {
            format!("gh exited {}", out.status)
        } else {
            stderr
        });
    }

    let mut candidates: Vec<OpenPrInfo> = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let PrLookup::Found(pr) = parse_gh_pr_line(line) {
            candidates.push(pr);
        }
    }

    for pr in &candidates {
        if pr
            .head_branch
            .as_deref()
            .is_some_and(|b| branch_name_references_spec(b, spec))
        {
            return PrLookup::Found(OpenPrInfo {
                number: pr.number,
                title: pr.title.clone(),
                url: pr.url.clone(),
                head_branch: pr.head_branch.clone(),
            });
        }
    }
    for pr in candidates {
        if open_pr_commit_headlines_reference_spec(project_root, pr.number, spec) {
            return PrLookup::Found(pr);
        }
    }
    PrLookup::NoOpenPr
}

/// Fallback PR lookup for BUG-223/BUG-876: find an open PR that references
/// `spec`, used when branch/lease linkage is unavailable or stale. Search title
/// / body first, then open PR head branches (`story-818`) and commit headlines
/// carrying `(SPEC-ID)` trailers so releasing a lease cannot hide a pushed PR.
/// trace:BUG-223 | ai:claude
// trace:BUG-876 | ai:codex
pub(crate) fn detect_open_pr_for_spec(project_root: &std::path::Path, spec: &str) -> PrLookup {
    let forge_kind = crate::forge::resolve_forge_kind(project_root);
    pr_lookup_from_change_lookup(
        crate::forge::forge_for_kind(project_root, forge_kind)
            .change_for_spec(spec)
            .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}"))),
    )
}

// STORY-1163: raw GitHub spec lookup used by GitHubForge after the public helper
// became forge-dispatched. trace:STORY-1163 | ai:codex
pub(crate) fn detect_open_pr_for_spec_github(
    project_root: &std::path::Path,
    spec: &str,
) -> PrLookup {
    match gh_pr_list_first(project_root, &["--search", spec, "--state", "open"]) {
        PrLookup::NoOpenPr => detect_open_pr_for_spec_by_head_or_commit(project_root, spec),
        other => other,
    }
}

/// TASK-843: list ALL open PRs that reference `spec` (number + head branch),
/// not just the first. `detect_open_pr_for_spec_via_forge` returns at most one,
/// so a spec with >1 open PR (a reopened/duplicate) was resolved by a silent,
/// non-deterministic pick. This surfaces the full candidate set so the pure
/// [`integrate::select_canonical_pr`] policy can pick the newest-canonical PR
/// and report the rest. Best-effort: an empty vec when gh is missing/failing or
/// no PR references the spec — the caller falls back to the single-PR path.
/// trace:TASK-843 | ai:claude
pub(crate) fn all_open_prs_for_spec_via_forge(
    project_root: &std::path::Path,
    spec: &str,
) -> Vec<(u64, String)> {
    let gh_bin = match resolve_gh_binary() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "list",
            "--search",
            spec,
            "--state",
            "open",
            "--limit",
            "20",
            "--json",
            "number,headRefName",
            "-q",
            r#".[] | "\(.number)\t\(.headRefName)""#,
        ])
        .output_retrying_etxtbsy();
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (num, branch) = line.split_once('\t')?;
            let number = num.trim().parse::<u64>().ok()?;
            Some((number, branch.trim().to_string()))
        })
        .collect()
}

/// Look up the head branch of an open PR by number — used by the BUG-223
/// spec-id fallback to realign the orchestrator's branch after a swap the
/// worktree-HEAD reconciliation missed, so the CI / merge phases probe the
/// PR's actual head. Best-effort: `None` on any `gh` failure.
/// trace:BUG-223 | ai:claude
pub(crate) fn pr_head_branch(project_root: &std::path::Path, pr_number: u64) -> Option<String> {
    let gh_bin = resolve_gh_binary()?;
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "view",
            &pr_number.to_string(),
            "--json",
            "headRefName",
            "-q",
            ".headRefName",
        ])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

/// TASK-1230: the UNIQUE registered worktree checked out on `branch`, excluding
/// the main checkout and the store worktree. Returns None on zero or ambiguous
/// (>1) matches — the caller must never guess which of several to remove.
// trace:TASK-1230 | ai:claude
pub(crate) fn unique_worktree_on_branch(
    project_root: &std::path::Path,
    branch: &str,
    main_root: &std::path::Path,
    store_wt: &std::path::Path,
) -> Option<std::path::PathBuf> {
    if branch.is_empty() {
        return None;
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Porcelain blocks are blank-line-separated; each has a `worktree <path>`
    // line and (when on a branch) a `branch refs/heads/<name>` line.
    let mut matches: Vec<std::path::PathBuf> = Vec::new();
    let mut cur_path: Option<std::path::PathBuf> = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            cur_path = Some(std::path::PathBuf::from(p.trim()));
        } else if let Some(b) = line.strip_prefix("branch ") {
            let short = b.trim().strip_prefix("refs/heads/").unwrap_or(b.trim());
            if short == branch {
                if let Some(p) = &cur_path {
                    let is_protected = p == main_root
                        || p == store_wt
                        || p.canonicalize().ok() == main_root.canonicalize().ok()
                        || p.canonicalize().ok() == store_wt.canonicalize().ok();
                    if !is_protected {
                        matches.push(p.clone());
                    }
                }
            }
        }
    }
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

/// TASK-1230: count of commits on the worktree's HEAD that are NOT yet on the
/// pushed PR branch (`origin/<branch>`) — genuinely-at-risk unpushed work. Zero
/// means every local commit is safely on the remote PR branch. On any git
/// failure returns a conservative 1 (treat as at-risk → keep).
// trace:TASK-1230 | ai:claude
pub(crate) fn local_commits_not_on_branch(worktree: &std::path::Path, branch: &str) -> u32 {
    if branch.is_empty() {
        return 1;
    }
    let remote_ref = format!("origin/{branch}");
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["rev-list", "--count", "HEAD", "--not", &remote_ref])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse::<u32>()
            .unwrap_or(1),
        // The remote ref not existing, or any other git error, is treated as
        // "cannot prove the work is pushed" → keep (conservative).
        _ => 1,
    }
}

/// Detect whether `branch` was the head of a now-MERGED PR. Used by
/// `aida push` (BUG-88) to warn that new commits will be stranded —
/// pushing to a merged-PR branch puts the commit on `origin/<branch>`
/// but it won't reach `main` without a new PR. Mirrors the shape of
/// [`detect_open_pr_for_branch`] but queries `--state merged` and returns
/// only the first hit (most recent). trace:BUG-88 | ai:claude
pub(crate) fn detect_merged_pr_for_branch(
    project_root: &std::path::Path,
    branch: &str,
) -> PrLookup {
    let forge_kind = crate::forge::resolve_forge_kind(project_root);
    pr_lookup_from_change_lookup(
        crate::forge::forge_for_kind(project_root, forge_kind)
            .merged_change_for_branch(branch)
            .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}"))),
    )
}

// STORY-1163: raw GitHub merged lookup used by GitHubForge after the public
// helper became forge-dispatched. trace:STORY-1163 | ai:codex
pub(crate) fn detect_merged_pr_for_branch_github(
    project_root: &std::path::Path,
    branch: &str,
) -> PrLookup {
    gh_pr_list_first(project_root, &["--head", branch, "--state", "merged"])
}

/// Is PR #`pr` merged on GitHub? Ground truth for the BUG-241 reconcile step
/// — a reviewer that escalated the merge to a human who merged out-of-band
/// leaves the PR merged but the orchestrator's verdict file absent. `None` on
/// any `gh` failure (binary missing, auth, network): the caller treats
/// "cannot confirm" as "the failure stands", never as a silent success.
/// trace:BUG-241 | ai:claude
/// BUG-286: same intent as the pre-BUG-286 `pr_is_merged` but with a
/// caller-supplied retry sink, so the orchestrator can surface a sub-second
/// transient blip to the drain-state file rather than treating it as
/// "cannot confirm." Non-orchestrator callers (none today) can pass
/// [`network_retry::NoopSink`] to keep the original silent best-effort
/// semantics. trace:BUG-286 | ai:claude
pub(crate) fn pr_is_merged_with_sink(
    project_root: &std::path::Path,
    pr: u32,
    sink: &mut dyn network_retry::RetrySink,
) -> Option<bool> {
    // STORY-621 Slice 2: routed through Forge::change_metadata so the
    // reconcile works on GitLab. GitHubForge keeps the resolve_gh_binary +
    // BUG-286 retry-with-sink behaviour this site had inline; any provider
    // error (CLI missing / failed / unreachable) maps to None — "cannot
    // confirm", never a silent success. trace:TASK-963 | ai:claude
    crate::forge::forge_for(project_root)
        .change_metadata(pr as u64, sink)
        .ok()
        .map(|m| m.state == crate::forge::ChangeState::Merged)
}

/// BUG-245: which SPEC-ID does the PR's commits actually credit?
///
/// Reads the PR's commit subjects via `gh pr view <N> --json commits` and
/// parses `(SPEC-ID)` trailers out of each. Returns:
///   - `Some(dispatched)` when the dispatched id appears among the credits —
///     the common path. No mismatch.
///   - `Some(other)` when the dispatched id is *absent* and a different id is
///     credited. The orchestrator reports this as a `shipped-mismatch`
///     anomaly and credits the truth.
///   - `None` when the PR cannot be inspected, no commit names any spec, or
///     `gh` is unreachable — "cannot determine" resolves to "trust the
///     dispatched id", so the reconcile only ratifies a real mismatch.
///
/// Picks the first credited id when multiple non-dispatched ids appear. The
/// observed BUG-245 case is one commit / one trailer; a PR carrying multiple
/// genuinely-different specs is an open-ended case the operator must triage.
/// trace:BUG-245 | ai:claude
/// BUG-286: same intent as the pre-BUG-286 `pr_credited_spec_id` but with a
/// caller-supplied retry sink. The orchestrator passes a [`crate::network_retry::DualSink`]
/// (stderr + drain-state); a hypothetical silent caller can pass
/// [`network_retry::NoopSink`]. trace:BUG-286 | ai:claude
pub(crate) fn pr_credited_spec_id_with_sink(
    project_root: &std::path::Path,
    pr: u32,
    dispatched: &str,
    sink: &mut dyn network_retry::RetrySink,
) -> Option<String> {
    // STORY-621 Slice 2: routed through Forge::change_commit_headlines
    // (GitHubForge issues the same `gh pr view --json commits` argv this site
    // had inline, retry-wrapped). trace:TASK-963 | ai:claude
    let headlines = crate::forge::forge_for(project_root)
        .change_commit_headlines(pr as u64, sink)
        .ok()?;
    pick_credited_spec(&headlines.join("\n"), dispatched)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrCreditMatch {
    Dispatched,
    Other(String),
    Unknown,
}

/// BUG-357: classify whether PR metadata credits the dispatched spec before a
/// reconcile step treats a merged PR as out-of-band success. Commit trailers
/// are strongest; a different commit trailer is an explicit mismatch. PR title
/// trailers are accepted when commits carry no SPEC-ID, matching `aida pr ship`
/// and manual squash flows where the PR title is the only durable trailer.
pub(crate) fn classify_pr_credit(
    commit_subjects: &str,
    pr_title: Option<&str>,
    dispatched: &str,
) -> PrCreditMatch {
    match pick_credited_spec(commit_subjects, dispatched) {
        Some(id) if id.eq_ignore_ascii_case(dispatched) => return PrCreditMatch::Dispatched,
        Some(id) => return PrCreditMatch::Other(id),
        None => {}
    }

    if let Some(title) = pr_title {
        let mut first_other: Option<String> = None;
        for id in extract_spec_ids_from_commit(title) {
            if id.eq_ignore_ascii_case(dispatched) {
                return PrCreditMatch::Dispatched;
            }
            if first_other.is_none() {
                first_other = Some(id);
            }
        }
        if let Some(id) = first_other {
            return PrCreditMatch::Other(id);
        }
    }

    PrCreditMatch::Unknown
}

pub(crate) fn pr_credit_match_with_sink(
    project_root: &std::path::Path,
    pr: u32,
    dispatched: &str,
    pr_title: Option<&str>,
    sink: &mut dyn network_retry::RetrySink,
) -> PrCreditMatch {
    let initial_title_match = pr_title
        .map(|title| classify_pr_credit("", Some(title), dispatched))
        .unwrap_or(PrCreditMatch::Unknown);
    if matches!(initial_title_match, PrCreditMatch::Dispatched) {
        return PrCreditMatch::Dispatched;
    }

    // STORY-621 Slice 2: routed through the forge metadata reads (BUG-357
    // reconcile works on GitLab). Commit headlines settle most cases; the PR
    // title (Forge::change_metadata) is fetched only when the commits leave
    // the credit Unknown — same precedence as the single gh call this
    // replaces, paying for the scalar read only when it is load-bearing.
    // trace:TASK-963 | ai:claude
    let forge = crate::forge::forge_for(project_root);
    let subjects = match forge.change_commit_headlines(pr as u64, sink) {
        Ok(headlines) => headlines.join("\n"),
        Err(_) => return PrCreditMatch::Unknown,
    };
    match classify_pr_credit(&subjects, None, dispatched) {
        PrCreditMatch::Dispatched => PrCreditMatch::Dispatched,
        PrCreditMatch::Other(id) => PrCreditMatch::Other(id),
        PrCreditMatch::Unknown => {
            let fetched_title = forge
                .change_metadata(pr as u64, sink)
                .ok()
                .map(|m| m.title)
                .filter(|t| !t.trim().is_empty());
            match classify_pr_credit("", fetched_title.as_deref(), dispatched) {
                PrCreditMatch::Unknown => initial_title_match,
                title_match => title_match,
            }
        }
    }
}

/// Pure selector for the credited spec id, given newline-separated commit
/// subjects (one per commit on the PR) and the dispatched id. Split out so
/// the precedence rules are unit-testable without spawning `gh`.
/// trace:BUG-245 | ai:claude
pub(crate) fn pick_credited_spec(commit_subjects: &str, dispatched: &str) -> Option<String> {
    let mut first_other: Option<String> = None;
    for subject in commit_subjects.lines() {
        for id in extract_spec_ids_from_commit(subject) {
            if id == dispatched {
                return Some(id);
            }
            if first_other.is_none() {
                first_other = Some(id);
            }
        }
    }
    first_other
}

/// Parse a single tab-separated `gh pr list -q ...` line into a
/// `PrLookup`. Extracted for unit-testing the empty / well-formed /
/// malformed shapes without spawning `gh`. trace:BUG-88 | ai:claude
pub(crate) fn parse_gh_pr_line(stdout: &str) -> PrLookup {
    let Some(line) = stdout.lines().next().map(str::trim) else {
        return PrLookup::NoOpenPr;
    };
    if line.is_empty() {
        return PrLookup::NoOpenPr;
    }
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() < 3 {
        return PrLookup::GhFailed(format!("could not parse gh output: {:?}", line));
    }
    let Ok(number) = parts[0].parse::<u64>() else {
        return PrLookup::GhFailed(format!("non-numeric PR number from gh: {:?}", parts[0]));
    };
    PrLookup::Found(OpenPrInfo {
        number,
        title: parts[1].to_string(),
        url: parts[2].to_string(),
        head_branch: parts
            .get(3)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    })
}

pub(crate) fn open_pr_review_story_using(
    project_root: &std::path::Path,
    pr_number: u64,
    aida: &std::path::Path,
) -> Option<String> {
    let Ok(out) = std::process::Command::new(aida)
        .current_dir(project_root)
        .args(["list", "--type", "story", "--format", "json"])
        .output_retrying_etxtbsy()
    else {
        return None;
    };
    if !out.status.success() {
        return None;
    }
    open_review_story_id(&String::from_utf8_lossy(&out.stdout), pr_number)
}

/// BUG-1186: does `aida list --type story --format json` output carry a
/// `Review PR-<n>:` story whose round is still open (not Done / Completed /
/// Rejected / Superseded)? Pure so the round semantics are unit-testable.
// trace:BUG-1186 | ai:claude
#[cfg(test)]
pub(crate) fn review_story_round_is_open(list_json: &str, pr_number: u64) -> bool {
    open_review_story_id(list_json, pr_number).is_some()
}

pub(crate) fn open_review_story_id(list_json: &str, pr_number: u64) -> Option<String> {
    let needle = format!("review pr-{}:", pr_number);
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(list_json) else {
        return None;
    };
    rows.iter().find_map(|row| {
        let title = row
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim_start()
            .to_ascii_lowercase();
        if !title.starts_with(&needle) {
            return None;
        }
        let status = row
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-', '_'], "");
        (!matches!(
            status.as_str(),
            "done" | "completed" | "rejected" | "superseded" | "released"
        ))
        .then(|| row.get("spec_id")?.as_str().map(str::to_owned))
        .flatten()
    })
}

pub(crate) fn reviewer_queue_story_ids(project_root: &std::path::Path) -> Option<Vec<String>> {
    let out = std::process::Command::new(aida_exe_path())
        .current_dir(project_root)
        .args([
            "queue",
            "list",
            "--all",
            "--for",
            "reviewer",
            "--json",
            "--no-scope",
        ])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_reviewer_queue_story_ids(&out.stdout)
}

// TASK-192: keep malformed output distinguishable from a valid empty queue so
// callers that gate operator action can fail closed.
// trace:TASK-192 | ai:codex
pub(crate) fn parse_reviewer_queue_story_ids(bytes: &[u8]) -> Option<Vec<String>> {
    let rows = serde_json::from_slice::<Vec<serde_json::Value>>(bytes).ok()?;
    Some(
        rows.iter()
            .filter(|row| row.get("for_role").and_then(|v| v.as_str()) == Some("reviewer"))
            .filter_map(|row| {
                row.get("spec_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            })
            .collect(),
    )
}

// trace:TASK-192 | ai:codex
pub(crate) fn pr_has_merge_hold(
    project_root: &std::path::Path,
    pr: &status_cleanup::OpenPrItem,
) -> bool {
    merge_hold::read_hold(project_root, pr.number).is_some()
        || pr
            .labels
            .iter()
            .any(|label| label.eq_ignore_ascii_case("aida:merge-hold"))
}

// trace:TASK-192 | ai:codex
pub(crate) fn reviewer_route_for_pr(
    routed_prs: Option<&std::collections::HashSet<u64>>,
    pr_number: u64,
) -> awaiting_you::ReviewerRoute {
    match routed_prs {
        Some(prs) if prs.contains(&pr_number) => awaiting_you::ReviewerRoute::Routed,
        Some(_) => awaiting_you::ReviewerRoute::Unrouted,
        None => awaiting_you::ReviewerRoute::Unknown,
    }
}

#[cfg(test)]
mod task_192_fail_closed_fact_tests {
    use super::*;

    fn pr(labels: &[&str]) -> status_cleanup::OpenPrItem {
        status_cleanup::OpenPrItem {
            number: 2035,
            is_draft: false,
            title: "broken".into(),
            head_branch: "broken-pr".into(),
            ci_rollup: Some("fail".into()),
            mergeable: Some("MERGEABLE".into()),
            review_decision: None,
            head_sha: Some("deadbeef".into()),
            labels: labels.iter().map(|label| (*label).into()).collect(),
            created_at: None,
        }
    }

    #[test]
    fn forge_label_only_merge_hold_is_authoritative() {
        let root = tempfile::tempdir().unwrap();
        assert!(pr_has_merge_hold(root.path(), &pr(&["aida:merge-hold"])));
        assert!(pr_has_merge_hold(root.path(), &pr(&["AIDA:MERGE-HOLD"])));
        assert!(!pr_has_merge_hold(root.path(), &pr(&["bug"])));
    }

    #[test]
    fn reviewer_queue_parser_distinguishes_empty_from_malformed() {
        assert_eq!(parse_reviewer_queue_story_ids(b"[]"), Some(Vec::new()));
        assert_eq!(parse_reviewer_queue_story_ids(b"not json"), None);
        assert_eq!(parse_reviewer_queue_story_ids(b"{}"), None);
    }

    #[test]
    fn unavailable_queue_maps_to_unknown_not_unrouted() {
        assert_eq!(
            reviewer_route_for_pr(None, 2035),
            awaiting_you::ReviewerRoute::Unknown
        );
        let available_empty = std::collections::HashSet::new();
        assert_eq!(
            reviewer_route_for_pr(Some(&available_empty), 2035),
            awaiting_you::ReviewerRoute::Unrouted
        );
    }

    #[test]
    fn forge_snapshot_retains_merge_hold_label() {
        let snapshot = parse_open_pr_snapshot(
            r#"[{"number":2035,"title":"broken","headRefName":"broken-pr","labels":[{"name":"aida:merge-hold"}],"statusCheckRollup":[]}]"#,
            None,
        );
        assert_eq!(
            snapshot.by_branch["broken-pr"].labels,
            vec!["aida:merge-hold"]
        );
    }
}

/// Complete inventory for the review-story creation gate only. Bulk readers
/// intentionally tolerate bad objects; absence here must not use that contract.
// trace:BUG-1807 | ai:codex
fn load_review_story_inventory(project_root: &std::path::Path) -> Result<RequirementsStore> {
    if let Some(store_path) = detect_distributed_store_from(project_root) {
        let objects = store_path.join("objects");
        let mut store = aida_core::GitBackend::read_metadata_only(&store_path)?;
        store.requirements = aida_core::object_store::list_objects_strict(&objects)?
            .into_iter()
            .map(|(_, path)| aida_core::object_store::read_object_from_path(&path))
            .collect::<Result<Vec<_>>>()?;
        return Ok(store);
    }

    // The normal resolver is best effort. Before allowing legacy resolution,
    // distinguish absent config from unreadable config or unavailable storage.
    for root in project_root.ancestors() {
        if aida_core::store_locate::is_system_temp_dir(root) {
            break;
        }
        let path = root.join(".aida/config.toml");
        match std::fs::read_to_string(&path) {
            Ok(config) => {
                toml::from_str::<toml::Table>(&config)
                    .with_context(|| format!("Cannot parse {}", path.display()))?;
                anyhow::ensure!(
                    !config_declares_distributed(&config)
                        && aida_core::store_locate::store_path_candidates(&config).is_empty(),
                    "Configured canonical review-story storage is unavailable at {}",
                    path.display()
                );
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err).with_context(|| format!("Cannot read {}", path.display())),
        }
    }
    let path = aida_core::resolve_requirements_path_in(
        project_root,
        None,
        None,
        std::path::Path::new(""),
    )?;
    Storage::new(path).load()
}

pub(crate) fn canonical_review_story<'a>(
    store: &'a RequirementsStore,
    forge: ReviewForge,
    number: u64,
    covered_specs: Option<&[String]>,
    preferred_spec: Option<&str>,
) -> Option<&'a aida_core::Requirement> {
    let expected: std::collections::BTreeSet<String> = covered_specs
        .unwrap_or_default()
        .iter()
        .map(|id| id.to_ascii_uppercase())
        .collect();
    let covered = |req: &aida_core::Requirement| {
        req.relationships
            .iter()
            .filter(|rel| rel.rel_type.to_string().eq_ignore_ascii_case("implements"))
            .filter_map(|rel| store.requirements.iter().find(|r| r.id == rel.target_id))
            .filter_map(|r| r.spec_id.as_deref().or(r.agreed_id.as_deref()))
            .map(|id| id.to_ascii_uppercase())
            .collect::<std::collections::BTreeSet<_>>()
    };
    let mut candidates: Vec<_> = store
        .requirements
        .iter()
        .filter(|req| {
            !is_terminal_status(&req.status) && review_title_matches(&req.title, forge, number)
        })
        .collect();
    candidates.sort_by_key(|req| {
        let coverage_rank = if covered_specs.is_some() && covered(req) != expected {
            1
        } else {
            0
        };
        let display = req.display_id();
        let preferred_rank = if preferred_spec.is_some_and(|id| id.eq_ignore_ascii_case(&display)) {
            0
        } else {
            1
        };
        (coverage_rank, preferred_rank, display.to_ascii_uppercase())
    });
    candidates.into_iter().next()
}

/// Strip ANSI SGR sequences (`ESC[...m`) so we can match output text
/// regardless of whether the child invocation colored its output. Keep it
/// minimal — we only need the SGR shape `aida add` emits.
/// trace:STORY-66 | ai:claude
pub(crate) fn strip_ansi_color(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for cc in chars.by_ref() {
                if cc.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Self-invoke `aida add` for a Story-typed review requirement and parse the
/// resulting spec id out of stdout. Returns None on any failure.
/// trace:STORY-66 | ai:claude
pub(crate) fn aida_subcmd_add_review_story(
    project_root: &std::path::Path,
    title: &str,
    description: &str,
) -> Option<String> {
    let aida = aida_exe_path();
    let out = std::process::Command::new(&aida)
        .current_dir(project_root)
        .args([
            "add",
            "--type",
            "story",
            "--status",
            "approved",
            "--priority",
            "medium",
            "--title",
            title,
            "--description",
            description,
        ])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "{} auto-queue review story: `aida add` failed: {}",
            "Warning:".yellow().bold(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_spec_id_from_add_output(&stdout)
}

/// Parse the spec id printed by `aida add`. The git-canonical path prints
/// `Added: STORY-N - <title>` (one line); the legacy YAML/SQLite path
/// prints a standalone `ID: STORY-N` line. Accept either so this hook keeps
/// working across backends. trace:STORY-66 | ai:claude
pub(crate) fn parse_spec_id_from_add_output(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let cleaned = strip_ansi_color(line);
        let candidate = cleaned
            .strip_prefix("Added: ")
            .map(|rest| rest.split(" - ").next().unwrap_or("").trim().to_string())
            .or_else(|| {
                cleaned
                    .strip_prefix("ID: ")
                    .map(|rest| rest.trim().to_string())
            });
        if let Some(c) = candidate {
            if !c.is_empty() {
                return Some(c);
            }
        }
    }
    None
}

// ============================================================================
// TASK-96 — plan Followups extraction. When a spec reaches Done (`aida queue
// done`) or Completed (the STORY-86 auto-bump), parse the `## Followups`
// section of any matching docs/plans/ file and offer to file each bullet as
// a child TASK so out-of-scope items don't get forgotten.
// ============================================================================

/// Comment prefix written to a spec once its plan followups have been
/// processed. Both extraction paths check for it first, so whichever runs
/// first (`queue done` interactively, or the auto-bump non-interactively)
/// wins and the other skips — declines are never silently re-filed.
pub(crate) const FOLLOWUPS_MARKER: &str = "[aida:followups]";

/// True when followup automation is disabled via `AIDA_AUTO_FOLLOWUPS`
/// (mirrors the `AIDA_AUTO_BUMP` opt-out shape).
pub(crate) fn auto_followups_disabled() -> bool {
    matches!(
        std::env::var("AIDA_AUTO_FOLLOWUPS")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "false" | "0" | "no" | "off"
    )
}

/// BUG-655: content-level dedup for auto-followups. The [`FOLLOWUPS_MARKER`]
/// comment is a *per-spec* fast-path, but it is not a hard guarantee: a plan
/// carrying a `## Followups` section can be committed into more than one slice
/// worktree (STORY-712 shipped via 4 slice PRs), and two independent commits
/// that each reach completion before the other's marker comment has synced
/// will *both* fire the filing — double-filing the same bullets. The robust
/// guarantee is content-level: before filing a bullet as a child TASK, check
/// whether a child TASK with the SAME parent spec AND the SAME (trimmed,
/// case-insensitive) title already exists, and skip it if so. Child specs are
/// real, queryable store rows, so this survives across commits/agents/PRs in a
/// way the single marker comment does not.
///
/// `existing_children` is the `(parent_spec_id, title)` set already present in
/// the store. Pure + total so it is unit-testable in isolation.
pub(crate) fn followup_already_filed(
    existing_children: &[(String, String)],
    parent: &str,
    bullet: &str,
) -> bool {
    let norm = |s: &str| s.trim().to_ascii_lowercase();
    let want = norm(bullet);
    existing_children
        .iter()
        .any(|(p, title)| p == parent && norm(title) == want)
}

// BUG-656: BUG-655's per-parent `(parent, title)` dedup was not enough. The
// auto-followups path keys child TASKs to the *completing spec*, so the same
// plan's bullets get re-filed when a DIFFERENT spec completes — either because
// the plan's header lists several specs (each "owns" the plan and re-extracts
// it) or because the same followup text was copied into a sibling plan. On
// EPIC-0428 this re-filed ~14 cross-plan duplicate TASKs (TASK-1019-1032). The
// stable signature is the PLAN PATH, not the completing spec: a plan's followups
// must be extracted exactly once across the whole store. The marker comment
// already records `extracted from <relative plan paths>`; we parse that back out
// of every spec's comments to build the already-extracted set, then skip any
// plan already in it. A re-completion, a sibling-spec completion, or a slice PR
// landing the plan twice all become no-ops at the plan granularity.

/// Parse the plan paths a [`FOLLOWUPS_MARKER`] comment records. The marker's
/// first line is `[aida:followups] extracted from <p1>, <p2>, ...`; everything
/// after `extracted from` up to the newline is the comma-separated relative
/// plan-path list. Returns the trimmed, non-empty paths (empty for any comment
/// that is not a followups marker, or a marker with no `extracted from` clause).
/// Pure + total so it is unit-testable in isolation.
// trace:BUG-656 | ai:claude
pub(crate) fn parse_extracted_plans_from_marker(comment: &str) -> Vec<String> {
    let Some(rest) = comment.strip_prefix(FOLLOWUPS_MARKER) else {
        return Vec::new();
    };
    let Some(first_line) = rest.lines().next() else {
        return Vec::new();
    };
    let Some(paths) = first_line.trim().strip_prefix("extracted from ") else {
        return Vec::new();
    };
    paths
        .split(',')
        .map(|p| normalize_recorded_plan_path(p.trim()))
        .filter(|p| !p.is_empty())
        .collect()
}

/// A plan path read back from a followup marker or a `followup-src:` tag, in
/// the `/` form [`plan_rel_path`] now writes. Versions before BUG-1648 wrote
/// `docs/plans\x.md` on Windows; normalizing on read keeps those dedup-able.
// trace:BUG-1648 | ai:claude
pub(crate) fn normalize_recorded_plan_path(recorded: &str) -> String {
    recorded.replace('\\', "/")
}

/// BUG-656: the cross-store stable signature. Given the relative plan paths a
/// completing spec owns and the set of plan paths already extracted (parsed from
/// every spec's [`FOLLOWUPS_MARKER`] comments), return only the plans not yet
/// extracted. Empty result ⇒ every owned plan's followups were already filed by
/// an earlier completion, so the whole pass is a no-op. Pure + total.
// trace:BUG-656 | ai:claude
pub(crate) fn plans_pending_extraction(
    owned_rel_paths: &[String],
    already_extracted: &std::collections::HashSet<String>,
) -> Vec<String> {
    owned_rel_paths
        .iter()
        .filter(|p| !already_extracted.contains(*p))
        .cloned()
        .collect()
}

/// BUG-656: secondary global-title guard. The plan-path signature stops a plan
/// being re-extracted, but the SAME followup text copied into a *distinct*
/// sibling plan (a different owning spec, a different path) would still slip
/// through. `existing_titles` is the normalized titles of every followup TASK
/// already filed across the store; a bullet whose trimmed/case-insensitive title
/// matches one of them has already been filed somewhere and is skipped. Pure +
/// total.
// trace:BUG-656 | ai:claude
pub(crate) fn followup_filed_anywhere(existing_titles: &[String], bullet: &str) -> bool {
    let want = bullet.trim().to_ascii_lowercase();
    existing_titles.iter().any(|t| *t == want)
}

/// BUG-1625: does `store` already hold `bullet` as a filed followup of
/// `parent`? True when a spec with the same trimmed, case-insensitive title is
/// a child of `parent` or carries the [`FOLLOWUP_SRC_TAG_PREFIX`] provenance
/// tag of `source_plan` — the plan this bullet came from. BUG-1633: a
/// same-titled spec filed from a DIFFERENT plan is not this followup.
/// Used to classify a failed `aida add` as "already filed" instead of
/// "declined". Pure + total.
// trace:BUG-1625 trace:BUG-1633 | ai:claude
pub(crate) fn followup_filed_in_store(
    store: &RequirementsStore,
    parent: &str,
    bullet: &str,
    source_plan: &str,
) -> bool {
    use aida_core::models::RelationshipType;
    let want = bullet.trim().to_ascii_lowercase();
    let child_ids: std::collections::HashSet<uuid::Uuid> = store
        .get_requirement_by_spec_id(parent)
        .map(|p| {
            p.relationships
                .iter()
                .filter(|r| r.rel_type == RelationshipType::Parent)
                .map(|r| r.target_id)
                .collect()
        })
        .unwrap_or_default();
    store.requirements.iter().any(|r| {
        r.title.trim().to_ascii_lowercase() == want
            && (child_ids.contains(&r.id)
                || r.tags.iter().any(|t| {
                    // Tags written before BUG-1648 may carry `\`.
                    // trace:BUG-1648 | ai:claude
                    t.strip_prefix(FOLLOWUP_SRC_TAG_PREFIX)
                        .is_some_and(|p| normalize_recorded_plan_path(p) == source_plan)
                }))
    })
}

/// BUG-680: tag prefix that records the source plan path on a followup TASK the
/// auto-followup path files. The BUG-656 marker comment records the extraction
/// on the *completing spec*, but that comment can be lost or arrive unsynced
/// (two commits completing before either marker syncs — the same window BUG-655
/// flagged), and it says nothing once the followup itself has shipped and been
/// archived away from the parent. Stamping the provenance on the child spec
/// makes it durable and queryable: the followup carries its own origin, so a
/// later re-extraction can recognise "this bullet already shipped from this
/// plan" straight from the store even when the parent's marker is gone. Flat,
/// colon-namespaced provenance tag (like `parent:`, `batch:`) per the tag
/// conventions.
// trace:BUG-680 | ai:claude
pub(crate) const FOLLOWUP_SRC_TAG_PREFIX: &str = "followup-src:";

pub(crate) const FOLLOWUP_META_PROSE_OPENERS: &[&str] =
    &["consider", "maybe", "possibly", "optionally"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FollowupSkip {
    PointerToExistingSpec(String),
    MetaProse(&'static str),
}

pub(crate) fn followup_strip_markdown_emphasis(text: &str) -> &str {
    text.trim()
        .trim_start_matches('*')
        .trim_start_matches('_')
        .trim_end_matches('*')
        .trim_end_matches('_')
        .trim()
}

// trace:BUG-1142 | ai:codex
pub(crate) fn followup_meta_prose_opener(text: &str) -> Option<&'static str> {
    let stripped = followup_strip_markdown_emphasis(text);
    let lower = stripped.to_ascii_lowercase();
    FOLLOWUP_META_PROSE_OPENERS.iter().copied().find(|opener| {
        lower == *opener
            || lower.starts_with(&format!("{opener}:"))
            || lower.starts_with(&format!("{opener} "))
    })
}

// trace:BUG-1142 | ai:codex
pub(crate) fn followup_existing_spec_pointer(
    store: &aida_core::models::RequirementsStore,
    text: &str,
) -> Option<String> {
    let re = regex::Regex::new(r"\b[A-Z][A-Z0-9]+(?:-\d+){1,3}\b").ok()?;
    let found = re.find_iter(text).find_map(|m| {
        let token = m.as_str();
        store
            .get_requirement_by_spec_id(token)
            .map(|req| req.display_id())
    });
    found
}

// trace:BUG-1142 | ai:codex
pub(crate) fn classify_plan_followup(
    store: &aida_core::models::RequirementsStore,
    text: &str,
) -> Option<FollowupSkip> {
    if let Some(id) = followup_existing_spec_pointer(store, text) {
        return Some(FollowupSkip::PointerToExistingSpec(id));
    }
    followup_meta_prose_opener(text).map(FollowupSkip::MetaProse)
}

/// BUG-680: a followup bullet already filed from the SAME source plan that has
/// since reached a terminal status (Completed / Rejected — the work shipped or
/// was rejected). `filed_from_plan` is `(recorded_plan_path, title, spec_id,
/// is_terminal)` for every spec carrying a [`FOLLOWUP_SRC_TAG_PREFIX`] tag;
/// `owned_plans` is the set of source plans currently being extracted. Returns
/// the id of a terminal spec previously filed from one of `owned_plans` whose
/// title matches `bullet` — re-filing it would create a second open spec for a
/// followup that already ran its course, so the caller skips and links to the
/// returned id. Match is trim + case-insensitive on the title; the plan path is
/// exact (both sides are the store-relative path). Pure + total so it is
/// unit-testable in isolation.
// trace:BUG-680 | ai:claude
pub(crate) fn followup_shipped_from_plan<'a>(
    filed_from_plan: &'a [(String, String, String, bool)],
    owned_plans: &std::collections::HashSet<String>,
    bullet: &str,
) -> Option<&'a str> {
    let want = bullet.trim().to_ascii_lowercase();
    filed_from_plan
        .iter()
        .find(|(plan, title, _, terminal)| {
            *terminal && owned_plans.contains(plan) && title.trim().to_ascii_lowercase() == want
        })
        .map(|(_, _, id, _)| id.as_str())
}

/// Find the docs/plans/ files that *belong to* `spec_id` — the id appears
/// in the `# Plan:` title line or the `Specs:` header line. Cross-references
/// in a `## Related` section don't count (that plan owns a different spec).
pub(crate) fn find_plan_files_for_spec(
    project_root: &std::path::Path,
    spec_id: &str,
) -> Vec<std::path::PathBuf> {
    let plans_dir = project_root.join("docs/plans");
    let Ok(entries) = std::fs::read_dir(&plans_dir) else {
        return Vec::new();
    };
    let needle = regex::Regex::new(&format!(r"\b{}\b", regex::escape(spec_id)));
    let Ok(needle) = needle else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Only the header zone (title + `Specs:` line) confers ownership.
        let owns = content
            .lines()
            .take(15)
            .any(|l| (l.starts_with("# ") || l.starts_with("Specs:")) && needle.is_match(l));
        if owns {
            hits.push(path);
        }
    }
    hits.sort();
    hits
}

/// Parse the `## Followups` section of a plan: collect the column-0
/// `- `/`* ` bullets until the next `##` header. Leading marker and a
/// trailing period are stripped; placeholder/empty bullets are dropped.
/// TASK-431: is this `## Followups` bullet a "no followups" sentinel
/// (`None`, `N/A`, `NA`, `Nothing`, `(none)`, optionally followed by an
/// em-dash / colon / spaced-dash explanation) rather than a real followup?
/// Conservative — only when the sentinel is the WHOLE leading clause, so a
/// genuine bullet like "None of these handlers do X" is kept. trace:TASK-431
pub(crate) fn followup_is_sentinel(text: &str) -> bool {
    let lower = text.trim().to_ascii_lowercase();
    let unparen = lower.trim_start_matches('(').trim_end_matches(')').trim();
    // Leading clause = everything before the first explanation separator.
    let head = unparen
        .split([':', '—'])
        .next()
        .unwrap_or(unparen)
        .split(" - ")
        .next()
        .unwrap_or(unparen)
        .trim();
    matches!(
        head,
        "none" | "n/a" | "na" | "nothing" | "nothing here" | "nothing to do"
    )
}

pub(crate) fn parse_plan_followups(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_section = false;
    let mut in_fence = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(h) = trimmed.strip_prefix("## ") {
            in_section = h.to_ascii_lowercase().contains("followup");
            continue;
        }
        if !in_section {
            continue;
        }
        // Column-0 bullets only — nested detail bullets are not followups.
        let bullet = line.strip_prefix("- ").or_else(|| line.strip_prefix("* "));
        if let Some(b) = bullet {
            let text = b.trim().trim_end_matches('.').trim().to_string();
            // BUG-104: skip only a literal template placeholder (a bullet that
            // STARTS with `<`, e.g. `<describe the followup>`) — not any bullet
            // that merely contains `<` (a real followup like "support `Vec<T>`"
            // or "fix the `a < b` guard" was being silently dropped).
            if text.is_empty()
                || text.starts_with('<')
                || text.starts_with("file as")
                || followup_is_sentinel(&text)
            {
                continue;
            }
            out.push(text);
        }
    }
    out
}

/// TASK-95: parse the `## Critical Files` section — the column-0 bullets'
/// backtick-quoted paths. The flat must-touch blast radius.
pub(crate) fn parse_plan_critical_files(content: &str) -> Vec<String> {
    let path_re = regex::Regex::new(r"`([A-Za-z0-9_][A-Za-z0-9_./\-]*\.[A-Za-z0-9]+)`").unwrap();
    let mut out: Vec<String> = Vec::new();
    let mut in_section = false;
    let mut in_fence = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(h) = trimmed.strip_prefix("## ") {
            in_section = h.to_ascii_lowercase().contains("critical files");
            continue;
        }
        if !in_section {
            continue;
        }
        if line.starts_with("- ") || line.starts_with("* ") {
            for cap in path_re.captures_iter(line) {
                let p = cap[1].to_string();
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// TASK-95: parse the `## Verification` section — the body of its first
/// fenced code block, i.e. the executable definition of done.
pub(crate) fn parse_plan_verification(content: &str) -> Option<String> {
    let mut in_section = false;
    let mut in_fence = false;
    let mut body = String::new();
    let mut captured = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if !in_section {
            if let Some(h) = trimmed.strip_prefix("## ") {
                in_section = h.to_ascii_lowercase().contains("verification");
            }
            continue;
        }
        if trimmed.starts_with("## ") {
            break; // reached the next section without a closed fence
        }
        if trimmed.starts_with("```") {
            if in_fence {
                captured = true;
                break;
            }
            in_fence = true;
            continue;
        }
        if in_fence {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(line);
        }
    }
    if captured && !body.trim().is_empty() {
        Some(body)
    } else {
        None
    }
}

/// TASK-95: build the plan brief for `spec_id` — locate the owning
/// docs/plans/ file and extract its Critical Files, Followups, and
/// Verification sections. Returns `None` when no plan file owns the spec
/// (the graceful no-op `aida queue work` falls back to).
pub(crate) fn discover_plan_context(
    project_root: &std::path::Path,
    spec_id: &str,
) -> Option<session_manifest::PlanContext> {
    // BUG-105: when more than one plan file owns the spec, merge them all
    // instead of silently using only the first. Critical-files and followups
    // are unioned (dedup, order-preserving); verification scripts are
    // concatenated; plan_file lists every contributing path.
    let plan_files = find_plan_files_for_spec(project_root, spec_id);
    let mut rels: Vec<String> = Vec::new();
    let mut critical_files: Vec<String> = Vec::new();
    let mut followups: Vec<String> = Vec::new();
    let mut verifications: Vec<String> = Vec::new();
    for plan_file in &plan_files {
        let Ok(content) = std::fs::read_to_string(plan_file) else {
            continue;
        };
        // `/`-separated on every OS, like the followup marker. trace:BUG-1648 | ai:claude
        rels.push(plan_rel_path(plan_file, project_root));
        for c in parse_plan_critical_files(&content) {
            if !critical_files.contains(&c) {
                critical_files.push(c);
            }
        }
        for f in parse_plan_followups(&content) {
            if !followups.contains(&f) {
                followups.push(f);
            }
        }
        if let Some(v) = parse_plan_verification(&content) {
            verifications.push(v);
        }
    }
    if rels.is_empty() {
        return None;
    }
    Some(session_manifest::PlanContext {
        plan_file: rels.join(", "),
        critical_files,
        followups,
        verification: if verifications.is_empty() {
            None
        } else {
            Some(verifications.join("\n\n"))
        },
    })
}

/// BUG-1633: test-only stand-in for the followup `aida add` subprocess.
/// Receives `(project_root, parent, title, source_plan)` and returns the new
/// spec id, or `None` to simulate a failed add.
// trace:BUG-1633 | ai:claude
#[cfg(test)]
pub(crate) type FollowupAddHook =
    Box<dyn FnMut(&std::path::Path, &str, &str, Option<&str>) -> Option<String>>;

#[cfg(test)]
thread_local! {
    static FOLLOWUP_ADD_HOOK: std::cell::RefCell<Option<FollowupAddHook>> =
        const { std::cell::RefCell::new(None) };
}

/// Self-invoke `aida add` to file one followup as a child TASK of
/// `parent_spec`. Always passes `--force-parent` — the parent is Done or
/// Completed by the time we file, and we explicitly want the children
/// regardless. When `source_plan` is set, stamps the plan's provenance as a
/// [`FOLLOWUP_SRC_TAG_PREFIX`] tag so a later re-extraction can dedup against
/// this followup even after it ships (BUG-680). Returns the new spec id on
/// success.
pub(crate) fn aida_subcmd_add_followup_task(
    project_root: &std::path::Path,
    parent_spec: &str,
    title: &str,
    source_plan: Option<&str>,
) -> Option<String> {
    // BUG-1633: tests substitute the `aida add` subprocess (the test binary is
    // not `aida`). trace:BUG-1633 | ai:claude
    #[cfg(test)]
    if let Some(result) = FOLLOWUP_ADD_HOOK.with(|h| {
        h.borrow_mut()
            .as_mut()
            .map(|f| f(project_root, parent_spec, title, source_plan))
    }) {
        return result;
    }
    let aida = aida_exe_path();
    let mut args: Vec<String> = vec![
        "add".into(),
        "--type".into(),
        "task".into(),
        "--status".into(),
        "approved".into(),
        "--priority".into(),
        "low".into(),
        "--parent".into(),
        parent_spec.into(),
        "--force-parent".into(),
        "--title".into(),
        title.into(),
    ];
    // BUG-680: durable source-plan provenance on the child, so re-filing after
    // this followup ships is a no-op straight from the store. trace:BUG-680
    if let Some(plan) = source_plan {
        args.push("--tags".into());
        args.push(format!("{FOLLOWUP_SRC_TAG_PREFIX}{plan}"));
    }
    let out = std::process::Command::new(&aida)
        .current_dir(project_root)
        .args(&args)
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "{} followup `aida add` failed: {}",
            "Warning:".yellow().bold(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    parse_spec_id_from_add_output(&String::from_utf8_lossy(&out.stdout))
}

/// True when interface-change capture is disabled via
/// `AIDA_CAPTURE_INTERFACE_CHANGES=0|false|no`. Mirrors
/// [`auto_followups_disabled`]: the env-opt-out keeps the close-checkpoint out
/// of unattended drains that don't want the prompt. trace:STORY-542 | ai:claude
pub(crate) fn capture_interface_changes_disabled() -> bool {
    matches!(
        std::env::var("AIDA_CAPTURE_INTERFACE_CHANGES")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "no"
    )
}

/// Prompt for the surface-delta lines on one interface (cli/mcp/tui/other) at a
/// TTY: one line per `Enter`, blank line ends the surface. Returns the
/// collected lines. trace:STORY-542 | ai:claude
pub(crate) fn prompt_interface_surface(label: &str, example: &str) -> Vec<String> {
    use std::io::Write;
    let mut lines = Vec::new();
    eprintln!(
        "  {} {} change(s)? One per line, blank line when done.",
        "→".cyan(),
        label.bold()
    );
    eprintln!("    {} {}", "e.g.".dimmed(), example.dimmed());
    loop {
        eprint!("    > ");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
            break;
        }
        let trimmed = answer.trim();
        if trimmed.is_empty() {
            break;
        }
        lines.push(trimmed.to_string());
    }
    lines
}

/// Capture the spec's user-facing interface changes at close (`aida queue
/// done`) — the deterministic Layer-1 source for the operator digest
/// (STORY-541 / STORY-542). Precedence:
///
/// 1. `--no-interface-change` ⇒ record nothing (the spec stays out of the
///    operator digest), no prompt.
/// 2. Any `--interface-{cli,mcp,tui,other}` flag ⇒ use exactly those lines,
///    no prompt (the deterministic / agent path).
/// 3. Otherwise, at a TTY and when `interactive` (not `--yes`) and not
///    env-disabled ⇒ prompt per surface (derive-and-confirm is future work;
///    today the implementer types the lines).
/// 4. Non-interactive with no flags ⇒ leave `interface_changes` untouched
///    (empty), exactly as a no-impact spec.
///
/// Idempotent: if the spec already carries a non-empty `interface_changes`
/// (e.g. `/aida-pr` populated it earlier), and no flags override it, it is left
/// alone. Best-effort — any failure returns `Ok(())` so it never breaks
/// `queue done`. trace:STORY-542 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn capture_interface_changes(
    storage: &Storage,
    target: &aida_core::Requirement,
    display_id: &str,
    flag_cli: &[String],
    flag_mcp: &[String],
    flag_tui: &[String],
    flag_other: &[String],
    no_interface_change: bool,
    interactive: bool,
) -> Result<()> {
    let req_id = target.id;
    let any_flag = !flag_cli.is_empty()
        || !flag_mcp.is_empty()
        || !flag_tui.is_empty()
        || !flag_other.is_empty();

    // Resolve the changes to record. `None` ⇒ leave the spec untouched.
    let changes: Option<aida_core::InterfaceChanges> = if no_interface_change {
        // Explicit "no user-facing change" — record an empty marker so a
        // re-run / the auto-bump path both treat this spec as decided and the
        // operator digest skips it. We persist `Some(empty)` to distinguish
        // "decided: nothing" from "never asked".
        Some(aida_core::InterfaceChanges::default())
    } else if any_flag {
        Some(aida_core::InterfaceChanges {
            cli: flag_cli.to_vec(),
            mcp: flag_mcp.to_vec(),
            tui: flag_tui.to_vec(),
            other: flag_other.to_vec(),
        })
    } else {
        // No flags: only prompt at a TTY, interactively, and not opted out.
        let at_tty = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
        if !interactive || !at_tty || capture_interface_changes_disabled() {
            return Ok(());
        }
        // Idempotency: don't re-prompt if a non-empty capture already exists.
        let store = storage.load()?;
        if let Some(r) = store.requirements.iter().find(|r| r.id == req_id) {
            if r.interface_changes
                .as_ref()
                .is_some_and(|ic| !ic.is_empty())
            {
                return Ok(());
            }
        }
        eprintln!();
        eprintln!(
            "{} How does {} change the user-facing interface? (feeds the operator digest)",
            "→".cyan().bold(),
            display_id.bold()
        );
        eprintln!(
            "  {} no user-facing change (clippy/refactor/test)? Just press Enter through each.",
            "·".dimmed()
        );
        let ic = aida_core::InterfaceChanges {
            cli: prompt_interface_surface("CLI", "aida foo — new command"),
            mcp: prompt_interface_surface("MCP", "queue_add — now advisor-gated"),
            tui: prompt_interface_surface("TUI", "press `g` — jump to graph view"),
            other: prompt_interface_surface("Other", "REST /digest endpoint added"),
        };
        Some(ic)
    };

    let Some(changes) = changes else {
        return Ok(());
    };

    let now = chrono::Utc::now();
    let was_empty = changes.is_empty();
    let summary = if was_empty {
        "no user-facing interface change".to_string()
    } else {
        let mut parts = Vec::new();
        if !changes.cli.is_empty() {
            parts.push(format!("{} cli", changes.cli.len()));
        }
        if !changes.mcp.is_empty() {
            parts.push(format!("{} mcp", changes.mcp.len()));
        }
        if !changes.tui.is_empty() {
            parts.push(format!("{} tui", changes.tui.len()));
        }
        if !changes.other.is_empty() {
            parts.push(format!("{} other", changes.other.len()));
        }
        parts.join(", ")
    };

    // Per-spec compare-and-swap, no whole-store write. trace:BUG-1612 | ai:claude
    storage.update_spec_atomically(target, |r| {
        // Store `Some(empty)` for the explicit no-change case so it reads as
        // "decided"; store the populated set otherwise.
        r.interface_changes = Some(changes.clone());
        r.modified_at = now;
    })?;

    if was_empty {
        println!("  {} interface changes: none recorded", "·".dimmed());
    } else {
        println!(
            "  {} interface changes captured for {} ({})",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            display_id.bold(),
            summary.dimmed()
        );
    }
    Ok(())
}

/// True when verification-step capture is disabled via
/// `AIDA_AUTO_TEST_PLAN_CAPTURE=0|false|no`. Mirrors [`auto_followups_disabled`]
/// and [`capture_interface_changes_disabled`] — the env-opt-out keeps the
/// close-checkpoint prompt out of unattended drains.
// trace:STORY-698 | ai:claude
pub(crate) fn capture_test_plan_disabled() -> bool {
    matches!(
        std::env::var("AIDA_AUTO_TEST_PLAN_CAPTURE")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "no"
    )
}

/// What [`capture_test_plan`] should do with the verification steps, given the
/// resolved flag / TTY / opt-out state. Split out as a pure decision so the
/// precedence rules are unit-testable without a TTY.
// trace:STORY-698 | ai:claude
#[derive(Debug, PartialEq)]
pub(crate) enum TestPlanCapture {
    /// Leave `test_coverage_notes` untouched (no prompt, no write).
    Skip,
    /// Store exactly these joined steps (the flag / deterministic path).
    Record(String),
    /// Prompt the builder at the TTY for the steps.
    Prompt,
}

/// Precedence for verification-step capture at `aida queue done`:
///
/// 1. Any `--test-plan STEP` flag ⇒ record exactly those steps, no prompt.
/// 2. `--no-test-plan` ⇒ skip (leave any existing notes untouched), no prompt.
/// 3. Otherwise, at a TTY, interactive (not `--yes`), not env-disabled, and
///    with nothing already captured ⇒ prompt.
/// 4. Any other case (non-interactive, no TTY, opted out, already set) ⇒ skip.
///
/// A `--test-plan` flag overrides an existing value (explicit intent); the
/// interactive path never re-prompts once a value is set.
// trace:STORY-698 | ai:claude
pub(crate) fn decide_test_plan_capture(
    flag_steps: &[String],
    no_test_plan: bool,
    interactive: bool,
    at_tty: bool,
    disabled: bool,
    already_set: bool,
) -> TestPlanCapture {
    if !flag_steps.is_empty() {
        return TestPlanCapture::Record(flag_steps.join("\n"));
    }
    if no_test_plan || !interactive || !at_tty || disabled || already_set {
        return TestPlanCapture::Skip;
    }
    TestPlanCapture::Prompt
}

/// Prompt at a TTY for the verification steps the builder ran: one step per
/// `Enter`, blank line ends. Mirrors [`prompt_interface_surface`]. Returns the
/// collected lines.
// trace:STORY-698 | ai:claude
pub(crate) fn prompt_test_plan_steps() -> Vec<String> {
    use std::io::Write;
    let mut lines = Vec::new();
    eprintln!(
        "  {} verification steps you ran? One per line, blank line when done.",
        "→".cyan()
    );
    eprintln!(
        "    {} {}",
        "e.g.".dimmed(),
        "cargo test -p aida-cli · manual: aida queue done at a TTY".dimmed()
    );
    loop {
        eprint!("    > ");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
            break;
        }
        let trimmed = answer.trim();
        if trimmed.is_empty() {
            break;
        }
        lines.push(trimmed.to_string());
    }
    lines
}

/// Capture the verification steps the builder actually ran at close (`aida
/// queue done`) — the implementation audit trail surfaced in the PR body
/// (STORY-698). Stored in `implementation_info.test_coverage_notes` (no new
/// model field, no cache migration — design LOCKED 2026-07-01). Steps are
/// joined newline-separated.
///
/// Best-effort — any failure returns `Ok(())` so it never breaks `queue done`.
/// Idempotent: an existing non-empty `test_coverage_notes` is left alone unless
/// a `--test-plan` flag explicitly overrides it.
// trace:STORY-698 | ai:claude
pub(crate) fn capture_test_plan(
    storage: &Storage,
    target: &aida_core::Requirement,
    display_id: &str,
    flag_steps: &[String],
    no_test_plan: bool,
    interactive: bool,
) -> Result<()> {
    let req_id = target.id;
    // Cheap pre-checks (flags / opt-out) decide most cases without a load.
    let at_tty = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let disabled = capture_test_plan_disabled();

    // Idempotency only matters for the interactive path; check it there.
    let already_set = |storage: &Storage| -> bool {
        storage
            .load()
            .ok()
            .and_then(|store| {
                store
                    .requirements
                    .iter()
                    .find(|r| r.id == req_id)
                    .and_then(|r| r.implementation_info.as_ref())
                    .and_then(|info| info.test_coverage_notes.as_ref())
                    .map(|notes| !notes.trim().is_empty())
            })
            .unwrap_or(false)
    };

    // Resolve without touching the store when the flags already decide it.
    let decision = if !flag_steps.is_empty() {
        decide_test_plan_capture(
            flag_steps,
            no_test_plan,
            interactive,
            at_tty,
            disabled,
            false,
        )
    } else if no_test_plan || !interactive || !at_tty || disabled {
        TestPlanCapture::Skip
    } else {
        decide_test_plan_capture(
            flag_steps,
            no_test_plan,
            interactive,
            at_tty,
            disabled,
            already_set(storage),
        )
    };

    let notes = match decision {
        TestPlanCapture::Skip => return Ok(()),
        TestPlanCapture::Record(joined) => joined,
        TestPlanCapture::Prompt => {
            eprintln!();
            eprintln!(
                "{} How did you verify {}? (recorded as the PR's audit trail)",
                "→".cyan().bold(),
                display_id.bold()
            );
            eprintln!("  {} nothing to record? Just press Enter.", "·".dimmed());
            let lines = prompt_test_plan_steps();
            if lines.is_empty() {
                return Ok(());
            }
            lines.join("\n")
        }
    };

    let now = chrono::Utc::now();
    let step_count = notes.lines().filter(|l| !l.trim().is_empty()).count();
    // Per-spec compare-and-swap, no whole-store write. trace:BUG-1612 | ai:claude
    storage.update_spec_atomically(target, |r| {
        let info = r
            .implementation_info
            .get_or_insert_with(aida_core::ImplementationInfo::default);
        info.test_coverage_notes = Some(notes.clone());
        r.modified_at = now;
    })?;

    println!(
        "  {} verification steps captured for {} ({} step{})",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        display_id.bold(),
        step_count,
        if step_count == 1 { "" } else { "s" }
    );
    Ok(())
}

/// A plan's project-relative path in the form the store records it: `/`
/// separated on every OS. The followup marker, the plan-path dedup set and
/// the `followup-src:` tag compare these strings, so a Windows clone that
/// wrote `docs/plans\x.md` would neither match what a Unix clone filed nor
/// the `docs/plans/x.md` shape everything else uses. The agent-brief
/// `.pending` sentinel (BUG-466) stores its keys the same way.
// trace:BUG-1648 | ai:claude
pub(crate) fn plan_rel_path(path: &std::path::Path, project_root: &std::path::Path) -> String {
    let Ok(rel) = path.strip_prefix(project_root) else {
        return path.display().to_string();
    };
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Extract the Followups section of any plan owned by `spec_id` and file
/// the accepted bullets as child TASKs. Idempotent via [`FOLLOWUPS_MARKER`].
///
/// `interactive` drives a per-bullet `[y/N/skip]` prompt; when false (the
/// auto-bump / `--yes` path) every bullet is filed. Best-effort throughout —
/// any failure logs a warning and returns `Ok(())` so it never breaks the
/// `queue done` / `aida pull` flow it hangs off. trace:TASK-96 | ai:claude
pub(crate) fn extract_plan_followups(
    storage: &Storage,
    project_root: &std::path::Path,
    spec_id: &str,
    display_id: &str,
    interactive: bool,
) -> Result<()> {
    if auto_followups_disabled() {
        return Ok(());
    }
    let store = storage.load()?;
    let Some(req) = store.get_requirement_by_spec_id(spec_id) else {
        return Ok(());
    };
    // Already processed — whichever path ran first owns the outcome.
    if req
        .comments
        .iter()
        .any(|c| c.content.starts_with(FOLLOWUPS_MARKER))
    {
        return Ok(());
    }

    let plan_files = find_plan_files_for_spec(project_root, spec_id);
    if plan_files.is_empty() {
        return Ok(());
    }

    // BUG-656: plan-path stable signature — the cross-store guarantee. Build the
    // set of plan paths whose followups have ALREADY been extracted (by this
    // spec, a re-completion, or any sibling spec that also owns the plan), then
    // drop owned plans already in it. The per-spec marker comment above only
    // suppresses re-runs of the SAME spec; this keys dedup on the plan path so a
    // plan whose header lists several specs is extracted exactly once. Without
    // it, completing each EPIC-0428 design task re-extracted the shared plans
    // and re-filed their followups (TASK-1019-1032). trace:BUG-656 | ai:claude
    let already_extracted: std::collections::HashSet<String> = store
        .requirements
        .iter()
        .flat_map(|r| r.comments.iter())
        .flat_map(|c| parse_extracted_plans_from_marker(&c.content))
        .collect();
    let owned_rel_paths: Vec<String> = plan_files
        .iter()
        .map(|p| plan_rel_path(p, project_root))
        .collect();
    let pending: std::collections::HashSet<String> =
        plans_pending_extraction(&owned_rel_paths, &already_extracted)
            .into_iter()
            .collect();
    if pending.is_empty() {
        // Every owned plan's followups were already filed by an earlier
        // completion — re-completion / sibling-spec completion is a no-op.
        return Ok(());
    }
    let plan_files: Vec<std::path::PathBuf> = plan_files
        .into_iter()
        .filter(|p| pending.contains(&plan_rel_path(p, project_root)))
        .collect();

    // BUG-655: content-level dedup set — the `(parent_spec, title)` of every
    // child TASK the parent already has. The marker comment above is the
    // fast-path, but a plan committed by more than one slice PR can fire the
    // filing twice before the first marker syncs; this set is the real
    // guarantee, since the children are durable store rows. We grow it in the
    // filing loop too so a plan that lists the same bullet twice is also a
    // no-op the second time.
    let mut existing_children: Vec<(String, String)> = {
        use aida_core::models::RelationshipType;
        req.relationships
            .iter()
            .filter(|r| r.rel_type == RelationshipType::Parent)
            .filter_map(|r| store.get_requirement_by_id(&r.target_id))
            .map(|child| (spec_id.to_string(), child.title.clone()))
            .collect()
    };

    // BUG-656: secondary global-title guard. Collect the normalized titles of
    // every followup TASK already filed across the store — the child TASKs of
    // any spec that carries a FOLLOWUPS_MARKER. If the same bullet text was
    // copied into a distinct sibling plan (a different owning spec, a path the
    // plan-path signature above won't match), this stops it being re-filed under
    // the new parent. We grow it in the filing loop too. trace:BUG-656 | ai:claude
    let mut global_filed_titles: Vec<String> = {
        use aida_core::models::RelationshipType;
        let mut titles = Vec::new();
        for r in &store.requirements {
            if !r
                .comments
                .iter()
                .any(|c| c.content.starts_with(FOLLOWUPS_MARKER))
            {
                continue;
            }
            for rel in r
                .relationships
                .iter()
                .filter(|rel| rel.rel_type == RelationshipType::Parent)
            {
                if let Some(child) = store.get_requirement_by_id(&rel.target_id) {
                    titles.push(child.title.trim().to_ascii_lowercase());
                }
            }
        }
        titles
    };

    // BUG-680: shipped-followup guard. Every spec that a prior run filed carries
    // a `followup-src:<plan>` tag recording its origin plan; collect
    // (plan, title, id, is_terminal) for each so a bullet already filed from one
    // of THIS spec's plans that has since shipped (Completed) or been rejected is
    // not re-filed as a fresh open spec. This is the durable, store-backed
    // guarantee the parent's marker comment can't make: it survives the marker
    // being lost/unsynced and survives the followup being archived away from the
    // parent. `owned_plans` is the store-relative path set we match against.
    // trace:BUG-680 | ai:claude
    let owned_plans: std::collections::HashSet<String> = owned_rel_paths.iter().cloned().collect();
    let filed_from_plan: Vec<(String, String, String, bool)> = store
        .requirements
        .iter()
        .flat_map(|r| {
            let title = r.title.clone();
            let id = r.display_id();
            let terminal = matches!(
                r.status,
                RequirementStatus::Completed | RequirementStatus::Rejected
            );
            r.tags
                .iter()
                .filter_map(|t| t.strip_prefix(FOLLOWUP_SRC_TAG_PREFIX))
                // trace:BUG-1648 | ai:claude
                .map(move |plan| {
                    (
                        normalize_recorded_plan_path(plan),
                        title.clone(),
                        id.clone(),
                        terminal,
                    )
                })
        })
        .collect();

    // Collect + dedupe followup bullets across every owning plan file. Each
    // bullet keeps the first plan it appeared in as its source, so a filed
    // followup can be stamped with its origin plan (BUG-680).
    let mut followups: Vec<(String, String)> = Vec::new(); // (bullet, source plan)
    let mut sources: Vec<String> = Vec::new();
    for path in &plan_files {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let parsed = parse_plan_followups(&content);
        if parsed.is_empty() {
            continue;
        }
        let rel = plan_rel_path(path, project_root);
        sources.push(rel.clone());
        for f in parsed {
            if !followups.iter().any(|(b, _)| b == &f) {
                followups.push((f, rel.clone()));
            }
        }
    }
    if followups.is_empty() {
        return Ok(());
    }

    println!();
    println!(
        "{} {} plan followup(s) found for {} ({})",
        "→".cyan().bold(),
        followups.len(),
        display_id.bold(),
        sources.join(", ").dimmed()
    );

    let mut filed: Vec<(String, String)> = Vec::new(); // (new_spec, title)
    let mut declined: Vec<String> = Vec::new();
    let mut deduped: Vec<String> = Vec::new(); // BUG-655: already-filed bullets
    let mut shipped: Vec<(String, String)> = Vec::new(); // BUG-680: (bullet, existing id)
    let mut pointer_skips: Vec<(String, String)> = Vec::new(); // BUG-1142: (bullet, existing id)
    let mut meta_skips: Vec<(String, &'static str)> = Vec::new(); // BUG-1142: (bullet, opener)
    let mut skip_rest = false;

    for (followup, source_plan) in &followups {
        match classify_plan_followup(&store, followup) {
            Some(FollowupSkip::PointerToExistingSpec(id)) => {
                pointer_skips.push((followup.clone(), id));
                continue;
            }
            Some(FollowupSkip::MetaProse(opener)) => {
                meta_skips.push((followup.clone(), opener));
                continue;
            }
            None => {}
        }
        // BUG-680: a bullet already filed from one of this spec's plans that has
        // since shipped (Completed) or been rejected — re-filing it would open a
        // duplicate for work that already ran its course. Skip and link to the
        // shipped spec. Checked first so the audit trail attributes it to the
        // shipped guard rather than the generic already-filed one.
        if let Some(existing_id) =
            followup_shipped_from_plan(&filed_from_plan, &owned_plans, followup)
        {
            shipped.push((followup.clone(), existing_id.to_string()));
            continue;
        }
        // BUG-655: a bullet whose title already exists as a child of this
        // parent has already been filed (this run, a prior run, or a sibling
        // commit's run) — skip it without prompting, so a plan landed by
        // multiple PRs cannot double-file. This is the cross-commit guarantee
        // the per-spec marker comment cannot make.
        // BUG-656: also skip a bullet whose title was already filed as a
        // followup TASK ANYWHERE in the store — the same text copied into a
        // sibling plan owned by a different spec must not become a duplicate.
        if followup_already_filed(&existing_children, spec_id, followup)
            || followup_filed_anywhere(&global_filed_titles, followup)
        {
            deduped.push(followup.clone());
            continue;
        }

        let accept = if skip_rest {
            false
        } else if interactive {
            eprint!("  File as TASK? [y/N/skip] {}\n  > ", followup.bold());
            use std::io::Write;
            let _ = std::io::stderr().flush();
            let mut answer = String::new();
            if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
                skip_rest = true;
                false
            } else {
                match answer.trim().to_ascii_lowercase().as_str() {
                    "y" | "yes" => true,
                    "s" | "skip" => {
                        skip_rest = true;
                        false
                    }
                    _ => false,
                }
            }
        } else {
            true
        };

        if accept {
            match aida_subcmd_add_followup_task(project_root, spec_id, followup, Some(source_plan))
            {
                Some(new_id) => {
                    // Record the title so a later identical bullet in the same
                    // plan is recognised as already-filed (BUG-655), both
                    // per-parent and globally (BUG-656).
                    existing_children.push((spec_id.to_string(), followup.clone()));
                    global_filed_titles.push(followup.trim().to_ascii_lowercase());
                    filed.push((new_id, followup.clone()));
                }
                // BUG-1625: a failed add is not automatically a decline — the
                // followup may already exist in the (now fresher) store, filed
                // by another clone. Re-read and record it as already-filed so
                // pull output never reports an existing task as declined.
                // trace:BUG-1625 | ai:claude
                None => {
                    // BUG-1633: only THIS plan's provenance tag counts.
                    // trace:BUG-1633 | ai:claude
                    let exists = storage
                        .load()
                        .map(|fresh| {
                            followup_filed_in_store(&fresh, spec_id, followup, source_plan)
                        })
                        .unwrap_or(false);
                    if exists {
                        deduped.push(followup.clone());
                    } else {
                        declined.push(followup.clone());
                    }
                }
            }
        } else {
            declined.push(followup.clone());
        }
    }

    // Marker comment — records the outcome so re-runs and the other
    // extraction path both skip, and the user can see what was declined.
    let mut marker = format!("{} extracted from {}", FOLLOWUPS_MARKER, sources.join(", "));
    if filed.is_empty() {
        marker.push_str("\nfiled 0 task(s)");
    } else {
        let ids: Vec<&str> = filed.iter().map(|(id, _)| id.as_str()).collect();
        marker.push_str(&format!(
            "\nfiled {} task(s): {}",
            filed.len(),
            ids.join(", ")
        ));
    }
    if !declined.is_empty() {
        marker.push_str(&format!("\ndeclined {}:", declined.len()));
        for d in &declined {
            marker.push_str(&format!("\n  - {d}"));
        }
    }
    if !deduped.is_empty() {
        // BUG-655: bullets skipped because an identical child TASK already
        // existed — recorded so the audit trail shows the double-file was
        // prevented, not silently dropped.
        marker.push_str(&format!("\nskipped {} already-filed:", deduped.len()));
        for d in &deduped {
            marker.push_str(&format!("\n  - {d}"));
        }
    }
    if !shipped.is_empty() {
        // BUG-680: bullets skipped because a followup filed from the same plan
        // already shipped (Completed/Rejected) — linked to the existing spec so
        // the audit trail shows the re-file was prevented, not lost.
        marker.push_str(&format!("\nskipped {} already-shipped:", shipped.len()));
        for (bullet, id) in &shipped {
            marker.push_str(&format!("\n  - {bullet} → {id}"));
        }
    }
    if !pointer_skips.is_empty() {
        marker.push_str(&format!(
            "\nskipped {} pointer-to-existing-spec:",
            pointer_skips.len()
        ));
        for (bullet, id) in &pointer_skips {
            marker.push_str(&format!("\n  - {bullet} → {id}"));
        }
    }
    if !meta_skips.is_empty() {
        marker.push_str(&format!("\nskipped {} meta-prose:", meta_skips.len()));
        for (bullet, opener) in &meta_skips {
            marker.push_str(&format!("\n  - {bullet} (opener: {opener})"));
        }
    }
    let now = chrono::Utc::now();
    let author = get_default_author();
    // Per-spec compare-and-swap, no whole-store write. trace:BUG-1612 | ai:claude
    let _ = storage.update_spec_atomically(req, |r| {
        r.comments
            .push(Comment::new(author.clone(), marker.clone()));
        r.modified_at = now;
    });

    if filed.is_empty() {
        println!("  {} no followups filed", "·".dimmed());
    } else {
        for (id, title) in &filed {
            println!(
                "  {} {} {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                id.bold(),
                title.dimmed()
            );
        }
    }
    if !declined.is_empty() {
        println!(
            "  {} {} followup(s) not filed — logged on {} for later review",
            "·".dimmed(),
            declined.len(),
            display_id
        );
    }
    if !deduped.is_empty() {
        println!(
            "  {} {} followup(s) already filed as child task(s) — skipped",
            "·".dimmed(),
            deduped.len(),
        );
    }
    if !shipped.is_empty() {
        for (bullet, id) in &shipped {
            println!(
                "  {} already shipped as {} — skipped {}",
                "·".dimmed(),
                id.bold(),
                bullet.dimmed(),
            );
        }
    }
    if !pointer_skips.is_empty() {
        for (bullet, id) in &pointer_skips {
            println!(
                "  {} pointer-to-existing-spec {} — skipped {}",
                "·".dimmed(),
                id.bold(),
                bullet.dimmed(),
            );
        }
    }
    if !meta_skips.is_empty() {
        for (bullet, opener) in &meta_skips {
            println!(
                "  {} meta-prose opener `{}` — skipped {}",
                "·".dimmed(),
                opener,
                bullet.dimmed(),
            );
        }
    }

    Ok(())
}

/// Best-effort `aida rel add <from> <to> --type implements` (and the
/// matching reverse `implemented-by` so `aida show <spec>` surfaces the
/// review story). Custom relation types don't have an `inverse()` mapping
/// in core, so `--bidirectional` is a no-op for them — we add both
/// directions manually instead. Logs and continues on failure.
/// trace:STORY-66 | ai:claude
pub(crate) fn aida_subcmd_rel_add_implements(project_root: &std::path::Path, from: &str, to: &str) {
    aida_subcmd_rel_add(project_root, from, to, "implements");
    aida_subcmd_rel_add(project_root, to, from, "implemented-by");
}

pub(crate) fn aida_subcmd_rel_add(
    project_root: &std::path::Path,
    from: &str,
    to: &str,
    rel_type: &str,
) {
    let aida = aida_exe_path();
    match std::process::Command::new(&aida)
        .current_dir(project_root)
        .args(["rel", "add", from, to, "--type", rel_type])
        .output_retrying_etxtbsy()
    {
        Ok(o) if o.status.success() => {}
        Ok(o) => eprintln!(
            "{} auto-queue: `rel add {} {} --type {}` failed: {}",
            "Warning:".yellow().bold(),
            from,
            to,
            rel_type,
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => eprintln!(
            "{} auto-queue: could not invoke `aida rel add`: {}",
            "Warning:".yellow().bold(),
            e
        ),
    }
}

/// `aida queue add <id> --for reviewer --no-scope --note <...>`.
///
/// The result is deliberately not best-effort: callers must not report a PR
/// as handed off until the reviewer queue durably owns it.
// trace:STORY-66 trace:BUG-1291 | ai:claude ai:codex
pub(crate) fn aida_subcmd_queue_add_for_reviewer(
    project_root: &std::path::Path,
    spec_id: &str,
    note: &str,
) -> anyhow::Result<()> {
    aida_subcmd_queue_add_for_reviewer_using(project_root, spec_id, note, &aida_exe_path())
}

pub(crate) fn aida_subcmd_queue_add_for_reviewer_using(
    project_root: &std::path::Path,
    spec_id: &str,
    note: &str,
    aida: &std::path::Path,
) -> anyhow::Result<()> {
    let out = std::process::Command::new(aida)
        .current_dir(project_root)
        .args([
            "queue",
            "add",
            spec_id,
            "--for",
            "reviewer",
            "--no-scope",
            "--note",
            note,
        ])
        // The injected `aida` may be a fixture this process just wrote, so a
        // sibling thread's forked child can still hold a writer descriptor on
        // it. Without the retry that surfaces as `could not invoke`, which is
        // the *other* error arm from the non-zero-exit one callers care about.
        // trace:BUG-1735 | ai:claude
        .output_retrying_etxtbsy();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => anyhow::bail!(
            "`aida queue add {spec_id}` failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => Err(e).context("could not invoke `aida queue add`"),
    }
}

/// What `try_auto_queue_pr_review` did, in four buckets that map to the
/// caller's display formatting (green check / cyan info / dim by-design /
/// yellow attention-needed). The fourth bucket (TASK-74) was carved out
/// of the original BUG-72 "Skipped" bucket so review sessions (whose
/// branches don't produce PRs by design) stop reading as failures, and
/// missing-tool / lookup-failed cases stop reading as routine.
/// trace:BUG-72 TASK-74 | ai:claude
pub(crate) enum AutoQueueStatus {
    /// New review story filed and queued for reviewer.
    Filed,
    /// PR found but a `Review PR-N` story already exists — idempotent re-run.
    AlreadyExists,
    /// Skipped on purpose — this session shape never produces a PR (e.g.
    /// reviewer session on a `pr-N` branch). Dimmed in output: nothing to
    /// fix, the user just needs to know the hook checked and stepped aside.
    /// trace:TASK-74 | ai:claude
    SkippedByDesign,
    /// Skipped but the user might want to do something about it: `gh`
    /// missing, `gh pr list` failed, or the queue/rel-add subprocess
    /// errored. Rendered in yellow so it doesn't blend into the by-design
    /// noise floor. trace:TASK-74 | ai:claude
    SkippedNeedsAttention,
}

/// Outcome of the end-of-session auto-queue. Always returns SOMETHING so
/// `session_end` can render a clear status line. trace:BUG-72 | ai:claude
pub(crate) struct AutoQueueOutcome {
    pub(crate) status: AutoQueueStatus,
    pub(crate) summary: String,
    /// STORY-106: PR number this outcome relates to (set on Filed and
    /// AlreadyExists). Drives the post-session "start the review"
    /// workflow hint. `None` for the skip cases.
    /// trace:STORY-106 | ai:claude
    pub(crate) pr_number: Option<u64>,
    /// TASK-267: delivered `(REQ-ID)` trailers the PR carries. Lets the
    /// post-session workflow hint name which specs `aida pull` will
    /// auto-bump Done → Completed. Populated on Filed; empty otherwise.
    /// trace:TASK-267 | ai:claude
    pub(crate) covered_specs: Vec<String>,
    /// Canonical story selected or filed for this change request.
    /// trace:BUG-1817 | ai:codex
    pub(crate) review_spec: Option<String>,
}

impl AutoQueueOutcome {
    pub(crate) fn filed(s: impl Into<String>) -> Self {
        Self {
            status: AutoQueueStatus::Filed,
            summary: s.into(),
            pr_number: None,
            covered_specs: Vec::new(),
            review_spec: None,
        }
    }
    pub(crate) fn already_exists(s: impl Into<String>) -> Self {
        Self {
            status: AutoQueueStatus::AlreadyExists,
            summary: s.into(),
            pr_number: None,
            covered_specs: Vec::new(),
            review_spec: None,
        }
    }
    /// STORY-106: attach the PR number that this outcome refers to. Used
    /// by `session_end` to emit a concrete "start the review" hint.
    pub(crate) fn with_pr(mut self, pr_number: u64) -> Self {
        self.pr_number = Some(pr_number);
        self
    }
    /// TASK-267: attach the delivered spec IDs the PR covers, so the
    /// session-end hint names them in the `aida pull` auto-bump line.
    /// trace:TASK-267 | ai:claude
    pub(crate) fn with_specs(mut self, specs: Vec<String>) -> Self {
        self.covered_specs = specs;
        self
    }
    pub(crate) fn with_review_spec(mut self, spec: impl Into<String>) -> Self {
        self.review_spec = Some(spec.into());
        self
    }
    /// Skip the user shouldn't care about (review session, no PR-producing
    /// branch). Rendered dim. trace:TASK-74 | ai:claude
    pub(crate) fn skipped_by_design(s: impl Into<String>) -> Self {
        Self {
            status: AutoQueueStatus::SkippedByDesign,
            summary: s.into(),
            pr_number: None,
            covered_specs: Vec::new(),
            review_spec: None,
        }
    }
    /// Skip the user might want to act on (missing tool, gh failed, queue
    /// subprocess error). Rendered yellow. trace:TASK-74 | ai:claude
    pub(crate) fn skipped_needs_attention(s: impl Into<String>) -> Self {
        Self {
            status: AutoQueueStatus::SkippedNeedsAttention,
            summary: s.into(),
            pr_number: None,
            covered_specs: Vec::new(),
            review_spec: None,
        }
    }
}

/// Heuristic: does this branch shape correspond to a session that, by
/// design, never produces a PR? Today: `pr-N`, `mr-N`, `github-N`,
/// `gitlab-N` — the local branch names `aida session start --owns PR-N`
/// creates for review sessions. Case-insensitive. trace:TASK-74 | ai:claude
pub(crate) fn is_review_session_branch(branch: &str) -> bool {
    let lower = branch.to_ascii_lowercase();
    let prefixes = ["pr-", "mr-", "github-", "gitlab-"];
    prefixes.iter().any(|p| {
        lower
            .strip_prefix(p)
            .map(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false)
    })
}

/// Which pre-flight condition, if any, says "do NOT file a review story".
///
/// The auto-file used to run unconditionally, so a repo with no `origin`
/// remote produced a `Review PR-0: ` story with an empty subject covering
/// no specs — on EVERY `aida session end`. Three conditions gate it now,
/// each of them meaning "there is no reviewable change here":
///
/// - no `origin` remote — a local-only project has nothing to open a PR
///   against (the same `has_remote(.., "origin")` condition `aida db sync
///   --pull` uses to skip its pull),
/// - the resolved change id is `0` — the pure-git provider's synthetic
///   "the branch IS the change" ref, i.e. the forge has no PR concept,
/// - the story would cover no specs — no `(REQ-ID)` trailers anywhere in
///   the PR's commit range, so the reviewer story carries no linkage.
///
/// Pure so the policy is unit-testable without a git repo or a forge.
/// `pr_number` / `covered_specs` are `None` at the call sites that run
/// before those facts are known.
// trace:BUG-776 | ai:claude
/// BUG-1223: map the detected forge onto the review-side forge enum. GitLab
/// keeps its own arm; GitHub and pure-git both take the GitHub arm (pure-git
/// has no PR concept and is filtered out earlier by the synthetic change id).
/// Pure so the mapping is unit-testable.
// trace:BUG-1223 | ai:claude
pub(crate) fn review_forge_for_kind(kind: crate::forge::ForgeKind) -> ReviewForge {
    match kind {
        crate::forge::ForgeKind::GitLab => ReviewForge::GitLab,
        _ => ReviewForge::GitHub,
    }
}

pub(crate) fn auto_queue_skip_reason(
    has_origin_remote: bool,
    pr_number: Option<u64>,
    covered_specs: Option<usize>,
) -> Option<String> {
    if !has_origin_remote {
        return Some(
            "auto-queue: no `origin` remote (local-only project) — no PR can exist, so no reviewer story was filed"
                .to_string(),
        );
    }
    if pr_number == Some(0) {
        return Some(
            "auto-queue: this forge has no pull-request concept (synthetic change id 0) — no reviewer story was filed"
                .to_string(),
        );
    }
    if let (Some(n), Some(0)) = (pr_number, covered_specs) {
        return Some(format!(
            "auto-queue: PR #{n} carries no `(REQ-ID)` trailers in its commit range — no reviewer story was filed (add a trailer, then re-run `aida pr auto-queue-review`)"
        ));
    }
    None
}

#[cfg(test)]
#[path = "tests/bug776_auto_queue_gate_tests.rs"]
mod bug776_auto_queue_gate_tests;

/// Where the auto-queue was triggered from. Used purely for the
/// description blurb on the filed review story (so a reviewer reading
/// it later can tell "the /aida-pr skill filed me at PR-create time"
/// from "session end picked me up as a backup"). Doesn't change any
/// other behavior. trace:STORY-90 | ai:claude
#[derive(Debug, Clone, Copy)]
pub(crate) enum AutoQueueOrigin {
    /// Fired from `aida pr auto-queue-review`, normally by /aida-pr right
    /// after `gh pr create`.
    PrSkill,
    /// Fired from `aida session end` as the idempotent backup trigger.
    SessionEnd,
}

impl AutoQueueOrigin {
    pub(crate) fn human_origin(self) -> &'static str {
        match self {
            AutoQueueOrigin::PrSkill => "`aida pr auto-queue-review` (typically from /aida-pr)",
            AutoQueueOrigin::SessionEnd => "`aida session end` (backup trigger)",
        }
    }
}

/// Auto-detect-and-queue. Triggered primarily from /aida-pr right after
/// `gh pr create` returns, and as an idempotent backup from `aida session
/// end` so a forgotten `gh pr create` (or a manual one outside the skill)
/// doesn't leave the reviewer unaware. Returns an outcome describing what
/// happened — see `AutoQueueStatus` for the categories.
/// trace:STORY-66 STORY-90 BUG-72 | ai:claude
/// Pick a working directory the end-of-session auto-queue can safely
/// shell out from. It runs `gh` / `git` / `aida` subprocesses, every one
/// of which fails with a misleading ENOENT (blamed on the binary, not the
/// cwd) if its working directory was removed. `aida session end` removes
/// the session worktree, so when that worktree was the invocation dir we
/// fall back to the lease's recorded parent project root — the main
/// worktree, which is never removed. Returns the first candidate that is
/// still a real directory, or `None` when both are gone.
/// trace:BUG-107 | ai:claude
pub(crate) fn auto_queue_working_dir(
    parent_project_root: Option<&std::path::Path>,
    invocation_root: &std::path::Path,
) -> Option<std::path::PathBuf> {
    [parent_project_root, Some(invocation_root)]
        .into_iter()
        .flatten()
        .find(|p| p.is_dir())
        .map(|p| p.to_path_buf())
}

pub(crate) fn try_auto_queue_pr_review(
    project_root: &std::path::Path,
    branch: &str,
    session_id: &str,
    origin: AutoQueueOrigin,
) -> AutoQueueOutcome {
    // TASK-74: short-circuit when the branch shape says "this session
    // never produces a PR" (reviewer session on `pr-N` / `mr-N` etc.).
    // Catches the case before we even call gh, so a reviewer with `gh`
    // uninstalled doesn't get a misleading "gh not found" warning when
    // the right answer is "we wouldn't have filed anyway".
    if is_review_session_branch(branch) {
        return AutoQueueOutcome::skipped_by_design(format!(
            "auto-queue: reviewer session on `{}` — no PR to file (skip by design)",
            branch
        ));
    }

    // BUG-776: a repo with no `origin` remote cannot have a PR, but the
    // pure-git forge provider still answers `change_for_branch` with a
    // synthetic id-0 ChangeRef — which used to mint a `Review PR-0: ` story
    // covering no specs on every single `aida session end`. Gate the whole
    // path on the same remote check `aida db sync --pull` uses before it
    // even talks to the forge. trace:BUG-776 | ai:claude
    if let Some(reason) = auto_queue_skip_reason(
        aida_core::git_ops::has_remote(project_root, "origin"),
        None,
        None,
    ) {
        return AutoQueueOutcome::skipped_by_design(reason);
    }

    // trace:BUG-1807 | ai:codex
    let kind = crate::forge::resolve_forge_kind(project_root);
    let noun = kind.change_noun();
    let cli = kind.cli_name();

    // STORY-516: forge-routed. Reconstruct OpenPrInfo from the ChangeRef so the
    // downstream pr.number/url/title uses stay unchanged. trace:STORY-516 | ai:claude
    let pr = match change_lookup_for_branch(project_root, branch) {
        crate::forge::ChangeLookup::Found(c) => OpenPrInfo {
            number: c.id,
            title: c.title.unwrap_or_default(),
            url: c.url,
            head_branch: (!c.branch.is_empty()).then_some(c.branch),
        },
        crate::forge::ChangeLookup::NoChange => {
            return AutoQueueOutcome::skipped_by_design(format!(
                "auto-queue: no open {noun} for branch `{}` — reviewer queue not filed",
                branch
            ));
        }
        crate::forge::ChangeLookup::CliMissing => {
            return AutoQueueOutcome::skipped_needs_attention(format!(
                "auto-queue: `{cli}` CLI not on PATH — would have queued reviewer story for branch `{}`. Install {cli} to enable.",
                branch
            ));
        }
        crate::forge::ChangeLookup::CliFailed(reason) => {
            return AutoQueueOutcome::skipped_needs_attention(format!(
                "auto-queue: `{cli}` lookup failed for branch `{}` ({}) — no reviewer story filed",
                branch, reason
            ));
        }
        crate::forge::ChangeLookup::Unreachable(reason) => {
            return AutoQueueOutcome::skipped_needs_attention(format!(
                "auto-queue: {noun} API unreachable for branch `{}` ({}) — no reviewer story filed (transient; retry once the API is reachable)",
                branch, reason
            ));
        }
    };
    // BUG-776: a remote that is not a PR-speaking forge (bare git server,
    // GitLab-over-plain-git) resolves to the synthetic id-0 ChangeRef. There
    // is no PR #0 to review — skip before the base/head probe so we don't
    // also emit its "couldn't resolve PR base/head" warning.
    // trace:BUG-776 | ai:claude
    if let Some(reason) = auto_queue_skip_reason(true, Some(pr.number), None) {
        return AutoQueueOutcome::skipped_by_design(reason);
    }
    // Pull the commit-range spec ids using the existing helpers from STORY-67.
    // BUG-85: split into "delivered" (subject `(REQ-ID)` parens) and
    // "referenced" (body content / trace comments). Only delivered IDs count
    // toward "covers N specs" or get an `implements` relation — referenced
    // IDs are informational (spot-check for regressions).
    // BUG-1223: this used to hard-code `ReviewForge::GitHub`, so on a GitLab
    // project the base/head lookup ran `gh pr view` against the wrong forge,
    // the fallback range covered nothing, and a trailered MR was reported as
    // "carries no `(REQ-ID)` trailers" — no `Review PR-N` story, so the
    // drain's reviewer phase had nothing to pick up. Resolve the forge the
    // same way the review-prompt path does (config `[forge]`, else origin).
    // trace:BUG-1223 trace:TASK-1254 | ai:claude
    let review_forge = review_forge_for_kind(kind);
    let (base, head) = pr_base_head(project_root, review_forge, pr.number)
        .unwrap_or_else(|_| ("main".to_string(), branch.to_string()));
    let messages = git_log_messages(project_root, &base, &head).unwrap_or_default();
    let mut spec_ids: Vec<String> = Vec::new();
    let mut referenced_ids: Vec<String> = Vec::new();
    for msg in &messages {
        for id in extract_spec_ids_from_commit(msg) {
            if !spec_ids.iter().any(|x| x.eq_ignore_ascii_case(&id)) {
                spec_ids.push(id);
            }
        }
        for id in extract_referenced_spec_ids_from_commit(msg) {
            if !referenced_ids.iter().any(|x| x.eq_ignore_ascii_case(&id)) {
                referenced_ids.push(id);
            }
        }
    }
    // A referenced ID delivered by ANOTHER commit in the same range is
    // still delivered overall — drop it from references to keep the lists
    // disjoint across the range. trace:BUG-85 | ai:claude
    referenced_ids.retain(|r| !spec_ids.iter().any(|d| d.eq_ignore_ascii_case(r)));

    // BUG-776: a story that covers no specs is unlinked noise in the reviewer
    // queue — the reviewer has nothing to check acceptance against, and the
    // row has to be dequeued and rejected by hand. Skip it and say why, so the
    // fix ("add a `(REQ-ID)` trailer") is obvious. trace:BUG-776 | ai:claude
    if let Some(reason) = auto_queue_skip_reason(true, Some(pr.number), Some(spec_ids.len())) {
        return AutoQueueOutcome::skipped_by_design(reason);
    }

    // Read canonical objects for both distributed and legacy stores. A failed
    // lookup cannot establish absence: do not file a duplicate on read failure.
    // trace:BUG-1807 | ai:codex
    let store = match load_review_story_inventory(project_root) {
        Ok(store) => store,
        Err(err) => {
            return AutoQueueOutcome::skipped_needs_attention(format!(
                "auto-queue: cannot read review stories for {noun}-{} — reviewer handoff not confirmed: {err:#}",
                pr.number
            ));
        }
    };
    {
        let persisted =
            drain_state::DrainState::read(project_root).and_then(|state| state.review_spec);
        if let Some(existing) = canonical_review_story(
            &store,
            review_forge,
            pr.number,
            Some(&spec_ids),
            persisted.as_deref(),
        ) {
            let existing_id = existing.display_id();
            let note = format!(
                "auto-queue retry reused canonical review story; covers {} spec{}",
                spec_ids.len(),
                if spec_ids.len() == 1 { "" } else { "s" }
            );
            if let Err(err) = aida_subcmd_queue_add_for_reviewer(project_root, &existing_id, &note)
            {
                return AutoQueueOutcome::skipped_needs_attention(format!(
                        "auto-queue: canonical review story {existing_id} exists for {} but reviewer queue insertion failed: {err}",
                        format_review_label(review_forge, pr.number)
                    ))
                    .with_pr(pr.number)
                    .with_specs(spec_ids)
                    .with_review_spec(existing_id);
            }
            return AutoQueueOutcome::already_exists(format!(
                "{} #{} reuses canonical review story {}",
                format_review_label(review_forge, pr.number)
                    .split_once('-')
                    .map(|(prefix, _)| prefix)
                    .unwrap_or("PR"),
                pr.number,
                existing_id
            ))
            .with_pr(pr.number)
            .with_specs(spec_ids)
            .with_review_spec(existing_id);
        }
    }

    let session_short: &str = &session_id[..session_id.len().min(8)];
    let mut desc = String::new();
    desc.push_str(&format!(
        "Auto-filed by {} (session `{}`) for branch `{}`.\n\n",
        origin.human_origin(),
        session_short,
        branch
    ));
    desc.push_str(&format!("- {noun}: <{}>\n", pr.url));
    desc.push_str(&format!("- Branch: `{}` → `{}`\n\n", head, base));
    if spec_ids.is_empty() {
        // BUG-776 gates this branch off — a zero-coverage story is no longer
        // filed at all. Kept as a defensive fallback so relaxing the gate
        // can't ship a story with an empty body. trace:BUG-776 | ai:claude
        desc.push_str(
            "No `(REQ-ID)` trailers were found in the PR's commit range — review against the PR title/body and link specs after the fact via `aida rel add <this> <spec> --type implements`.\n\n",
        );
    } else {
        desc.push_str("## Covers\n\n");
        for id in &spec_ids {
            desc.push_str(&format!("- {}\n", id));
        }
        desc.push('\n');
    }
    if !referenced_ids.is_empty() {
        desc.push_str("## References (not delivered)\n\n");
        desc.push_str(
            "Spec IDs mentioned in commit bodies or trace comments — touched but not delivered by this PR. Spot-check for regressions.\n\n",
        );
        for id in &referenced_ids {
            desc.push_str(&format!("- {}\n", id));
        }
        desc.push('\n');
    }
    desc.push_str("## Acceptance\n\n");
    desc.push_str(&format!(
        "- Generate a structured review prompt with `aida review prompt --pr {}` and verify each spec's acceptance criteria.\n",
        pr.number
    ));
    desc.push_str("- Approve and merge, or request changes by spec id.\n");
    desc.push_str("- Mark this story `completed` once the PR is merged.\n");

    // BUG-1609: the story title carries the forge-correct label ("Review
    // MR-N" for GitLab), not a hardcoded "Review PR-N". Downstream,
    // `parse_review_scope` reads this prefix back into `plan.review_target`'s
    // `ReviewForge` — a GitHub-shaped title on a GitLab MR silently made the
    // whole reviewer-preflight chain (stale-base check, intermediate-only
    // check, `pr_base_head`) treat the MR as a GitHub PR and shell out to
    // `gh`, which then fabricated a GitHub-shaped `pr-N` fallback head for a
    // branch `gh` had never heard of. trace:BUG-1609 | ai:claude
    let title = format!(
        "Review {}: {}",
        format_review_label(review_forge, pr.number),
        pr.title
    );
    let new_id = match aida_subcmd_add_review_story(project_root, &title, &desc) {
        Some(id) => id,
        None => {
            return AutoQueueOutcome::skipped_needs_attention(format!(
                "auto-queue: `aida add` failed for {noun}-{} (see warning above)",
                pr.number
            ));
        }
    };

    for id in &spec_ids {
        aida_subcmd_rel_add_implements(project_root, &new_id, id);
    }

    let origin_short = match origin {
        AutoQueueOrigin::PrSkill => "aida pr",
        AutoQueueOrigin::SessionEnd => "session end",
    };
    let note = format!(
        "auto-queued by `{}` ({}); covers {} spec{}",
        origin_short,
        session_short,
        spec_ids.len(),
        if spec_ids.len() == 1 { "" } else { "s" }
    );
    if let Err(err) = aida_subcmd_queue_add_for_reviewer(project_root, &new_id, &note) {
        return AutoQueueOutcome::skipped_needs_attention(format!(
            "auto-queue: filed {new_id} for {noun}-{} but reviewer queue insertion failed: {err}; the unqueued story remains a durable retry signal",
            pr.number
        ))
        .with_pr(pr.number)
        .with_specs(spec_ids)
        .with_review_spec(new_id);
    }

    let covers = if spec_ids.is_empty() {
        "no specs".to_string()
    } else {
        spec_ids.join(", ")
    };
    AutoQueueOutcome::filed(format!(
        "filed {} (covers {}) → reviewer queue ({noun}-{})",
        new_id, covers, pr.number
    ))
    .with_pr(pr.number)
    .with_specs(spec_ids)
    .with_review_spec(new_id)
}

// trace:BUG-1807 | ai:codex
fn require_review_handoff(outcome: &AutoQueueOutcome) -> Result<(), auto_complete::PhaseFailure> {
    match outcome.status {
        AutoQueueStatus::Filed | AutoQueueStatus::AlreadyExists => Ok(()),
        _ => Err(auto_complete::PhaseFailure::new(format!(
            "reviewer handoff not confirmed: {}; retry `aida pr auto-queue-review`",
            outcome.summary
        ))),
    }
}

/// Print an `AutoQueueOutcome` in the convention shared by `aida session
/// end` and `aida pr auto-queue-review` (filed = green check, already-exists
/// = cyan info, by-design skip = dim, needs-attention = yellow warning).
/// trace:STORY-90 BUG-72 | ai:claude
pub(crate) fn render_auto_queue_outcome(outcome: &AutoQueueOutcome) {
    match outcome.status {
        AutoQueueStatus::Filed => println!(
            "{} {}",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            outcome.summary
        ),
        AutoQueueStatus::AlreadyExists => println!(
            "{} {}",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            outcome.summary
        ),
        AutoQueueStatus::SkippedByDesign => {
            eprintln!(
                "{} {}",
                crate::glyph(crate::glyphs::Glyph::Info).dimmed(),
                outcome.summary.dimmed()
            )
        }
        AutoQueueStatus::SkippedNeedsAttention => {
            eprintln!(
                "{} {}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                outcome.summary.yellow()
            )
        }
    }
}

/// `git -C <root> branch --show-current` — used by `aida pr` when the
/// caller doesn't pass --branch. Returns trimmed branch name on success;
/// empty string when detached / not a repo. trace:STORY-90 | ai:claude
pub(crate) fn current_git_branch(project_root: &std::path::Path) -> Result<String> {
    let out = std::process::Command::new("git")
        .current_dir(project_root)
        .args(["branch", "--show-current"])
        .output()
        .context("could not invoke `git branch --show-current` — is git installed?")?;
    if !out.status.success() {
        anyhow::bail!(
            "`git branch --show-current` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// BUG-677: `LeaseState` + `classify_lease_state` moved to `aida_core::liveness`
/// (re-exported at the crate root, see the top-of-file `pub(crate) use`). The
/// pure enum + classifier is now shared with `aida-tui`; only the CLI glyph
/// rendering stays here, as an extension trait (glyph() reaches into the CLI
/// glyph palette, which aida-core has no business knowing). `label()` moved with
/// the enum (pure strings).
// trace:BUG-677 | ai:claude
pub(crate) trait LeaseStateGlyph {
    fn glyph(self) -> &'static str;
}

impl LeaseStateGlyph for LeaseState {
    fn glyph(self) -> &'static str {
        match self {
            LeaseState::Live => "●",
            LeaseState::Dormant => crate::glyph(crate::glyphs::Glyph::InFlight),
            LeaseState::Stale => crate::glyph(crate::glyphs::Glyph::Warning),
        }
    }
}

/// TASK-56: find the Claude Code session id matching a worktree path,
/// derived from the newest `*.jsonl` file at
/// `~/.claude/projects/<encoded-cwd>/`. Returns None when the project
/// dir doesn't exist or has no jsonl files. trace:TASK-56 | ai:claude
pub(crate) fn cc_session_id_for_worktree(worktree: &std::path::Path) -> Option<String> {
    let dir = session::claude_project_dir(worktree).ok()?;
    if !dir.exists() {
        return None;
    }
    let mut newest: Option<(std::time::SystemTime, String)> = None;
    for entry in std::fs::read_dir(&dir).ok()? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if stem.is_empty() {
            continue;
        }
        match &newest {
            Some((best_mtime, _)) if mtime <= *best_mtime => {}
            _ => newest = Some((mtime, stem)),
        }
    }
    newest.map(|(_, s)| s)
}

/// STORY-637: render the cross-clone lease section — claims on the shared
/// `aida-store` registry held by OTHER clones (a different clone path than
/// ours). Returns the number of foreign claims shown. Distinct from the
/// local-lease table above it. trace:STORY-637 | ai:claude
///
/// BUG-1764: stale claims are now EXCLUDED by default. This section applied no
/// expiry predicate of any kind — its only filter was `clone_path != ours`, and
/// while it computed `age` from `heartbeat_at` for the display column it never
/// used it as a predicate, so a foreign claim rendered as active forever once
/// written. `show_all` (`--all`) brings them back with the staleness reason,
/// mirroring the contract the LOCAL lease table above already has for `--all`.
// trace:BUG-1764 | ai:claude
pub(crate) fn print_cross_clone_leases(
    project_root: &std::path::Path,
    now: chrono::DateTime<chrono::Utc>,
    show_all: bool,
) -> usize {
    let store_root = project_root.join(".aida-store");
    // Two clones sharing one store inherit the same node id, so discriminate
    // "ours" vs "foreign" by the canonical clone path (the project root).
    let our_clone = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf())
        .display()
        .to_string();
    let foreign: Vec<coordination::Claim> = coordination::list_claims(&store_root)
        .into_iter()
        .filter(|c| c.clone_path.is_empty() || c.clone_path != our_clone)
        .collect();
    // BUG-1764: split before rendering. `partition_claims` is keyed on `host`,
    // not `clone_path`, so a foreign-CLONE claim on THIS host is evaluated
    // against the local process table rather than excused as unreachable.
    let (live, stale) = coordination::partition_claims(foreign, now, &coordination::hostname());
    let shown: Vec<(&coordination::Claim, Option<&str>)> = if show_all {
        live.iter()
            .map(|c| (c, None))
            .chain(stale.iter().map(|(c, r)| (c, Some(r.as_str()))))
            .collect()
    } else {
        live.iter().map(|c| (c, None)).collect()
    };
    if shown.is_empty() {
        // Nothing live, but say so rather than leaving the operator to wonder
        // why a lease they can see on disk never appears.
        if !stale.is_empty() {
            println!();
            println!(
                "{} {} stale cross-clone lease{} hidden · {} to list, {} to release",
                "·".dimmed(),
                stale.len(),
                if stale.len() == 1 { "" } else { "s" },
                "aida session leases --all".cyan(),
                "aida session leases --prune-stale".cyan()
            );
        }
        return 0;
    }
    println!();
    println!("{}", "Cross-clone leases (other clones)".bold());
    println!();
    if show_all {
        println!(
            "{:<20} {:<10} {:<14} {:<14} {:<10} state",
            "scope", "host", "node", "agent", "age"
        );
    } else {
        println!(
            "{:<20} {:<10} {:<14} {:<14} age",
            "scope", "host", "node", "agent"
        );
    }
    println!("{}", "─".repeat(if show_all { 86 } else { 72 }));
    for (c, stale_reason) in &shown {
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
        if show_all {
            let state = match stale_reason {
                Some(reason) => format!("{} {}", "⚠ stale".yellow(), reason.dimmed()),
                None => format!("{}", "● live".green()),
            };
            println!(
                "{:<20} {:<10} {:<14} {:<14} {:<10} {}",
                truncate(&c.scope, 20),
                truncate(&c.host, 10),
                truncate(&c.node_id, 14),
                truncate(if c.agent.is_empty() { "-" } else { &c.agent }, 14),
                age,
                state
            );
        } else {
            println!(
                "{:<20} {:<10} {:<14} {:<14} {}",
                truncate(&c.scope, 20),
                truncate(&c.host, 10),
                truncate(&c.node_id, 14),
                truncate(if c.agent.is_empty() { "-" } else { &c.agent }, 14),
                age
            );
        }
        println!("{}{}", " ".repeat(2), c.clone_path.dimmed());
    }
    if !show_all && !stale.is_empty() {
        println!(
            "{} {} stale cross-clone lease{} hidden · {} to list, {} to release",
            "·".dimmed(),
            stale.len(),
            if stale.len() == 1 { "" } else { "s" },
            "aida session leases --all".cyan(),
            "aida session leases --prune-stale".cyan()
        );
    }
    shown.len()
}

/// TASK-345: build the `drain` cross-reference for a lease whose scope is the
/// active drain's current member, or `None` when this lease is not the drain's
/// active member (or no drain is live). Lets `aida session leases --json`
/// correlate a lease to the orchestrator drain/run driving it — `run_uuid`
/// (the per-spec orchestration id), the current phase, the drain mode/batch,
/// the launching command, the orchestrator pid, and its start time.
// trace:TASK-345 | ai:claude
pub(crate) fn lease_drain_xref(
    scope: &str,
    drain: Option<&drain_state::DrainState>,
) -> Option<serde_json::Value> {
    let d = drain?;
    // Only the drain's *current* member is an orchestrator phase child.
    if d.current.as_deref() != Some(scope) {
        return None;
    }
    Some(serde_json::json!({
        "run_uuid": (!d.run_uuid.is_empty()).then(|| d.run_uuid.clone()),
        "member": d.current.clone(),
        "phase": d.current_phase.clone(),
        "mode": d.mode.clone(),
        "batch": d.batch.clone(),
        "command": d.command.clone(),
        "orchestrator_pid": d.orchestrator_pid,
        "started_at": d.started_at.clone(),
    }))
}

/// TASK-345: assemble the `aida session leases --json` rows. Pure over the
/// classified leases + optional live drain snapshot so the JSON shape is
/// unit-testable. Each row mirrors the human columns (id/scope/branch/role/
/// worktree/state/pid) and adds the `drain` cross-reference (null for leases
/// the orchestrator isn't currently driving).
// trace:TASK-345 | ai:claude
pub(crate) fn leases_json_rows(
    shown: &[(SessionLease, LeaseState, Option<u32>)],
    drain: Option<&drain_state::DrainState>,
) -> Vec<serde_json::Value> {
    shown
        .iter()
        .map(|(l, state, pid)| {
            serde_json::json!({
                "id": l.id,
                "scope": l.scope,
                "branch": l.branch,
                "role": l.role,
                "worktree": l.worktree_path.display().to_string(),
                "state": state.label(),
                "pid": pid,
                "drain": lease_drain_xref(&l.scope, drain),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/task345_leases_json_tests.rs"]
mod task345_leases_json_tests;

#[cfg(test)]
#[path = "tests/bug_1772_harness_worktree_lease_tests.rs"]
mod bug_1772_harness_worktree_lease_tests;

/// BUG-1764 clause 4: `aida session leases --prune-stale` — release every
/// CROSS-CLONE claim the staleness predicate reports dead.
///
/// Before this there was no supported way to clear one. `aida session end
/// --spec BUG-1689` answers `No lease found for spec ...` because a foreign
/// claim has no local lease to resolve; `aida session reap` requires spec
/// Done/Completed + branch merged + process exited; and
/// `coordination::release_claim` refuses a claim whose `clone_path` is not ours
/// by design. The only remedy was deleting
/// `.aida-store/coordination/leases/*.toml` by hand.
///
/// Opt-in and never automatic: releasing another clone's claim is a cross-clone
/// mutation. It can only ever touch a claim the SHARED predicate reports stale,
/// so there is no `--force` to add.
// trace:BUG-1764 | ai:claude
pub(crate) fn session_leases_prune_stale(yes: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let store_root = project_root.join(".aida-store");
    let our_clone = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf())
        .display()
        .to_string();
    let foreign: Vec<coordination::Claim> = coordination::list_claims(&store_root)
        .into_iter()
        .filter(|c| c.clone_path.is_empty() || c.clone_path != our_clone)
        .collect();
    let (_live, stale) =
        coordination::partition_claims(foreign, chrono::Utc::now(), &coordination::hostname());

    // BUG-1772 AC2: `aida session leases --prune-stale` also learns to reap
    // immortal pid-less harness-worktree leases.
    // trace:BUG-1772 | ai:antigravity
    let local_leases = list_leases(&project_root);
    let mut stale_local_harness_leases = vec![];
    let harness_threshold = chrono::Utc::now() - chrono::Duration::hours(24);
    for lease in local_leases {
        if lease.scope == worktree_lease::HARNESS_WORKTREE_SCOPE
            && lease.active_pid.is_none()
            && lease.creator_pid.is_none()
            && lease.started_at < harness_threshold
        {
            stale_local_harness_leases.push(lease);
        }
    }

    if stale.is_empty() && stale_local_harness_leases.is_empty() {
        println!("No stale leases to release.");
        return Ok(());
    }
    if !stale.is_empty() {
        println!(
            "{} stale cross-clone lease{} to release:",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" }
        );
        for (c, reason) in &stale {
            println!(
                "  {} {} {}",
                truncate(&c.scope, 20).yellow(),
                format!("({})", c.host).dimmed(),
                reason.dimmed()
            );
        }
    }
    if !stale_local_harness_leases.is_empty() {
        println!(
            "{} stale pid-less local harness-worktree lease{} to release:",
            stale_local_harness_leases.len(),
            if stale_local_harness_leases.len() == 1 {
                ""
            } else {
                "s"
            }
        );
        for l in &stale_local_harness_leases {
            println!("  {} {}", l.id.yellow(), "age > 24h".dimmed());
        }
    }

    // Same posture as `aida session end`: a non-terminal stdin must not be
    // able to release a peer's claims without saying so explicitly.
    if !yes {
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            anyhow::bail!(
                "refusing to prune non-interactively without --yes (releasing another clone's \
                 claim is a cross-clone mutation)"
            );
        }
        let total = stale.len() + stale_local_harness_leases.len();
        if !confirm(&format!("Release {} stale lease(s)? [y/N] ", total)) {
            println!("Aborted.");
            return Ok(());
        }
    }
    let mut released = 0usize;
    for (c, reason) in &stale {
        match coordination::release_stale_foreign_claim(
            &store_root,
            &c.scope,
            &c.clone_path,
            &c.heartbeat_at,
        ) {
            Ok(true) => {
                released += 1;
                println!(
                    "  {} released cross-clone {} — {}",
                    "✓".green(),
                    c.scope,
                    reason
                );
            }
            // Refreshed, reclaimed, or already gone between the listing and the
            // delete — the verdict no longer describes the record on disk.
            Ok(false) => println!(
                "  {} skipped cross-clone {} — changed on the store since it was listed",
                "·".dimmed(),
                c.scope
            ),
            Err(e) => eprintln!(
                "  {} could not release cross-clone {}: {e}",
                "Warning:".yellow().bold(),
                c.scope
            ),
        }
    }
    let mut local_released = 0usize;
    for l in &stale_local_harness_leases {
        let p = lease_path(&project_root, &l.id);
        if p.exists() {
            if let Err(e) = std::fs::remove_file(&p) {
                eprintln!(
                    "  {} could not remove {}: {e}",
                    "Warning:".yellow().bold(),
                    l.id
                );
            } else {
                local_released += 1;
                println!("  {} released local harness lease {}", "✓".green(), l.id);
            }
        }
    }

    println!();
    println!(
        "Released {} of {} stale cross-clone lease claim(s), and {} of {} stale local harness lease(s).",
        released,
        stale.len(),
        local_released,
        stale_local_harness_leases.len()
    );
    Ok(())
}
pub(crate) fn session_leases(verbose: bool, all: bool, json: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let leases = list_leases(&project_root);
    if leases.is_empty() {
        // TASK-345: JSON consumers get an empty array, not the human hints.
        // trace:TASK-345 | ai:claude
        if json {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!([]))?);
            return Ok(());
        }
        // STORY-637: even with no LOCAL leases, a peer clone may hold a
        // cross-clone lease — surface it before the "no sessions" hint so a
        // refused `session start` has a place to point.
        let foreign = print_cross_clone_leases(&project_root, chrono::Utc::now(), all);
        if foreign == 0 {
            println!("(no active sessions)");
        }
        println!();
        println!(
            "Start one with: {} {}",
            "aida session start --owns".cyan(),
            "<scope>".dimmed()
        );
        // BUG-98: explicit pointer at the historical-conversations view so
        // users who landed here looking for `list` know where it lives.
        eprintln!();
        eprintln!(
            "{}",
            "(for the historical list of Claude Code conversations in this project, run `aida session conversations`)"
                .dimmed()
        );
        return Ok(());
    }

    // TASK-55: probe live claudes once + classify every lease so we
    // can render the state column AND apply the hide-stale filter
    // off the same data. trace:TASK-55 | ai:claude
    let now = chrono::Utc::now();
    let live = process_probe::probe_live_claude_sessions();
    let classified: Vec<(SessionLease, LeaseState, Option<u32>)> = leases
        .iter()
        .map(|l| {
            let live_in_worktree = live.iter().find(|s| {
                !s.stale_cwd && (s.cwd == l.worktree_path || s.cwd.starts_with(&l.worktree_path))
            });
            // BUG-511: review-verb leases classify by creator PID, not worktree.
            let state = lease_state_for(l, &live, now);
            let pid = l
                .active_pid
                .filter(|p| process_probe::pid_is_alive(*p))
                .or_else(|| live_in_worktree.map(|s| s.pid))
                .or(if l.review_verb || l.claim_verb {
                    l.creator_pid
                } else {
                    None
                });
            (l.clone(), state, pid)
        })
        .collect();

    let (shown, hidden_stale): (Vec<_>, Vec<_>) = if all {
        (classified, Vec::new())
    } else {
        classified
            .into_iter()
            .partition(|(_, state, _)| !matches!(state, LeaseState::Stale))
    };

    // STORY-301: cross-reference the active `--auto-complete` drain. A lease
    // whose scope is the drain's current member is an orchestrator phase
    // child — annotate it so `aida session leases` shows which session the
    // orchestrator is driving, and at which phase. trace:STORY-301 | ai:claude
    // TASK-345: retain the full live DrainState so the `--json` path can emit
    // the drain cross-reference (run uuid / mode / orchestrator pid) per lease,
    // not just the phase string the human view needs. trace:TASK-345 | ai:claude
    let drain_active: Option<drain_state::DrainState> = match find_main_worktree_root()
        .map(|r| drain_state::probe(&r))
        .unwrap_or(drain_state::DrainStatus::None)
    {
        drain_state::DrainStatus::Active(s) => Some(s),
        _ => None,
    };
    let drain_current: Option<(String, Option<String>)> = drain_active
        .as_ref()
        .and_then(|s| s.current.clone().map(|c| (c, s.current_phase.clone())));

    // TASK-345: machine-readable lease list. Each row carries a `drain`
    // cross-reference so a consumer can correlate a lease to the orchestrator
    // drain/run driving it. Emitted before any human rendering.
    // trace:TASK-345 | ai:claude
    if json {
        let rows = leases_json_rows(&shown, drain_active.as_ref());
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!(rows))?
        );
        return Ok(());
    }

    println!("{}", "Active session leases".bold());
    println!();
    // STORY-65: role column shows the persona inherited at session start.
    // TASK-55: adds a `state` column when --all is set. TASK-56: adds
    // `pid` + `cc-session` columns when --verbose is set.
    // trace:STORY-65, TASK-55, TASK-56 | ai:claude
    let mut header = format!(
        "{:<14} {:<20} {:<18} {:<14}",
        "id", "scope", "branch", "role"
    );
    if all {
        header.push_str(&format!(" {:<10}", "state"));
    }
    if verbose {
        header.push_str(&format!(" {:<10} {:<20}", "pid", "cc-session"));
    }
    header.push_str(" worktree");
    println!("{}", header);
    println!("{}", "─".repeat(header.len().max(80)));
    // BUG-312: HLC-derived UUIDs put the timestamp at the start, so two
    // leases created in the same generation window share the historical
    // 8-char prefix. Disambiguate against the FULL active set (not just
    // `shown`) — `aida session end` resolves over the unfiltered set,
    // so the rendered id has to be unique against everything an end
    // call might match. trace:BUG-312 | ai:claude
    let all_ids: Vec<&str> = leases.iter().map(|l| l.id.as_str()).collect();
    for (l, state, pid) in &shown {
        let prefix_len = unique_prefix_len(&l.id, &all_ids, 8);
        let mut row = format!(
            "{:<14} {:<20} {:<18} {:<14}",
            (&l.id[..prefix_len]).yellow(),
            truncate(&l.scope, 20),
            truncate(&l.branch, 18),
            truncate(l.role.as_deref().unwrap_or("-"), 14),
        );
        if all {
            let label = format!("{} {}", state.glyph(), state.label());
            let colored = match state {
                LeaseState::Live => label.green(),
                LeaseState::Dormant => label.cyan(),
                LeaseState::Stale => label.yellow(),
            };
            row.push_str(&format!(" {:<10}", colored));
        }
        if verbose {
            let pid_col = pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
            let cc = cc_session_id_for_worktree(&l.worktree_path)
                .map(|s| s.chars().take(20).collect::<String>())
                .unwrap_or_else(|| "-".into());
            row.push_str(&format!(" {:<10} {:<20}", pid_col, cc));
        }
        row.push_str(&format!(" {}", l.worktree_path.display()));
        println!("{}", row);
        // STORY-301: annotate the lease the orchestrator is currently driving.
        if let Some((cur, phase)) = &drain_current {
            if &l.scope == cur {
                let phase_txt = phase.as_deref().unwrap_or("starting");
                println!(
                    "{}",
                    format!("{}◆ orchestrator drain — phase {phase_txt}", " ".repeat(15)).cyan()
                );
            }
        }
    }
    if !hidden_stale.is_empty() {
        println!();
        println!(
            "{}",
            format!(
                "({} stale lease{} hidden — pass --all to show)",
                hidden_stale.len(),
                if hidden_stale.len() == 1 { "" } else { "s" }
            )
            .dimmed()
        );
    }

    // STORY-69: --verbose warns about leaked claudes — processes whose
    // cwd is `(deleted)` (worktree was removed without ending claude
    // first, the BUG-61 signature). Note: live-in-lease claude PIDs
    // now appear inline in the `pid` column for each lease row
    // (TASK-56), so the previous "Live claude processes in lease
    // worktrees" sub-section was redundant and was removed.
    // trace:STORY-69 | ai:claude
    if verbose {
        let leaked: Vec<_> = live.iter().filter(|s| s.stale_cwd).collect();
        if !leaked.is_empty() {
            println!();
            println!("{}", "Leaked claude processes:".bold());
            for s in &leaked {
                println!(
                    "  {} pid {} cwd {} {} — `kill {}` to clean up",
                    "WARN".yellow().bold(),
                    s.pid.to_string().yellow(),
                    s.cwd.display(),
                    "(deleted)".dimmed(),
                    s.pid
                );
            }
        }
    }

    // STORY-637: surface cross-clone lease claims held by OTHER clones —
    // distinct from the local leases above. trace:STORY-637 | ai:claude
    print_cross_clone_leases(&project_root, now, all);

    println!();
    println!(
        "End one with: {} {}",
        "aida session end".cyan(),
        "<id>".dimmed()
    );
    Ok(())
}

/// One `.jsonl` file old enough to prune. Captured up-front so the
/// confirmation list and the deletion loop see the same set.
/// trace:STORY-60 | ai:claude
pub(crate) struct PruneCandidate {
    pub(crate) path: std::path::PathBuf,
    pub(crate) size: u64,
    pub(crate) age_seconds: u64,
}

/// Format a byte count with a human-readable unit (KB/MB/GB). One
/// decimal place so a 1.5 MB file doesn't read as "1 MB" or "1500000 B".
/// trace:STORY-60 | ai:claude
pub(crate) fn humanize_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{} B", bytes)
    } else if b < MB {
        format!("{:.1} KB", b / KB)
    } else if b < GB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.1} GB", b / GB)
    }
}

/// Compute the new queue position for `aida queue move X --after Y`.
/// `anchor` is Y's current position; `successor` is the position of the
/// next entry strictly after Y, or `None` when Y is at the bottom.
///
/// Returns a position that sorts strictly between `anchor` and the next
/// entry. Midpoint math when there's room; +1 fallback when adjacent
/// (collision risk acknowledged — safe in practice because positions are
/// initially gapped by 1000); +1000 when Y is at the bottom. All math is
/// saturating so a corrupt-state queue (every item at `i64::MAX` from a
/// pre-fix queue_add, see git_backend.rs) doesn't panic on overflow —
/// the callsite is expected to surface a friendlier hint instead.
/// trace:STORY-72 | ai:claude
pub(crate) fn position_after(anchor: i64, successor: Option<i64>) -> i64 {
    match successor {
        Some(np) if np > anchor.saturating_add(1) => {
            anchor.saturating_add((np.saturating_sub(anchor)) / 2)
        }
        Some(_) => anchor.saturating_add(1),
        None => anchor.saturating_add(1000),
    }
}

/// Compute the queue order after `aida queue move X --to N` — moving
/// `moved_id` to the 1-indexed absolute slot `requested`.
///
/// `entries` is the queue in display order (position-sorted) and must
/// include `moved_id`. The moved id is dropped, then re-inserted at the
/// clamped slot, so `--to 3` lands the item at the third slot of the
/// *final* queue regardless of where it started. `requested` is clamped
/// to `1..=entries.len()`: an out-of-range N moves the item to the
/// nearest end rather than erroring (the callsite surfaces a hint). The
/// caller renumbers the returned id list with the standard 1000-gap.
/// trace:TASK-280 | ai:claude
pub(crate) fn move_to_absolute_position(
    entries: &[uuid::Uuid],
    moved_id: uuid::Uuid,
    requested: usize,
) -> (Vec<uuid::Uuid>, usize) {
    let queue_len = entries.len();
    let mut order: Vec<uuid::Uuid> = entries.iter().copied().filter(|&e| e != moved_id).collect();
    let slot = requested.clamp(1, queue_len.max(1));
    let insert_idx = (slot - 1).min(order.len());
    order.insert(insert_idx, moved_id);
    (order, slot)
}

pub(crate) fn humanize_age_secs(secs: u64) -> String {
    if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else if secs < 7 * 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs < 30 * 86_400 {
        format!("{}w", secs / (7 * 86_400))
    } else if secs < 365 * 86_400 {
        format!("{}mo", secs / (30 * 86_400))
    } else {
        format!("{}y", secs / (365 * 86_400))
    }
}

/// `aida session prune` — walk Claude Code's per-project session
/// directories under `~/.claude/projects/<encoded>/` and delete `.jsonl`
/// files whose mtime is older than `days`. Skips any project dir
/// corresponding to an active session lease so a long-running session
/// doesn't self-delete from a forgotten cron-like usage. Logs each
/// deletion to `<project>/.aida/session-prune.log` for auditing.
/// trace:STORY-60 | ai:claude
/// Walk `~/.claude/projects/*` and find project dirs whose recorded cwd
/// no longer exists on disk. Each surviving jsonl encodes its cwd in
/// every event; we read one jsonl per project dir, extract the cwd
/// from the first event that has one, and check the filesystem. No
/// jsonls in the dir → treat as orphan (the dir is content-less).
/// Falls back to the lossy dir-name decode (replace `-` with `/`) when
/// the jsonl lacks a `"cwd":` field. trace:TASK-70 | ai:claude
/// TASK-358: mechanically tear down a lease + its worktree without the
/// interactive ceremony of [`session_end`] — no CI probe, no live-claude
/// refusal, no dirty-tree refusal, no prompts. Only callers that already
/// know the lease is safe to remove (e.g. the orchestrator's
/// `--escalate-blocks` marker [`SessionLease::escalated_to_human`]) should
/// reach this. Mirrors `session_end`'s interior writes — strip runtime
/// symlinks → unlink lease/manifest/activity log → `git worktree remove
/// --force` — and warns rather than errors on partial failure so a sweep
/// that hits one stuck worktree still tries the rest. Returns `true` when
/// the worktree was removed cleanly.
/// trace:TASK-358 | ai:claude
pub(crate) fn force_cleanup_lease(project_root: &std::path::Path, lease: &SessionLease) -> bool {
    // BUG-511: advisory leases (review-verb / MCP claims that recorded no
    // worktree) have nothing on disk beyond the lease file itself — and an
    // empty `worktree_path` would make the symlink-strip and `git worktree
    // remove` legs below resolve relative to CWD. Remove the lease file
    // (after the same activity aggregation) and stop.
    if lease.worktree_path.as_os_str().is_empty() {
        aggregate_session_activity_into_roles(project_root, &lease.id);
        let _ = std::fs::remove_file(lease_path(project_root, &lease.id));
        let activity = session_activity_path(project_root, &lease.id);
        if activity.exists() {
            let _ = std::fs::remove_file(&activity);
        }
        return true;
    }

    // Snapshot the lease's authoritative on-disk path before we strip the
    // worktree's symlinks (which can break the symlink chain). Mirrors
    // BUG-56's pattern in `session_end`.
    let lease_file_via_symlink = lease_path(project_root, &lease.id);
    let canonical_lease = lease_file_via_symlink.canonicalize().ok();

    // Aggregate the session's activity log into project-level role activity
    // before we delete the log — same ordering as `session_end`.
    aggregate_session_activity_into_roles(project_root, &lease.id);
    let activity_file = session_activity_path(project_root, &lease.id);
    let canonical_activity = activity_file.canonicalize().ok();
    let activity_target: std::path::PathBuf = canonical_activity
        .clone()
        .unwrap_or_else(|| activity_file.clone());

    // Strip the runtime symlinks `session_start` installed so `git
    // worktree remove` doesn't refuse on untracked files (BUG-52).
    let store_link = lease.worktree_path.join(".aida-store");
    if store_link.is_symlink() {
        let _ = std::fs::remove_file(&store_link);
    }
    let aida_dir = lease.worktree_path.join(".aida");
    for runtime in &[
        "sessions",
        "roles",
        "cache.db",
        "cache.db-shm",
        "cache.db-wal",
        "pgdata",
    ] {
        let p = aida_dir.join(runtime);
        if p.is_symlink() {
            let _ = std::fs::remove_file(&p);
        }
    }

    // Lease file first — once gone the session no longer counts as active.
    let lease_target: &std::path::Path = canonical_lease
        .as_deref()
        .unwrap_or(&lease_file_via_symlink);
    match std::fs::remove_file(lease_target) {
        Ok(_) | Err(_) => {}
    }

    // Companion files — quiet on missing.
    let manifest_target = session_manifest::manifest_path(project_root, &lease.id);
    if manifest_target.exists() {
        let _ = std::fs::remove_file(&manifest_target);
    }
    if activity_target.exists() {
        let _ = std::fs::remove_file(&activity_target);
    }

    // `git worktree remove --force` — `--force` because the dirty-tree
    // refusal in `session_end` is gated by interactive prompts; an
    // escalation cleanup never had a chance to commit, so we just remove.
    let res = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "worktree",
            "remove",
            "--force",
            "--", // trace:BUG-1622 | ai:claude
            lease.worktree_path.to_str().unwrap_or_default(),
        ])
        .output();
    let removed = matches!(&res, Ok(o) if o.status.success());
    if !removed {
        eprintln!(
            "  {} could not remove worktree {} — remove manually with \
             `git worktree remove --force {}`",
            "Warning:".yellow().bold(),
            lease.worktree_path.display(),
            lease.worktree_path.display(),
        );
    }
    removed
}

/// TASK-358: on a triage that takes a spec out of `NeedsAttention`, find any
/// orchestrator-escalated lease for it and clean up the lingering worktree.
/// The lease's `escalated_to_human` marker is the load-bearing gate: only
/// leases set by `--escalate-blocks` are touched, so an interactive user
/// session on the same spec stays put, and the advisor-resume path
/// (which never sets the marker) preserves its worktree as STORY-306
/// requires. Best-effort: a missing project root or zero matching leases
/// is a quiet no-op. trace:TASK-358 | ai:claude
pub(crate) fn cleanup_escalated_leases_for_spec(project_root: &std::path::Path, spec_id: &str) {
    let leases = list_leases(project_root);
    let mut cleaned = 0usize;
    for lease in leases {
        if lease.escalated_to_human.is_none() {
            continue;
        }
        if !lease.scope.eq_ignore_ascii_case(spec_id) {
            continue;
        }
        eprintln!(
            "  {} cleaning up escalated worktree for {}: {}",
            "→".dimmed(),
            spec_id.cyan(),
            lease.worktree_path.display().to_string().dimmed(),
        );
        force_cleanup_lease(project_root, &lease);
        cleaned += 1;
    }
    if cleaned > 0 {
        eprintln!(
            "  {} {} escalated worktree{} cleaned up",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            cleaned,
            if cleaned == 1 { "" } else { "s" },
        );
    }
}

/// TASK-1311: the per-row requeue affordance under a NeedsAttention spec in
/// `aida findings list`. It shows the one command that returns the spec to
/// flight and the status it lands in, and puts an open decision first.
/// STORY-1429: rendered from the same `requeue_preview` the interactive
/// `aida rework` loop shows, so the hint and the keystroke cannot disagree.
// trace:TASK-1311 trace:STORY-1429 | ai:claude
pub(crate) fn print_requeue_hint_row(
    r: &aida_core::Requirement,
    did: &str,
    store: Option<&aida_core::RequirementsStore>,
    storage: &Storage,
) {
    let route = queue_cmd::current_queue_route(storage, r.id);
    let preview = requeue::requeue_preview(
        r,
        store,
        route.as_deref(),
        requeue::caller_may_clear_escalation(),
    );
    println!(
        "  {:<20} {:<14} {}",
        "",
        "",
        format!(
            "{} {}",
            crate::glyph(crate::glyphs::Glyph::SubArrow),
            requeue::requeue_hint(did, &preview)
        )
        .dimmed()
    );
}

/// EPIC-28: park a spec in `NeedsAttention` with a structured
/// [`aida_core::FailureReason`] so a batch drain can continue past a phase
/// failure rather than halting. Called from
/// `RealPhaseDriver::shelve_on_failure` (which the orchestrator's
/// `finish_failure` invokes for every shelvable phase failure).
///
/// Returns the populated `FailureReason` on success (the orchestrator
/// stamps it on `OrchestrationResult::shelved_reason`); returns `Ok(None)`
/// when the spec cannot be safely shelved (e.g. it is already in a
/// terminal status). Best-effort: ledger-write failures are logged but
/// never undo the status flip, mirroring `aida punt`.
/// trace:EPIC-28 | ai:claude
pub(crate) fn shelve_spec_on_failure(
    project_root: &std::path::Path,
    spec: &str,
    phase_slug: &str,
    phase_index: u8,
    kind_slug: &str,
    detail: &str,
    recovery_hint: &str,
) -> anyhow::Result<Option<aida_core::FailureReason>> {
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        // No distributed store at this root → nothing to update. The
        // orchestrator only runs in git-canonical projects today; this is
        // defensive for the legacy centralized-SQLite layout where
        // shelving is not supported.
        return Ok(None);
    };
    let dispenser = load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    // trace:TASK-1468 | ai:claude
    let Some(mut req) = backend.get_requirement_unambiguous(spec)? else {
        // Spec not in the store — orchestrator was driving a stale id.
        // Best-effort: log and skip rather than crash the failure path.
        eprintln!(
            "  {} could not load spec {} for shelving — store has no matching id",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            spec,
        );
        return Ok(None);
    };
    let display_id = req.display_id();

    let now = chrono::Utc::now();
    let fr = aida_core::FailureReason {
        phase: phase_slug.to_string(),
        phase_index,
        kind: kind_slug.to_string(),
        detail: detail.to_string(),
        recovery_hint: if recovery_hint.is_empty() {
            None
        } else {
            Some(recovery_hint.to_string())
        },
        shelved_by: std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|r| !r.is_empty()),
        shelved_at: now,
    };

    // Honor the STORY-332 transition guard. The common case is
    // `InProgress → NeedsAttention` (orchestrator drove the spec through
    // phase 1 pickup, which set InProgress). Edge cases where the spec
    // never reached InProgress (a phase-0 setup failure) still record the
    // FailureReason so the operator sees it, but skip the status flip so
    // we don't fight the guard.
    shelve_status_flip(&mut req);
    req.failure_reason = Some(fr.clone());
    req.modified_at = now;
    backend.update_requirement(&req)?;

    // Best-effort ledger append + role activity record. A failure here
    // must not undo the spec update, which is the load-bearing part.
    if let Err(e) = punt::append_failure_to_ledger(project_root, &display_id, &fr) {
        eprintln!(
            "  {} could not write shelving record to .aida/punts.jsonl: {e}",
            "Note:".dimmed()
        );
    }
    record_role_activity(&display_id, "shelve");

    Ok(Some(fr))
}

/// The shelve's status flip: `NeedsAttention` when the STORY-332 transition
/// guard allows it (from In Progress or Done), otherwise no change, so a
/// terminal spec is never moved. Returns whether the status changed.
///
/// BUG-1632: the shelve is an automated write, so the flip is recorded in the
/// status history under [`aida_core::conflict::ORCHESTRATOR_SHELVE_AUTHOR`].
/// The BUG-1625 merge guard then keeps a terminal status another clone reached
/// meanwhile instead of letting this newer NeedsAttention win.
// trace:EPIC-28 trace:BUG-1632 | ai:claude
pub(crate) fn shelve_status_flip(req: &mut aida_core::Requirement) -> bool {
    if aida_core::forbidden_attention_transition(
        &req.status,
        &aida_core::RequirementStatus::NeedsAttention,
    )
    .is_some()
    {
        return false;
    }
    let from = req.status.clone();
    req.status = aida_core::RequirementStatus::NeedsAttention;
    aida_core::conflict::record_status_transition(
        req,
        aida_core::conflict::ORCHESTRATOR_SHELVE_AUTHOR,
        &from,
    );
    req.status != from
}

/// BUG-1637: the PR-open Done assertion's status write. Automated, and only
/// ever In Progress -> Done, so it never leaves a terminal status. Recorded
/// under [`aida_core::conflict::PR_OPEN_AUTHOR`]. Returns whether it moved.
// trace:BUG-1637 | ai:claude
pub(crate) fn pr_open_done_flip(req: &mut Requirement) -> bool {
    if !matches!(req.status, RequirementStatus::InProgress) {
        return false;
    }
    aida_core::conflict::set_status_recorded(
        req,
        RequirementStatus::Done,
        aida_core::conflict::PR_OPEN_AUTHOR,
    );
    true
}

/// BUG-1637: the Approved -> In Progress coherence bump shared by the
/// SubagentStart lease-take hook ([`aida_core::conflict::LEASE_TAKE_AUTHOR`])
/// and `aida session start` ([`aida_core::conflict::SESSION_START_AUTHOR`]).
/// Automated; any other source status (terminal included) is left alone.
/// Returns whether it moved.
// trace:BUG-1637 | ai:claude
pub(crate) fn approved_to_in_progress_bump(req: &mut Requirement, author: &str) -> bool {
    if !matches!(req.status, RequirementStatus::Approved) {
        return false;
    }
    aida_core::conflict::set_status_recorded(req, RequirementStatus::InProgress, author);
    req.modified_at = chrono::Utc::now();
    true
}

/// BUG-1637: the zen autopilot's Draft -> Approved write. It applies only to
/// a Draft (the status the approve-gate judged and the autopilot audit records
/// as the prior state), so it never leaves a terminal status. Recorded under
/// [`aida_core::conflict::ZEN_APPROVE_AUTHOR`]. Returns whether it moved.
// trace:BUG-1637 | ai:claude
pub(crate) fn zen_approve_flip(req: &mut Requirement) -> bool {
    if !matches!(req.status, RequirementStatus::Draft) {
        return false;
    }
    aida_core::conflict::set_status_recorded(
        req,
        RequirementStatus::Approved,
        aida_core::conflict::ZEN_APPROVE_AUTHOR,
    );
    req.modified_at = chrono::Utc::now();
    true
}

/// BUG-1637: the orchestrator's phase-1 pre-spawn bump (BUG-369). Re-checks
/// the source status on the copy read inside the atomic write, so a spec a
/// concurrent writer moved to Rejected or Completed is not bumped, and records
/// the move under [`aida_core::conflict::ORCHESTRATOR_PHASE1_AUTHOR`].
/// Returns whether it moved.
// trace:BUG-369 trace:BUG-1637 | ai:claude
pub(crate) fn phase1_status_bump(
    r: &mut Requirement,
    target: &RequirementStatus,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    if auto_complete_phase1_target_status(&r.status).as_ref() != Some(target) {
        return false;
    }
    aida_core::conflict::set_status_recorded(
        r,
        target.clone(),
        aida_core::conflict::ORCHESTRATOR_PHASE1_AUTHOR,
    );
    r.modified_at = now;
    true
}

/// BUG-1637: TASK-133's restore of the pre-bump status after a phase-1
/// failure. Automated: it never leaves a terminal status (a spec that reached
/// one meanwhile is no longer the spec phase 1 bumped) and records the move
/// under [`aida_core::conflict::ORCHESTRATOR_PHASE1_AUTHOR`]. Returns whether
/// it applied.
// trace:TASK-133 trace:BUG-1637 | ai:claude
pub(crate) fn phase1_status_restore(req: &mut Requirement, prior: &RequirementStatus) -> bool {
    if aida_core::conflict::is_terminal_status(&req.status) {
        return false;
    }
    aida_core::conflict::set_status_recorded(
        req,
        prior.clone(),
        aida_core::conflict::ORCHESTRATOR_PHASE1_AUTHOR,
    );
    true
}

/// BUG-1637: the whole TASK-133 restore write, run inside the atomic write:
/// restore the pre-bump status and clear the spurious shelve FailureReason, or
/// change nothing when [`phase1_status_restore`] refuses. Returns whether it
/// applied.
// trace:TASK-133 trace:BUG-1637 | ai:claude
pub(crate) fn apply_phase1_restore(r: &mut Requirement, prior: &RequirementStatus) -> bool {
    if !phase1_status_restore(r, prior) {
        return false;
    }
    // The shelve that ran on the failure path stamped a FailureReason; with no
    // lease and no work behind it that finding is spurious — clear it.
    r.failure_reason = None;
    r.modified_at = chrono::Utc::now();
    true
}

// BUG-1638: race-test seam for the self-reading status writers
// (`ensure_spec_done_after_pr`, `bump_spec_in_progress_at_lease_take`,
// `restore_phase1_status_on_lease_failure`). Each reads the spec, then writes
// it through `update_spec_atomically`; a test arms a one-shot hook that runs
// between the read and the write, so it can make a concurrent change there and
// pin that the write re-checks the copy read under the store lock. Compiled
// out of non-test builds. trace:BUG-1638 | ai:claude
#[cfg(test)]
pub(crate) type StatusWriteRaceHook = Box<dyn FnOnce(&std::path::Path)>;

#[cfg(test)]
thread_local! {
    static STATUS_WRITE_RACE_HOOK: std::cell::RefCell<Option<StatusWriteRaceHook>> =
        const { std::cell::RefCell::new(None) };
}

/// BUG-1638: arm the one-shot hook [`status_write_race_seam`] runs on this
/// thread. The hook receives the store root the writer resolved.
// trace:BUG-1638 | ai:claude
#[cfg(test)]
pub(crate) fn inject_status_write_race(hook: impl FnOnce(&std::path::Path) + 'static) {
    STATUS_WRITE_RACE_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
}

/// BUG-1638: called by each self-reading status writer between its read and
/// its atomic write. A no-op outside tests.
// trace:BUG-1638 | ai:claude
pub(crate) fn status_write_race_seam(store_root: &std::path::Path) {
    #[cfg(test)]
    {
        let hook = STATUS_WRITE_RACE_HOOK.with(|h| h.borrow_mut().take());
        if let Some(hook) = hook {
            hook(store_root);
        }
    }
    #[cfg(not(test))]
    let _ = store_root;
}

/// BUG-1638: what [`restore_phase1_status_on_lease_failure`] did, so the
/// orchestrator reports a restore only when one happened.
// trace:BUG-1638 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Phase1RestoreOutcome {
    /// The pre-bump status was written back.
    Restored,
    /// Nothing was written; the reason says why.
    Refused(String),
}

/// BUG-1638: the orchestrator's line for a phase-1 restore outcome. It says
/// "status restored" only for [`Phase1RestoreOutcome::Restored`]. BUG-1647:
/// statuses read as the CLI shows them ("Needs Attention"), the same form the
/// refusal reason uses.
// trace:BUG-1638 trace:BUG-1647 | ai:claude
pub(crate) fn phase1_restore_outcome_message(
    outcome: &Phase1RestoreOutcome,
    display_id: &str,
    prior: &RequirementStatus,
) -> String {
    match outcome {
        Phase1RestoreOutcome::Restored => format!(
            "phase-1 startup failed before acquiring a lease — status restored to {prior} \
             (re-queueable); no work was stranded"
        ),
        Phase1RestoreOutcome::Refused(reason) => format!(
            "phase-1 startup failed before acquiring a lease — {display_id}'s status was not \
             restored to {prior}: {reason}"
        ),
    }
}

/// BUG-1651: the stderr line the orchestrator prints for a phase-1 restore
/// outcome, or `None` under `--json`: both the restore and the refusal are
/// suppressed so JSON output stays machine-readable (BUG-1647).
// trace:BUG-1647 trace:BUG-1651 | ai:claude
pub(crate) fn phase1_restore_report_line(
    outcome: &Phase1RestoreOutcome,
    display_id: &str,
    prior: &RequirementStatus,
    json: bool,
) -> Option<String> {
    if json {
        return None;
    }
    let glyph = match outcome {
        Phase1RestoreOutcome::Restored => "↩".cyan(),
        Phase1RestoreOutcome::Refused(_) => crate::glyph(crate::glyphs::Glyph::Info).cyan(),
    };
    Some(format!(
        "  {glyph} {}",
        phase1_restore_outcome_message(outcome, display_id, prior)
    ))
}

/// TASK-133: undo the orchestrator parent's pre-spawn phase-1 status bump.
///
/// `prepare_auto_complete_phase1_status` flips a spec Approved/Planned/Draft →
/// InProgress *before* spawning the implementer child (BUG-369). When phase 1
/// then fails without the child ever acquiring a lease, no work happened — but
/// the spec is now InProgress and, because `finish_failure` shelves shelvable
/// phase failures, typically NeedsAttention with a `failure_reason`. That
/// strands a spec the operator should be able to re-queue: there is nothing to
/// triage, only a transient spawn / contention error.
///
/// This restores the captured pre-bump status and clears the (spurious)
/// `failure_reason` so the spec drops out of `aida findings list` and is
/// cleanly pickable again. Best-effort, like the shelve it compensates: a
/// store-write error is logged but never aborts the drain epilogue. Writes
/// through `CachedGitBackend` so the read-projection (`aida list`) reflects
/// the reset immediately rather than waiting for a stale-detection rebuild.
/// trace:TASK-133 | ai:claude
/// BUG-479: does ANY child-side evidence of real work exist for `spec`?
/// Pure decision over the three probed signals — a live/stale lease scoped to
/// the spec, an on-disk worktree for it, or unmerged commits on its branch.
/// Any one being present means the implementer child DID acquire a lease and do
/// work before exiting non-zero (e.g. a post-commit `/aida-pr` failure or a
/// headless session aborting after committing), so the parent's "no lease ⇒ no
/// work" assumption is wrong and the spec must stay shelved for triage rather
/// than be reset. trace:BUG-479 | ai:claude
pub(crate) fn child_side_work_exists(
    has_lease: bool,
    has_worktree: bool,
    has_unmerged_commits: bool,
) -> bool {
    has_lease || has_worktree || has_unmerged_commits
}

/// BUG-479: probe the real world for child-side work on `spec`, returning
/// `(has_lease, has_worktree, has_unmerged_commits)`.
///
/// - `has_lease`: a session lease whose scope matches the spec (case-insensitive
///   — the same scope-matching `aida session start`'s double-claim guard and the
///   implementer-prompt lease lookups use).
/// - `has_worktree`: such a lease records a worktree path that still exists on
///   disk (the worktree the child created when it acquired the lease).
/// - `has_unmerged_commits`: such a lease's branch has commits not yet on the
///   default branch (`git rev-list --count <default>..<branch>` > 0).
///
/// Conservative: a git probe that can't run leaves its signal `false`, but the
/// lease/worktree signals already cover the dominant case, and the caller treats
/// ANY of the three as "leave it shelved". trace:BUG-479 | ai:claude
pub(crate) fn probe_child_side_work_for_spec(
    project_root: &std::path::Path,
    spec: &str,
) -> (bool, bool, bool) {
    let matching: Vec<SessionLease> = list_leases(project_root)
        .into_iter()
        .filter(|l| l.scope.eq_ignore_ascii_case(spec))
        .collect();
    let has_lease = !matching.is_empty();
    let has_worktree = matching.iter().any(|l| l.worktree_path.exists());

    let has_unmerged_commits = match detect_default_branch_ref(project_root) {
        Some(default_ref) => matching
            .iter()
            .any(|l| branch_has_unmerged_commits(project_root, &default_ref, &l.branch)),
        None => false,
    };

    (has_lease, has_worktree, has_unmerged_commits)
}

/// BUG-479 (extracted for BUG-1716): does `branch` carry commits not yet on
/// `default_ref` (`git rev-list --count <default>..<branch>` > 0)?
/// Conservative: an empty/default branch name or a git probe that can't run
/// reads as `false`.
// trace:BUG-479 trace:BUG-1716 | ai:claude
pub(crate) fn branch_has_unmerged_commits(
    project_root: &std::path::Path,
    default_ref: &str,
    branch: &str,
) -> bool {
    if branch.is_empty() || branch == default_ref {
        return false;
    }
    let range = format!("{default_ref}..{branch}");
    std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-list", "--count", &range])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u64>()
                .ok()
        })
        .map(|n| n > 0)
        .unwrap_or(false)
}

/// BUG-1716: the session-lease evidence feeding the `Missing`-log arm of
/// [`empty_launch_decision`]. `None` — no lease exists for THIS session (the
/// child never claimed one), which routes the failure to the BUG-1629/
/// BUG-1769 lost-child recovery, not the empty-launch lane. `Some(has
/// commits)` — the child claimed its lease (the TASK-204 shape), and the
/// bool says whether its branch carries unmerged commits (real work the
/// empty-launch release must never orphan). Keyed to THIS session's lease
/// via the minted `--session-id`, never a scope-wide sweep — a sibling
/// attempt's PR branch must not veto classifying this launch.
// trace:BUG-1716 | ai:claude
pub(crate) fn orchestrated_session_lease_evidence(
    project_root: &std::path::Path,
    claude_session_id: &str,
) -> Option<bool> {
    let (_, branch, _, _) = find_orchestrated_lease(project_root, claude_session_id)?;
    let has_commits = detect_default_branch_ref(project_root)
        .map(|default_ref| branch_has_unmerged_commits(project_root, &default_ref, &branch))
        .unwrap_or(false);
    Some(has_commits)
}

pub(crate) fn restore_phase1_status_on_lease_failure(
    project_root: &std::path::Path,
    spec: &str,
    prior: &aida_core::RequirementStatus,
) -> anyhow::Result<Phase1RestoreOutcome> {
    // BUG-479: before resetting, probe child-side reality. The caller gates on
    // the PARENT-side `implementer_lease` field, which is only set after a CLEAN
    // child exit — so a child that acquired a lease + worktree + committed and
    // THEN exited non-zero (post-commit `/aida-pr` failure, aborted headless
    // session) reaches here with `implementer_lease == None` even though real
    // work is on disk. Resetting would clear the shelve + restore status while
    // the orphaned lease/worktree/commits remain, so the next pickup collides
    // (BUG-436/BUG-438 class). If ANY child-side work exists, leave it shelved
    // for triage. Conservative: any doubt → don't restore. trace:BUG-479 | ai:claude
    let (has_lease, has_worktree, has_unmerged_commits) =
        probe_child_side_work_for_spec(project_root, spec);
    // BUG-1638: every early exit is a refusal the caller reports, never a
    // silent Ok that reads as "restored". trace:BUG-1638 | ai:claude
    if child_side_work_exists(has_lease, has_worktree, has_unmerged_commits) {
        return Ok(Phase1RestoreOutcome::Refused(format!(
            "a lease, worktree or commits exist for {spec}, so it stays shelved for triage \
             (run `aida findings list`)"
        )));
    }

    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(Phase1RestoreOutcome::Refused(
            "no AIDA store was found for this project".to_string(),
        ));
    };
    let dispenser = load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    let gone = || Phase1RestoreOutcome::Refused(format!("{spec} no longer exists"));
    // trace:TASK-1468 | ai:claude
    let Some(req) = backend.get_requirement_unambiguous(spec)? else {
        return Ok(gone());
    };
    // BUG-1638: the race-test seam between the read and the write.
    // trace:BUG-1638 | ai:claude
    status_write_race_seam(&store_path);
    // BUG-1637: the restore and its terminal-status refusal run on the copy
    // read under the store lock; a refused restore writes nothing.
    // trace:BUG-1637 | ai:claude
    let mut applied = false;
    let mut seen = req.status.clone();
    let written = backend.update_spec_atomically(&req, |r| {
        seen = r.status.clone();
        applied = apply_phase1_restore(r, prior);
    })?;
    Ok(match (written, applied) {
        (None, _) => gone(),
        (Some(_), true) => Phase1RestoreOutcome::Restored,
        (Some(_), false) => Phase1RestoreOutcome::Refused(format!(
            "it is now {seen}, a final status, so it was left as is"
        )),
    })
}

/// TASK-358 tests — the `--escalate-blocks` worktree cleanup.
///
/// The wiring exercised here:
///   1. `mark_lease_escalated_to_human` stamps the timestamp on the lease
///      file and round-trips through TOML.
///   2. `cleanup_escalated_leases_for_spec` only touches leases that
///      (a) carry the marker AND (b) match the spec by scope. A bare
///      interactive lease on the same spec is ignored (preserves user work),
///      and an escalated lease for an unrelated spec is ignored (the triage
///      hook is per-spec).
///
/// trace:BUG-511 | ai:claude
#[cfg(test)]
#[path = "tests/bug_511_review_lease_tests.rs"]
mod bug_511_review_lease_tests;

// The per-spec liveness verdict (`aida status <spec>`) and the `aida why`
// STALLED surfacing. trace:STORY-694 trace:BUG-623 | ai:claude
#[cfg(test)]
#[path = "tests/story_694_spec_liveness_tests.rs"]
mod story_694_spec_liveness_tests;

// BUG-637: the spec-scoped CLAIM gates — pre-pickup refuse (the BUG-634
// duplicate-fanout fix) + pre-edit liveness (the EPIC-54-reject fix). Both gates
// are pure over the lease set plus an injected liveness predicate, so the matrix
// is testable without a live process or a lease dir.
// trace:BUG-637 | ai:claude
#[cfg(test)]
#[path = "tests/bug_637_spec_claim_tests.rs"]
mod bug_637_spec_claim_tests;

/// TASK-957: `aida claim` / `aida unclaim` — the advisor-fan-out claim that
/// closes the BUG-637 gap. A claim is a spec-scoped advisory lease
/// (`claim_verb = true`, no worktree, pid-based liveness) that the BUG-637 gates
/// pick up exactly like an AIDA-launched lease.
// trace:TASK-957 | ai:claude
#[cfg(test)]
#[path = "tests/task_957_claim_tests.rs"]
mod task_957_claim_tests;

// The global running-work table (`aida ps`). trace:STORY-696 | ai:claude
#[cfg(test)]
#[path = "tests/story_696_ps_tests.rs"]
mod story_696_ps_tests;

// `held_prs` derived from the verdict corpus, so a refusal recorded outside
// `aida review record` is still surfaced. trace:BUG-1773 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1773_corpus_held_prs_tests.rs"]
mod bug_1773_corpus_held_prs_tests;

// The merge chokepoint and the marker/label mirrors derived from the same
// verdict corpus, so a refusal from any producer holds the PR.
// trace:BUG-1774 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1774_corpus_merge_gate_tests.rs"]
mod bug_1774_corpus_merge_gate_tests;

// `aida ps` flags a live seat whose mail identity would fall back to the
// shell user. trace:TASK-1451 | ai:claude
#[cfg(test)]
#[path = "tests/task_1451_mail_identity_ps_tests.rs"]
mod task_1451_mail_identity_ps_tests;

// The pending-approval marker's priority over the heuristic classifier, plus
// `aida ps` / `aida awaiting` end-to-end rendering of a Blocked seat.
// trace:TASK-1454 | ai:claude
#[cfg(test)]
#[path = "tests/task_1454_pending_approval_tests.rs"]
mod task_1454_pending_approval_tests;

// The orphaned-In-Progress detection → `aida awaiting` mapping.
// trace:BUG-1523 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1523_orphaned_in_progress_mapping_tests.rs"]
mod bug_1523_orphaned_in_progress_mapping_tests;

// trace:BUG-1656 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1656_subagent_liveness_tests.rs"]
mod bug_1656_subagent_liveness_tests;

// trace:BUG-1680 | ai:antigravity
#[cfg(test)]
#[path = "tests/bug_1680_salvage_main_tests.rs"]
mod bug_1680_salvage_main_tests;

// The three `aida ps` reporting defects: landed work advertised as stalled,
// guessed fan-out ownership, and fan-out rows wearing the parent's role.
// trace:BUG-1681 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1681_ps_reporting_tests.rs"]
mod bug_1681_ps_reporting_tests;

// BUG-1740: `possibly_subagent` must not be corroborated by the reporting
// session, and a harness lease pinned to the project root is not a fan-out.
// trace:BUG-1740 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1740_fanout_self_corroboration_tests.rs"]
mod bug_1740_fanout_self_corroboration_tests;

/// trace:TASK-358 | ai:claude
#[cfg(test)]
#[path = "tests/task_358_escalation_cleanup_tests.rs"]
mod task_358_escalation_cleanup_tests;

pub(crate) fn session_prune_orphans(dry_run: bool, yes: bool) -> Result<()> {
    let home = crate::home_dir().context("HOME not set; cannot locate Claude project dir")?;
    let projects = home.join(".claude/projects");
    if !projects.is_dir() {
        println!("(no ~/.claude/projects dir found)");
        return Ok(());
    }

    struct Orphan {
        path: std::path::PathBuf,
        decoded_cwd: String,
        jsonl_count: usize,
    }

    let mut orphans: Vec<Orphan> = Vec::new();
    let mut active = 0usize;
    let mut errored = 0usize;
    for entry in std::fs::read_dir(&projects)?.filter_map(|e| e.ok()) {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        // Read one jsonl event to recover the authoritative cwd. Skip if
        // the dir has no jsonls — it's empty cruft, treat as orphan.
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
            // Lossy fallback decode: `-home-joe-ai-aida` → `/home/joe/ai/aida`.
            let name = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            name.replace('-', "/")
        });
        let cwd_path = std::path::Path::new(&cwd_str);
        if cwd_path.is_dir() {
            active += 1;
            continue;
        }
        if cwd_str.is_empty() {
            errored += 1;
            continue;
        }
        orphans.push(Orphan {
            path: dir,
            decoded_cwd: cwd_str,
            jsonl_count,
        });
    }

    if orphans.is_empty() {
        println!(
            "(no orphan project dirs; {} active dir{} under {})",
            active,
            if active == 1 { "" } else { "s" },
            projects.display()
        );
        if errored > 0 {
            println!(
                "  ({} dir{} couldn't be inspected — left in place)",
                errored,
                if errored == 1 { "" } else { "s" }
            );
        }
        return Ok(());
    }

    println!(
        "Found {} orphan Claude Code project dir{} (cwd missing):",
        orphans.len(),
        if orphans.len() == 1 { "" } else { "s" }
    );
    println!();
    for o in &orphans {
        println!("  {}", o.path.display().to_string().dimmed());
        println!(
            "      cwd: {}    {} jsonl{}",
            o.decoded_cwd.yellow(),
            o.jsonl_count,
            if o.jsonl_count == 1 { "" } else { "s" }
        );
    }
    println!();

    if dry_run {
        println!("{} (--dry-run; nothing removed)", "Dry run".dimmed());
        return Ok(());
    }

    if !yes {
        use std::io::Write;
        eprint!("Remove these {} orphan dir(s)? [y/N] ", orphans.len());
        std::io::stderr().flush()?;
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let mut removed = 0usize;
    for o in &orphans {
        match std::fs::remove_dir_all(&o.path) {
            Ok(_) => removed += 1,
            Err(e) => eprintln!(
                "{} couldn't remove {}: {}",
                "Warning:".yellow().bold(),
                o.path.display(),
                e
            ),
        }
    }
    println!(
        "{} removed {} orphan project dir{}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        removed,
        if removed == 1 { "" } else { "s" }
    );

    // BUG-80: also sweep planned-cluster manifests for leases that no
    // longer exist in this project. Same `--orphans` flag, same dry-run /
    // yes semantics. trace:BUG-80 | ai:claude
    if let Ok(project_root) = find_project_root() {
        let manifests = session_manifest::list_all_with_paths(&project_root);
        let sessions_dir = project_root.join(".aida").join("sessions");
        let orphan_manifests: Vec<_> = manifests
            .into_iter()
            .filter(|(_, m)| !sessions_dir.join(format!("{}.toml", m.session_id)).exists())
            .collect();
        if !orphan_manifests.is_empty() {
            println!();
            println!(
                "Found {} orphan session manifest{}:",
                orphan_manifests.len(),
                if orphan_manifests.len() == 1 { "" } else { "s" }
            );
            for (p, _) in &orphan_manifests {
                println!("  {}", p.display().to_string().dimmed());
            }
            if !dry_run {
                let mut mremoved = 0usize;
                for (p, _) in &orphan_manifests {
                    if std::fs::remove_file(p).is_ok() {
                        mremoved += 1;
                    }
                }
                println!(
                    "{} removed {} orphan manifest{}",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    mremoved,
                    if mremoved == 1 { "" } else { "s" }
                );
            }
        }
    }

    Ok(())
}

/// TASK-358: explicit pruner for `--escalate-blocks` worktrees whose
/// underlying spec has been triaged out of `NeedsAttention`. The auto-clean
/// in `edit_requirement_cli` covers the happy path; this verb is the
/// recovery surface for cases where the auto-clean didn't fire (an older
/// triage that pre-dates this code, a write that errored, a sibling
/// worktree at edit time). A lease with the marker but whose spec is still
/// in `NeedsAttention` is left alone — the human hasn't triaged yet.
/// trace:TASK-358 | ai:claude
pub(crate) fn session_prune_escalations(dry_run: bool, yes: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let leases = list_leases(&project_root);

    // Two buckets: the prune-eligible (marker set AND spec out of
    // NeedsAttention) and the still-parked (marker set AND spec still in
    // NeedsAttention — left alone, surfaced as info).
    let storage = Storage::new(project_root.join(".aida-store"));
    let store = storage.load().ok();
    let lookup_status = |spec: &str| -> Option<RequirementStatus> {
        store.as_ref().and_then(|s| {
            s.requirements
                .iter()
                .find(|r| {
                    r.spec_id
                        .as_deref()
                        .map(|sid| sid.eq_ignore_ascii_case(spec))
                        .unwrap_or(false)
                })
                .map(|r| r.status.clone())
        })
    };

    let mut eligible: Vec<(SessionLease, Option<RequirementStatus>)> = Vec::new();
    let mut still_parked: Vec<SessionLease> = Vec::new();
    for lease in leases {
        if lease.escalated_to_human.is_none() {
            continue;
        }
        let status = lookup_status(&lease.scope);
        match &status {
            Some(RequirementStatus::NeedsAttention) => still_parked.push(lease),
            _ => eligible.push((lease, status)),
        }
    }

    if !still_parked.is_empty() {
        println!(
            "Skipping {} escalated worktree{} — spec still in Needs Attention:",
            still_parked.len(),
            if still_parked.len() == 1 { "" } else { "s" }
        );
        for l in &still_parked {
            println!(
                "  {}  {}  ({})",
                l.scope.cyan(),
                l.worktree_path.display().to_string().dimmed(),
                "awaiting human triage".dimmed(),
            );
        }
        println!();
    }

    if eligible.is_empty() {
        println!(
            "(no prune-eligible escalated worktrees{})",
            if still_parked.is_empty() {
                ""
            } else {
                " — all marked sessions are still in Needs Attention"
            }
        );
        return Ok(());
    }

    println!(
        "Found {} escalated worktree{} whose spec has been triaged:",
        eligible.len(),
        if eligible.len() == 1 { "" } else { "s" }
    );
    for (l, status) in &eligible {
        let status_label = status
            .as_ref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "  {}  {}  → {}",
            l.scope.cyan(),
            l.worktree_path.display().to_string().dimmed(),
            status_label.green(),
        );
    }
    println!();

    if dry_run {
        println!("{} (--dry-run; nothing removed)", "Dry run".dimmed());
        return Ok(());
    }

    if !yes {
        use std::io::Write;
        eprint!(
            "Remove these {} escalated worktree(s)? [y/N] ",
            eligible.len()
        );
        std::io::stderr().flush()?;
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let mut removed = 0usize;
    for (l, _) in &eligible {
        if force_cleanup_lease(&project_root, l) {
            removed += 1;
        }
    }
    println!(
        "{} removed {} escalated worktree{}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        removed,
        if removed == 1 { "" } else { "s" }
    );
    Ok(())
}

pub(crate) fn session_prune(
    days: u32,
    dry_run: bool,
    yes: bool,
    orphans: bool,
    escalations: bool,
) -> Result<()> {
    // TASK-70: orphan-sweep mode short-circuits the per-file age path.
    // Iterates every project dir under ~/.claude/projects and removes any
    // whose recorded cwd no longer exists on disk (stranded by an
    // earlier `aida session end` that didn't `--purge-cc`).
    // trace:TASK-70 | ai:claude
    if orphans {
        return session_prune_orphans(dry_run, yes);
    }
    // TASK-358: escalations-sweep mode — find lingering `--escalate-blocks`
    // worktrees whose spec has been triaged out of NeedsAttention and clean
    // them up. The auto-clean in `edit_requirement_cli` covers the happy
    // path; this is the explicit recovery surface.
    // trace:TASK-358 | ai:claude
    if escalations {
        return session_prune_escalations(dry_run, yes);
    }

    let cwd = std::env::current_dir().context("could not determine cwd")?;
    let project_root = find_project_root().unwrap_or_else(|_| cwd.clone());

    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(u64::from(days) * 86_400))
        .ok_or_else(|| anyhow::anyhow!("--days {} is too large", days))?;
    let now = std::time::SystemTime::now();

    // Project dirs to walk: the current cwd's encoded dir, plus the parent
    // project's (when run from inside a session worktree). De-dup since
    // they collapse to the same dir for plain non-worktree usage.
    let mut search_dirs: Vec<std::path::PathBuf> = Vec::new();
    let push_dir = |dirs: &mut Vec<std::path::PathBuf>, p: std::path::PathBuf| {
        if p.is_dir() && !dirs.iter().any(|d| d == &p) {
            dirs.push(p);
        }
    };
    if let Ok(d) = session::claude_project_dir(&cwd) {
        push_dir(&mut search_dirs, d);
    }
    if let Some(parent) = parent_project_root_for_session(&cwd) {
        if let Ok(d) = session::claude_project_dir(&parent) {
            push_dir(&mut search_dirs, d);
        }
    }
    // Also include encoded dirs for every active lease's worktree path —
    // they often live OUTSIDE the parent project root (sibling worktrees),
    // so the parent-walk doesn't catch them. We'll skip these in the
    // candidate loop (see active_dirs below) but we still need to know
    // about them for the "skipped active" tally. trace:STORY-60 | ai:claude
    let active_leases = list_leases(&project_root);
    let mut active_dirs: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    for l in &active_leases {
        if let Ok(d) = session::claude_project_dir(&l.worktree_path) {
            active_dirs.insert(d.clone());
            push_dir(&mut search_dirs, d);
        }
    }

    if search_dirs.is_empty() {
        println!("(no Claude Code session directories found for this project)");
        return Ok(());
    }

    let mut candidates: Vec<PruneCandidate> = Vec::new();
    let mut skipped_active = 0usize;
    for dir in &search_dirs {
        if active_dirs.contains(dir) {
            // Defensive: anything in here belongs to an in-progress
            // session, regardless of mtime. Skip wholesale.
            // trace:STORY-60 | ai:claude
            if let Ok(read) = std::fs::read_dir(dir) {
                skipped_active += read
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jsonl"))
                    .count();
            }
            continue;
        }
        let Ok(read) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in read.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let Ok(mtime) = metadata.modified() else {
                continue;
            };
            if mtime >= cutoff {
                continue;
            }
            let age_seconds = now.duration_since(mtime).map(|d| d.as_secs()).unwrap_or(0);
            candidates.push(PruneCandidate {
                path,
                size: metadata.len(),
                age_seconds,
            });
        }
    }

    if candidates.is_empty() {
        println!(
            "(no .jsonl files older than {} day{} in {} session director{})",
            days,
            if days == 1 { "" } else { "s" },
            search_dirs.len(),
            if search_dirs.len() == 1 { "y" } else { "ies" },
        );
        if skipped_active > 0 {
            println!(
                "  ({} file{} skipped — belong to {} active session{})",
                skipped_active,
                if skipped_active == 1 { "" } else { "s" },
                active_dirs.len(),
                if active_dirs.len() == 1 { "" } else { "s" },
            );
        }
        return Ok(());
    }

    // Sort oldest-first so the user sees the most-stale entries up top.
    candidates.sort_by(|a, b| b.age_seconds.cmp(&a.age_seconds));

    let total_size: u64 = candidates.iter().map(|c| c.size).sum();
    let count = candidates.len();
    println!(
        "Found {} .jsonl file{} older than {} day{} ({} total):",
        count,
        if count == 1 { "" } else { "s" },
        days,
        if days == 1 { "" } else { "s" },
        humanize_size(total_size)
    );
    println!();
    for c in &candidates {
        let id = c.path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
        let id_short = &id[..id.len().min(8)];
        println!(
            "  {:>6}  {:>10}  {}  {}",
            humanize_age_secs(c.age_seconds),
            humanize_size(c.size),
            id_short.dimmed(),
            c.path.display()
        );
    }
    println!();
    if skipped_active > 0 {
        println!(
            "{} {} file{} from {} active session{} excluded.",
            "Note:".yellow().bold(),
            skipped_active,
            if skipped_active == 1 { "" } else { "s" },
            active_dirs.len(),
            if active_dirs.len() == 1 { "" } else { "s" },
        );
    }

    if dry_run {
        println!("{}", "(--dry-run; nothing deleted)".dimmed());
        return Ok(());
    }

    if !yes {
        use std::io::Write;
        print!(
            "Delete {} file{}? [y/N] ",
            count,
            if count == 1 { "" } else { "s" }
        );
        std::io::stdout().flush().ok();
        let mut ans = String::new();
        if std::io::stdin().read_line(&mut ans).is_err()
            || !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes")
        {
            println!("Aborted.");
            return Ok(());
        }
    }

    let log_path = project_root.join(".aida").join("session-prune.log");
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut log_handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();

    let mut deleted = 0usize;
    let mut bytes_freed = 0u64;
    let mut errors = 0usize;
    let now_iso = chrono::Utc::now().to_rfc3339();
    for c in &candidates {
        match std::fs::remove_file(&c.path) {
            Ok(_) => {
                deleted += 1;
                bytes_freed += c.size;
                if let Some(f) = log_handle.as_mut() {
                    use std::io::Write;
                    let _ = writeln!(
                        f,
                        "{}\t{}\t{}\t{}",
                        now_iso,
                        c.size,
                        c.age_seconds,
                        c.path.display()
                    );
                }
            }
            Err(e) => {
                errors += 1;
                eprintln!(
                    "{} could not delete {}: {}",
                    "Warning:".yellow().bold(),
                    c.path.display(),
                    e
                );
            }
        }
    }

    println!(
        "{} Deleted {} file{} ({} freed){}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        deleted,
        if deleted == 1 { "" } else { "s" },
        humanize_size(bytes_freed),
        if errors > 0 {
            format!("; {} error{}", errors, if errors == 1 { "" } else { "s" })
        } else {
            String::new()
        }
    );
    if log_handle.is_some() {
        println!("  log: {}", log_path.display().to_string().dimmed());
    }
    Ok(())
}

/// STORY-68: detail view for one session lease. Resolution: explicit id
/// → cwd-based lease → ancestor-PID match (same chain as session end's
/// resolution chain, minus the single-active prompt — `show` should be
/// non-interactive). Errors with the active-lease listing if nothing
/// resolves. trace:STORY-68 | ai:claude
pub(crate) fn session_show(id: Option<&str>, plan: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let leases = list_leases(&project_root);
    if leases.is_empty() {
        println!("(no active sessions)");
        println!();
        println!(
            "Start one with: {} {}",
            "aida session start --owns".cyan(),
            "<scope>".dimmed()
        );
        return Ok(());
    }

    let lease = if let Some(q) = id {
        find_lease_by_id_prefix(q, &leases)?
    } else if let Some(l) = std::env::current_dir().ok().and_then(|cwd| {
        let canon = cwd.canonicalize().unwrap_or(cwd);
        leases
            .iter()
            .find(|&l| lease_covers_cwd(l, &canon))
            .cloned()
    }) {
        l
    } else {
        // Ancestor-PID fallback (no env-var lookup here — `show` is for
        // human inspection, not env mutation, so we want a deterministic
        // best-effort match without relying on the export side channel).
        let chain: std::collections::HashSet<u32> =
            process_probe::walk_ancestor_pids(std::process::id())
                .into_iter()
                .collect();
        let candidates: Vec<&SessionLease> = leases
            .iter()
            .filter(|l| l.creator_pid.map(|p| chain.contains(&p)).unwrap_or(false))
            .collect();
        match candidates.len() {
            1 => candidates[0].clone(),
            _ => {
                let mut msg = String::from(
                    "no session lease covers cwd or this shell's ancestors — pass an id explicitly\n\nActive sessions:",
                );
                for l in &leases {
                    msg.push_str(&format!(
                        "\n  {} {} ({})",
                        &l.id[..8],
                        l.scope,
                        l.worktree_path.display()
                    ));
                }
                anyhow::bail!(msg)
            }
        }
    };

    let lease_file = lease_path(&project_root, &lease.id);
    let started_local = lease.started_at.with_timezone(&chrono::Local);
    let now = chrono::Utc::now();
    let age = now.signed_duration_since(lease.started_at);
    let age_human = humanize_relative(lease.started_at);

    println!("{} {}", "Session".bold(), (&lease.id[..8]).yellow());
    println!("  {}: {}", "scope".bold(), lease.scope.cyan());
    println!("  {}: {}", "branch".bold(), lease.branch.cyan());
    println!("  {}: {}", "worktree".bold(), lease.worktree_path.display());
    if let Some(role) = &lease.role {
        println!(
            "  {}: {} {}",
            "role".bold(),
            role.cyan(),
            "(inherited from shell at start time)".dimmed()
        );
    }
    println!(
        "  {}: {} ({})",
        "started_at".bold(),
        started_local.format("%Y-%m-%d %H:%M %Z"),
        age_human.dimmed()
    );
    if age.num_seconds() < 0 {
        // Future-dated lease (clock skew or hand-edited TOML) — flag it.
        eprintln!(
            "  {} lease started_at is in the future — clock skew?",
            "Warning:".yellow().bold()
        );
    }
    println!("  {}: {}", "owner".bold(), lease.owner);
    println!("  {}: {}", "hostname".bold(), lease.hostname);
    // STORY-71 / TASK-51: when this is a review session (--owns PR-N or
    // MR-N), session_start captured the head/base SHAs and base ref.
    // Render them on one line so the reviewer can see what diff range
    // they're working against. Omit the line entirely for non-PR
    // sessions so non-review output stays clean.
    // trace:STORY-71, TASK-51 | ai:claude
    if lease.pr_head_sha.is_some() || lease.pr_base_sha.is_some() || lease.pr_base_ref.is_some() {
        let head = lease
            .pr_head_sha
            .as_deref()
            .map(|s| s[..s.len().min(12)].to_string())
            .unwrap_or_else(|| "?".to_string());
        let base = match (lease.pr_base_ref.as_deref(), lease.pr_base_sha.as_deref()) {
            (Some(r), Some(b)) => format!("{} ({})", r, &b[..b.len().min(12)]),
            (Some(r), None) => r.to_string(),
            (None, Some(b)) => b[..b.len().min(12)].to_string(),
            (None, None) => "?".to_string(),
        };
        println!(
            "  {}: head {}  base {}",
            "PR / MR".bold(),
            head.cyan(),
            base.cyan()
        );
    }
    if let Some(pid) = lease.creator_pid {
        let descendant = {
            let chain: std::collections::HashSet<u32> =
                process_probe::walk_ancestor_pids(std::process::id())
                    .into_iter()
                    .collect();
            chain.contains(&pid)
        };
        let suffix = if descendant {
            " (this shell is a descendant)".green().to_string()
        } else {
            String::new()
        };
        println!("  {}: {}{}", "creator_pid".bold(), pid, suffix);
    }
    println!(
        "  {}: {}",
        "lease file".bold(),
        lease_file.display().to_string().dimmed()
    );

    // Activity rows from the session-local log.
    let activity = load_session_activity(&project_root, &lease.id);
    println!();
    if activity.entries.is_empty() {
        println!("{}", "Activity in this session: (none yet)".bold());
    } else {
        println!("{}", "Activity in this session:".bold());
        // BUG-83: resolve activity-log spec_id keys to their preferred
        // display id (agreed_id when assigned). The log records whatever
        // form the user typed at the call site, so a row written under
        // legacy `STORY-2-076` should render here as the short
        // post-merge-gate id once one's been minted.
        // trace:BUG-83 | ai:claude
        let display_lookup = build_display_id_lookup(&project_root);
        let mut sorted = activity.entries.clone();
        sorted.sort_by_key(|e| std::cmp::Reverse(e.at));
        for e in sorted.iter().take(10) {
            let display_id = display_lookup
                .get(&e.spec_id)
                .cloned()
                .unwrap_or_else(|| e.spec_id.clone());
            println!(
                "  {:<14} {:<10} {} ({})",
                display_id.cyan(),
                e.action,
                e.role.dimmed(),
                humanize_relative(e.at).dimmed()
            );
        }
        if activity.entries.len() > 10 {
            println!(
                "  {} {} more",
                "…".dimmed(),
                (activity.entries.len() - 10).to_string().dimmed()
            );
        }
    }

    // Liveness: is there a live claude inside this worktree right now?
    let live_in_wt = probe_live_claudes_in_worktree(&lease.worktree_path);
    println!();
    if live_in_wt.is_empty() {
        println!("{}: (no live claude detected)", "Claude Code".bold());
    } else {
        println!(
            "{}: {} live claude process{} in worktree",
            "Claude Code".bold(),
            live_in_wt.len().to_string().green(),
            if live_in_wt.len() == 1 { "" } else { "es" }
        );
        for s in &live_in_wt {
            let jsonl_note = match &s.jsonl {
                Some(p) => format!(
                    " (jsonl {})",
                    p.file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("?")
                        .dimmed()
                ),
                None => String::new(),
            };
            println!("  pid {}{}", s.pid.to_string().green(), jsonl_note);
        }
    }

    let dangling = probe_dangling_claudes_at_path(&lease.worktree_path);
    if !dangling.is_empty() {
        println!();
        println!(
            "{} {} leaked claude process(es) with `(deleted)` cwd at this path:",
            "Warning:".yellow().bold(),
            dangling.len()
        );
        for s in &dangling {
            println!(
                "  pid {} — `kill {}` to clean up",
                s.pid.to_string().yellow(),
                s.pid
            );
        }
    }

    if plan {
        render_session_manifest(&project_root, &lease.id)?;
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionHandoffRecommendation {
    StartFresh,
    Compact,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionHandoffFacts {
    pub(crate) live_drain: Option<String>,
    pub(crate) stale_drain: Option<String>,
    pub(crate) live_session_leases: Vec<String>,
}

pub(crate) fn decide_session_handoff(facts: &SessionHandoffFacts) -> SessionHandoffRecommendation {
    if facts.live_drain.is_some() || !facts.live_session_leases.is_empty() {
        SessionHandoffRecommendation::Compact
    } else {
        SessionHandoffRecommendation::StartFresh
    }
}

pub(crate) fn active_handoff_leases(
    leases: &[SessionLease],
    cwd: Option<&std::path::Path>,
) -> Vec<String> {
    let mut out = Vec::new();
    for lease in leases {
        let covers_cwd = cwd.is_some_and(|cwd| lease_covers_cwd(lease, cwd));
        if covers_cwd {
            out.push(format!(
                "{} {} ({})",
                &lease.id[..8.min(lease.id.len())],
                lease.scope,
                lease.branch
            ));
        }
    }
    out.sort();
    out.dedup();
    out
}

pub(crate) fn collect_session_handoff_facts(project_root: &std::path::Path) -> SessionHandoffFacts {
    let (live_drain, stale_drain) = match drain_lock::probe_lock(project_root) {
        drain_lock::LockStatus::Running(lock) => (
            Some(format!(
                "pid {} on {}: {}",
                lock.pid, lock.host, lock.command
            )),
            None,
        ),
        drain_lock::LockStatus::Stale(lock) => (
            None,
            Some(format!(
                "pid {} on {} is not alive: {}",
                lock.pid, lock.host, lock.command
            )),
        ),
        drain_lock::LockStatus::None => (None, None),
    };
    let cwd = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.canonicalize().unwrap_or(cwd));
    let live_session_leases = active_handoff_leases(&list_leases(project_root), cwd.as_deref());
    SessionHandoffFacts {
        live_drain,
        stale_drain,
        live_session_leases,
    }
}

/// STORY-1464: write or show a seat handoff note (the rotation primitive).
// trace:STORY-1464 | ai:claude
pub(crate) fn session_handoff_seat(
    write: Option<&str>,
    _show: bool,
    seat: Option<&str>,
) -> Result<()> {
    let seat = match seat {
        Some(s) => s.to_string(),
        None => std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| canonical_role_name(&s))
            .ok_or_else(|| {
                anyhow::anyhow!("no seat given: pass --seat <seat> or set AIDA_SESSION_ROLE")
            })?,
    };
    let project_root = main_worktree_root_from(&find_project_root()?);
    if let Some(src) = write {
        let body = if src == "-" {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
            s
        } else {
            std::fs::read_to_string(src).with_context(|| format!("reading {src}"))?
        };
        let written =
            seat_rotation::write_handoff(&project_root, &seat, &body, chrono::Utc::now())?;
        println!(
            "Handoff written: {} ({} bytes)",
            written.path.display(),
            written.bytes
        );
        println!("Latest: {}", written.latest.display());
        println!("Next session: aida session handoff --seat {seat} --show");
        return Ok(());
    }
    match seat_rotation::read_latest_handoff(&project_root, &seat)? {
        Some(text) => print!("{text}"),
        None => println!("No handoff recorded for seat '{seat}'."),
    }
    Ok(())
}

pub(crate) fn session_handoff_check(_check: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let facts = collect_session_handoff_facts(&project_root);
    let recommendation = decide_session_handoff(&facts);
    // trace:STORY-1006 | ai:codex
    match recommendation {
        SessionHandoffRecommendation::StartFresh => {
            println!("Recommendation: START FRESH");
            println!("Reason: no live drain or current-session lease pins this conversation.");
        }
        SessionHandoffRecommendation::Compact => {
            println!("Recommendation: COMPACT");
            println!("Reason: live session-bound state is still attached here:");
            if let Some(drain) = &facts.live_drain {
                println!("  - live drain: {drain}");
            }
            for lease in &facts.live_session_leases {
                println!("  - live lease: {lease}");
            }
        }
    }
    if let Some(stale) = &facts.stale_drain {
        println!("Note: stale drain lock present but not a compact pin: {stale}");
    }
    println!(
        "CLI limit: conversation-only residue is unknowable here; run /aida-handoff or /aida-capture before ending the session."
    );
    Ok(())
}

#[cfg(test)]
mod session_handoff_tests {
    use super::*;

    fn facts() -> SessionHandoffFacts {
        SessionHandoffFacts {
            live_drain: None,
            stale_drain: None,
            live_session_leases: Vec::new(),
        }
    }

    #[test]
    fn handoff_recommends_start_fresh_when_substrate_has_no_live_pins() {
        assert_eq!(
            decide_session_handoff(&facts()),
            SessionHandoffRecommendation::StartFresh
        );
    }

    #[test]
    fn handoff_recommends_compact_for_live_drain() {
        let mut f = facts();
        f.live_drain = Some("pid 123: aida queue work --auto-complete".to_string());
        assert_eq!(
            decide_session_handoff(&f),
            SessionHandoffRecommendation::Compact
        );
    }

    #[test]
    fn handoff_recommends_compact_for_live_session_lease() {
        let mut f = facts();
        f.live_session_leases
            .push("abcd1234 STORY-1006 (story-1006)".to_string());
        assert_eq!(
            decide_session_handoff(&f),
            SessionHandoffRecommendation::Compact
        );
    }

    #[test]
    fn handoff_stale_drain_does_not_pin_compaction() {
        let mut f = facts();
        f.stale_drain = Some("pid 123 is not alive".to_string());
        assert_eq!(
            decide_session_handoff(&f),
            SessionHandoffRecommendation::StartFresh
        );
    }
}

/// Render the planned-cluster manifest for `session_id` as a status table.
/// Reads the requirement store once to resolve each item's current status
/// (so a spec the user flipped to "in progress" via /aida-pickup actually
/// shows as the in-flight glyph regardless of whether mark-started ran). trace:STORY-98
pub(crate) fn render_session_manifest(
    project_root: &std::path::Path,
    session_id: &str,
) -> Result<()> {
    let path = session_manifest::manifest_path(project_root, session_id);
    println!();
    if !path.exists() {
        println!(
            "{} (no planned-cluster manifest — /aida-pickup hasn't recorded one for this session)",
            "Plan:".bold()
        );
        println!(
            "  {} {}",
            "tip:".dimmed(),
            "/aida-pickup writes the manifest when it confirms a multi-item cluster".dimmed()
        );
        return Ok(());
    }
    let manifest = session_manifest::load(&path)?;

    // Resolve each item's current store status — expensive, so do it once.
    // Git-canonical first (the default backend now), falling back to the
    // legacy Storage path if no .aida-store/ is present.
    // trace:STORY-98 | ai:claude
    let store = load_store_for_lookup(project_root);

    let mut done = 0usize;
    let mut in_progress = 0usize;
    let mut pending = 0usize;
    let total = manifest.items.len();

    // Pre-compute per-item (status, title, display_id).
    // BUG-83: the manifest stores spec_id (canonical), but the table
    // should display agreed_id when present so the plan view matches
    // `aida list` / queue. trace:BUG-83 | ai:claude
    let mut rows: Vec<(
        session_manifest::ItemStatus,
        &session_manifest::ManifestItem,
        String, // title
        String, // display_id
    )> = Vec::with_capacity(total);
    for item in &manifest.items {
        let (status_str, title, display_id) = if let Some(store) = &store {
            let req = store
                .requirements
                .iter()
                .find(|r| r.spec_id.as_deref() == Some(item.spec_id.as_str()));
            (
                req.map(|r| format!("{}", r.status)),
                req.map(|r| r.title.clone())
                    .unwrap_or_else(|| "(not found)".to_string()),
                req.map(|r| r.display_id())
                    .unwrap_or_else(|| item.spec_id.clone()),
            )
        } else {
            (None, String::new(), item.spec_id.clone())
        };
        let st = session_manifest::classify_item(item, status_str.as_deref());
        match st {
            session_manifest::ItemStatus::Done => done += 1,
            session_manifest::ItemStatus::InProgress => in_progress += 1,
            session_manifest::ItemStatus::Pending => pending += 1,
        }
        rows.push((st, item, title, display_id));
    }

    println!(
        "{} {} items, {} done, {} in-progress, {} pending",
        "Plan:".bold(),
        total,
        done.to_string().green(),
        in_progress.to_string().yellow(),
        pending.to_string().dimmed(),
    );
    println!(
        "  {} {} · {} {}",
        "planned_at:".dimmed(),
        manifest
            .planned_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M %Z"),
        "source:".dimmed(),
        manifest.plan_source.cyan()
    );
    // TASK-272: surface the batch marker so /aida-pickup and /aida-pr can
    // detect batch context from this view. trace:TASK-272 | ai:claude
    if let Some(batch) = &manifest.batch_name {
        println!("  {} {}", "batch:".dimmed(), batch.cyan());
    }
    println!();
    println!(
        "  {:<3} {:<14} {:<14} {}",
        "#".dimmed(),
        "Status".dimmed(),
        "Spec".dimmed(),
        "Title".dimmed()
    );
    for (st, item, title, display_id) in &rows {
        let glyph = st.glyph();
        let label = st.label();
        let display = format!("{} {}", glyph, label);
        let colored = match st {
            session_manifest::ItemStatus::Done => display.green(),
            session_manifest::ItemStatus::InProgress => display.yellow(),
            session_manifest::ItemStatus::Pending => display.dimmed(),
        };
        println!(
            "  {:<3} {:<14} {:<14} {}",
            item.position,
            colored,
            display_id.bold(),
            title
        );
    }

    // TASK-95: the plan brief — Critical Files / Followups / Verification
    // pre-populated by `aida queue work` from the owning docs/plans/ file.
    // This is what /aida-pickup surfaces so the implementer gets their
    // blast radius and definition of done without grepping. trace:TASK-95
    if let Some(ctx) = &manifest.plan {
        println!();
        println!("{} {}", "Plan brief:".bold(), ctx.plan_file.cyan());
        if !ctx.critical_files.is_empty() {
            println!(
                "  {} ({})",
                "Critical files".dimmed(),
                ctx.critical_files.len()
            );
            for f in &ctx.critical_files {
                println!("    {}", f);
            }
        }
        if !ctx.followups.is_empty() {
            println!("  {} ({})", "Followups".dimmed(), ctx.followups.len());
            for f in &ctx.followups {
                println!("    - {}", f);
            }
        }
        if let Some(v) = &ctx.verification {
            println!("  {}", "Definition of done".dimmed());
            for l in v.lines() {
                println!("    {}", l.dimmed());
            }
        }
    }

    Ok(())
}

/// STORY-98: best-effort manifest-row flip on `aida edit --status`.
/// Resolves the active session lease via cwd / ancestor-PID match, then
/// stamps started_at or completed_at on the matching item. Silent no-op
/// when no lease covers this shell or the manifest doesn't list the
/// spec — most edits happen outside a planned cluster, so the path stays
/// quiet by design. trace:STORY-98 | ai:claude
pub(crate) fn update_manifest_for_status(spec_id: &str, canonical_status: &str) {
    let Ok(project_root) = find_project_root() else {
        return;
    };
    let leases = list_leases(&project_root);
    if leases.is_empty() {
        return;
    }
    let lease = match std::env::current_dir().ok() {
        Some(cwd) => {
            let canon = cwd.canonicalize().unwrap_or(cwd);
            leases
                .iter()
                .find(|&l| lease_covers_cwd(l, &canon))
                .cloned()
        }
        None => None,
    }
    .or_else(|| {
        let chain: std::collections::HashSet<u32> =
            process_probe::walk_ancestor_pids(std::process::id())
                .into_iter()
                .collect();
        let ancestor: Vec<&SessionLease> = leases
            .iter()
            .filter(|l| l.creator_pid.map(|p| chain.contains(&p)).unwrap_or(false))
            .collect();
        if ancestor.len() == 1 {
            Some(ancestor[0].clone())
        } else {
            None
        }
    });
    let Some(lease) = lease else {
        return;
    };
    let path = session_manifest::manifest_path(&project_root, &lease.id);
    if !path.exists() {
        return;
    }
    let lower = canonical_status.to_ascii_lowercase();
    let res = if lower == "in-progress" || lower == "in_progress" || lower == "in progress" {
        session_manifest::mark_started(&path, spec_id)
    } else if lower == "done" || lower == "completed" || lower == "rejected" {
        // STORY-86: `Done` checks the manifest item off the same way
        // `Completed` does. The cluster only cares "is this item visually
        // shipped?" — both states qualify (Done = on branch, Completed =
        // on main; either counts as "the work is no longer in flight").
        // trace:STORY-86 | ai:claude
        session_manifest::mark_completed(&path, spec_id)
    } else {
        return; // draft/approved transitions don't move the manifest needle
    };
    if let Err(e) = res {
        eprintln!(
            "{} session-manifest update failed for {} ({}): {}",
            "Warning:".yellow().bold(),
            spec_id,
            canonical_status,
            e
        );
    }
}

/// Build a map from "any id form a caller might have stored"
/// (spec_id or agreed_id) to the requirement's preferred display id
/// (agreed_id when assigned, else spec_id). Used by the session-show
/// activity rows so log entries written under either form render
/// consistently as the short post-merge-gate id once one's been minted.
///
/// Returns an empty map when the store can't be loaded — callers fall
/// back to the raw stored id. trace:BUG-83 | ai:claude
pub(crate) fn build_display_id_lookup(
    project_root: &std::path::Path,
) -> std::collections::HashMap<String, String> {
    let Some(store) = load_store_for_lookup(project_root) else {
        return std::collections::HashMap::new();
    };
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for r in &store.requirements {
        let display = r.display_id();
        if let Some(sid) = r.spec_id.as_deref() {
            map.insert(sid.to_string(), display.clone());
        }
        if let Some(aid) = r.agreed_id.as_deref() {
            map.entry(aid.to_string()).or_insert(display);
        }
    }
    map
}

/// Best-effort load of the requirements store for read-only spec-id /
/// title / status lookups inside session-manifest rendering. Tries
/// distributed git-canonical mode first (the default), then falls back
/// to the legacy YAML/SQLite path. Returns None on any error — caller
/// renders with "(not found)" placeholders so missing data degrades
/// gracefully.
///
/// Pure with respect to `project_root`: BOTH resolution modes read only stores
/// reachable from that root, so this can never return another project's
/// requirements. No process CWD, no `REQ_DB_NAME`, no registry default — see
/// the legacy arm's comment for what went wrong when it did (BUG-1732).
// trace:STORY-98 trace:BUG-1732 | ai:claude
pub(crate) fn load_store_for_lookup(
    project_root: &std::path::Path,
) -> Option<aida_core::RequirementsStore> {
    // Git-canonical: walk up from project_root looking for an .aida/config.toml.
    if let Some(store_path) = detect_distributed_store_from(project_root) {
        if let Ok(backend) = aida_core::GitBackend::new(&store_path) {
            if let Ok(s) = aida_core::DatabaseBackend::load(&backend) {
                return Some(s);
            }
        }
    }
    // Legacy fallback for projects that haven't migrated. BUG-1732: this used
    // `determine_requirements_path(None)`, which probes for `requirements.db` /
    // `requirements.yaml` relative to the PROCESS CWD and honours an ambient
    // `REQ_DB_NAME` — both of which ignore the `project_root` every one of this
    // function's ~70 callers went to the trouble of resolving. On a host where
    // the cwd happens to hold an unrelated legacy store, a project-scoped
    // lookup silently returned that other project's requirements, which is how
    // `aida awaiting` came to report six In-Progress specs from a different
    // project in its orphaned-in-progress channel. BUG-1574's author already
    // documented this hazard and hand-bypassed the fallback for one call site
    // (see `unattended_git_mutation_refusal`); this makes the function itself
    // pure with respect to its argument so no caller has to remember.
    //
    // `resolve_requirements_path_in` is reused rather than reimplemented so the
    // probe ORDER (`requirements.db` before `requirements.yaml`) stays defined
    // in one place. Passing `project_root` as its cwd and `None` for both the
    // `-p` option and `REQ_DB_NAME` leaves no ambient input: on that arm the
    // resolver returns before it ever reads the registry, so the empty registry
    // path below is inert and is passed to make that unreadability explicit
    // rather than to imply the registry participates.
    // trace:BUG-1732 | ai:claude
    aida_core::resolve_requirements_path_in(project_root, None, None, std::path::Path::new(""))
        .ok()
        .map(Storage::new)
        .and_then(|s| s.load().ok())
}

/// `aida session manifest <subcommand>` dispatcher. trace:STORY-98
pub(crate) fn session_manifest_dispatch(cmd: &SessionManifestCommand) -> Result<()> {
    match cmd {
        SessionManifestCommand::Write {
            items,
            source,
            session,
        } => session_manifest_write(items, source, session.as_deref()),
        SessionManifestCommand::MarkStarted { spec_id, session } => {
            session_manifest_mark(spec_id, session.as_deref(), MarkKind::Started)
        }
        SessionManifestCommand::MarkCompleted { spec_id, session } => {
            session_manifest_mark(spec_id, session.as_deref(), MarkKind::Completed)
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum MarkKind {
    Started,
    Completed,
}

/// Resolve the target lease for a manifest operation: explicit id wins,
/// else the lease covering cwd, else ancestor-PID match, else error.
/// Centralized so all three manifest subcommands resolve identically.
pub(crate) fn resolve_manifest_target_lease(
    project_root: &std::path::Path,
    session_query: Option<&str>,
) -> Result<SessionLease> {
    let leases = list_leases(project_root);
    if leases.is_empty() {
        anyhow::bail!(
            "no active sessions — start one with `aida session start --owns <scope>` before writing a manifest"
        );
    }
    if let Some(q) = session_query {
        return find_lease_by_id_prefix(q, &leases);
    }
    if let Ok(cwd) = std::env::current_dir() {
        let canon = cwd.canonicalize().unwrap_or(cwd);
        if let Some(l) = leases.iter().find(|&l| lease_covers_cwd(l, &canon)) {
            return Ok(l.clone());
        }
    }
    let chain: std::collections::HashSet<u32> =
        process_probe::walk_ancestor_pids(std::process::id())
            .into_iter()
            .collect();
    let ancestor: Vec<&SessionLease> = leases
        .iter()
        .filter(|l| l.creator_pid.map(|p| chain.contains(&p)).unwrap_or(false))
        .collect();
    if ancestor.len() == 1 {
        return Ok(ancestor[0].clone());
    }
    anyhow::bail!(
        "could not resolve a single active session — pass --session <id> ({} active)",
        leases.len()
    );
}

pub(crate) fn session_manifest_write(
    items: &str,
    source: &str,
    session_query: Option<&str>,
) -> Result<()> {
    let project_root = find_project_root()?;
    let lease = resolve_manifest_target_lease(&project_root, session_query)?;

    let parsed: Vec<String> = items
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if parsed.is_empty() {
        anyhow::bail!("--items is empty — pass a comma-separated list of SPEC-IDs");
    }

    // Resolve current store statuses so status_at_plan reflects truth.
    let store = load_store_for_lookup(&project_root);

    let now = chrono::Utc::now();
    let mut manifest_items: Vec<session_manifest::ManifestItem> = Vec::with_capacity(parsed.len());
    // BUG-83: collect parallel display_ids so the confirmation echo
    // shows agreed_id when one's been assigned. The manifest itself
    // keeps spec_id (canonical) on disk so mark-started / mark-completed
    // continue to key off it. trace:BUG-83 | ai:claude
    let mut display_ids: Vec<String> = Vec::with_capacity(parsed.len());
    for (i, spec_id) in parsed.iter().enumerate() {
        let req = store.as_ref().and_then(|s| {
            s.requirements
                .iter()
                .find(|r| r.spec_id.as_deref() == Some(spec_id.as_str()))
        });
        let status = req
            .map(|r| format!("{}", r.status))
            .unwrap_or_else(|| "Unknown".to_string());
        let display_id = req
            .map(|r| r.display_id())
            .unwrap_or_else(|| spec_id.clone());
        manifest_items.push(session_manifest::ManifestItem {
            spec_id: spec_id.clone(),
            position: (i + 1) as u32,
            status_at_plan: status,
            started_at: None,
            completed_at: None,
            note: None,
        });
        display_ids.push(display_id);
    }

    // TASK-272: preserve a batch marker recorded by `aida queue work
    // --batch` — rewriting the planned cluster (e.g. /aida-pickup Step 3a
    // recording a user-confirmed multi-item batch) must not silently drop
    // the batch context /aida-pickup keys its next-steps menu off.
    // trace:TASK-272 | ai:claude
    let path = session_manifest::manifest_path(&project_root, &lease.id);
    let batch_name = session_manifest::load(&path)
        .ok()
        .and_then(|m| m.batch_name);

    let manifest = session_manifest::SessionManifest {
        session_id: lease.id.clone(),
        planned_at: now,
        plan_source: source.to_string(),
        claude_session_id: None,
        batch_name,
        plan: None,
        items: manifest_items,
    };

    session_manifest::save(&path, &manifest)?;

    println!(
        "{} {} for session {} ({} items)",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        "Wrote planned-cluster manifest".bold(),
        (&lease.id[..lease.id.len().min(8)]).yellow(),
        manifest.items.len()
    );
    for (it, display_id) in manifest.items.iter().zip(display_ids.iter()) {
        println!(
            "  {}. {}  [{}]",
            it.position.to_string().dimmed(),
            display_id.bold(),
            it.status_at_plan.dimmed()
        );
    }
    println!();
    println!(
        "  {} {} {}",
        "manifest:".dimmed(),
        path.display().to_string().dimmed(),
        "(view with `aida session show --plan`)".dimmed()
    );

    Ok(())
}

pub(crate) fn session_manifest_mark(
    spec_id: &str,
    session_query: Option<&str>,
    kind: MarkKind,
) -> Result<()> {
    let project_root = find_project_root()?;
    let lease = resolve_manifest_target_lease(&project_root, session_query)?;
    let path = session_manifest::manifest_path(&project_root, &lease.id);
    let updated = match kind {
        MarkKind::Started => session_manifest::mark_started(&path, spec_id)?,
        MarkKind::Completed => session_manifest::mark_completed(&path, spec_id)?,
    };
    let action = match kind {
        MarkKind::Started => "started",
        MarkKind::Completed => "completed",
    };
    if updated {
        println!(
            "{} marked {} as {} in session {} manifest",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            spec_id.bold(),
            action,
            (&lease.id[..lease.id.len().min(8)]).dimmed()
        );
    } else if !path.exists() {
        // Quiet no-op: most edits happen outside a planned cluster. We
        // emit a single dim line so manual `aida session manifest
        // mark-started` calls aren't silently lost, but we don't shout
        // when the hook path runs against every edit.
        println!(
            "{} no manifest for session {} — nothing to mark (run `aida session manifest write` first)",
            "→".dimmed(),
            (&lease.id[..lease.id.len().min(8)]).dimmed()
        );
    } else {
        println!(
            "{} {} not in session {} manifest — not marking",
            "→".dimmed(),
            spec_id,
            (&lease.id[..lease.id.len().min(8)]).dimmed()
        );
    }
    Ok(())
}

/// True when one SHA is a (case-insensitive, hex) prefix of the other.
/// Used by statusline to compare a cache-stored SHA (potentially full,
/// 40 chars) against the current `git rev-parse --short HEAD` output (7
/// chars) without flagging a spurious stale.
/// trace:TASK-1-045 | ai:claude
pub(crate) fn sha_prefix_match(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return a == b;
    }
    let min = a.len().min(b.len());
    a[..min].eq_ignore_ascii_case(&b[..min])
}

/// Statusline cache freshness state. Two independent axes folded into a
/// single label by severity:
///   1. cache.db vs local orphan branch HEAD — `Stale` when they differ
///      (next read will rebuild the cache).
///   2. local orphan branch HEAD vs `origin/aida-store` — `Behind` when
///      local lags origin (run `aida db sync --pull`).
///      `Unknown` covers "can't tell about origin yet" — either no recent
///      fetch (STORY-79 hasn't run, or it's been >threshold) or no
///      `origin/aida-store` ref locally. `Fresh` is the all-good state and
///      doesn't render. trace:STORY-78 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheFreshness {
    Fresh,
    Stale,
    Behind,
    Unknown,
    /// No `.aida-store` attached in this worktree (e.g. a harness /
    /// general-purpose worktree with no orphan store checked out).
    /// Distinct from `Unknown` (origin freshness is transiently
    /// unknowable in an *attached* store): with no store at all there is
    /// nothing to report, so the segment is omitted rather than rendering
    /// a misleading `cache:?`. trace:BUG-518 | ai:claude
    NoStore,
}

impl CacheFreshness {
    /// One-letter label used by statusline. None means "don't render"
    /// (the Fresh case — boring, hide it; and the NoStore case — there is
    /// no store to report on, BUG-518). trace:STORY-78 | ai:claude
    pub(crate) fn label(self) -> Option<&'static str> {
        match self {
            CacheFreshness::Fresh => None,
            CacheFreshness::Stale => Some("stale"),
            CacheFreshness::Behind => Some("behind"),
            CacheFreshness::Unknown => Some("?"),
            // No store attached → omit the segment entirely. trace:BUG-518
            CacheFreshness::NoStore => None,
        }
    }
}

/// Decide which freshness state to render given pre-collected inputs.
/// Pure — no I/O, no env reads — so the precedence rules can be
/// exhaustively tested.
///
/// `local_behind_origin` is a tri-state:
///   - Some(true)  → origin has commits local doesn't (pull needed)
///   - Some(false) → local is equal-to or strictly-ahead-of origin
///     (push may be needed but no pull) — render Fresh
///   - None        → we couldn't determine direction (no recent fetch,
///     no origin ref, or rev-list lookup failed)
///
/// The "strictly ahead → Fresh" semantics keep us from nagging the user
/// to pull when they have unpushed local commits — `cache:behind` would
/// be misleading because there's nothing on origin to pull.
/// trace:STORY-78 | ai:claude
pub(crate) fn classify_cache_freshness(
    recorded_cache_sha: Option<&str>,
    local_sha: &str,
    local_behind_origin: Option<bool>,
    last_fetch_age_secs: Option<u64>,
    freshness_threshold_secs: u64,
) -> CacheFreshness {
    // No local store attached (e.g. a harness / general-purpose worktree
    // with no `.aida-store` checked out) → there is nothing to report.
    // Signal NoStore so the statusline omits the segment rather than
    // rendering a misleading `cache:?` (which means "store attached,
    // origin freshness transiently unknown"). trace:BUG-518 | ai:claude
    if local_sha.is_empty() {
        return CacheFreshness::NoStore;
    }
    // Axis 1: cache vs local. Stale wins over every other state because
    // the immediate consequence (slow next read while cache rebuilds) is
    // user-visible regardless of remote state.
    let cache_matches_local = recorded_cache_sha
        .map(|r| sha_prefix_match(r, local_sha))
        .unwrap_or(false);
    if !cache_matches_local {
        return CacheFreshness::Stale;
    }
    // Axis 2: local vs origin. Requires both a recent fetch AND a
    // direction signal — either missing collapses to Unknown rather
    // than false-fresh. STORY-79 is the producer of the last-fetch
    // timestamp; until it ships, last_fetch_age_secs is always None and
    // we render `?` whenever cache matches local. trace:STORY-79 | ai:claude
    let fetch_is_recent = last_fetch_age_secs
        .map(|age| age <= freshness_threshold_secs)
        .unwrap_or(false);
    if !fetch_is_recent {
        return CacheFreshness::Unknown;
    }
    match local_behind_origin {
        None => CacheFreshness::Unknown,
        Some(true) => CacheFreshness::Behind,
        Some(false) => CacheFreshness::Fresh,
    }
}

/// Decide whether `local_sha` is strictly behind `origin_sha` in the
/// orphan store at `store_path`. "Strictly behind" means: origin has at
/// least one commit not reachable from local. Returns None when git
/// can't answer (e.g. unknown commit, fork without merge-base).
///
/// `local == origin` short-circuits to Some(false). Strictly-ahead also
/// returns Some(false) — we explicitly do NOT report "behind" for the
/// have-unpushed-commits case. trace:STORY-78 | ai:claude
pub(crate) fn local_lags_origin(
    store_path: &std::path::Path,
    local_sha: &str,
    origin_sha: &str,
) -> Option<bool> {
    if sha_prefix_match(local_sha, origin_sha) {
        return Some(false);
    }
    let range = format!("{}..{}", local_sha, origin_sha);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(store_path)
        .args(["rev-list", "--count", &range])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let count: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(count > 0)
}

/// Read `~/.aida/cache/last-fetch.toml` and return the age (in seconds)
/// of the most recent successful fetch for `project_root`. Returns None
/// when the file is absent, the entry is missing, the last result was
/// an error, or the timestamp is unparseable — all of which collapse to
/// "Unknown" in the freshness classifier. trace:STORY-78 | ai:claude
pub(crate) fn read_last_fetch_age_secs(project_root: &std::path::Path) -> Option<u64> {
    let home = aida_home_dir()?;
    let toml_path = home.join(".aida/cache/last-fetch.toml");
    let raw = std::fs::read_to_string(&toml_path).ok()?;
    let value: toml::Value = toml::from_str(&raw).ok()?;
    // Schema (per STORY-79): top-level keys are project root absolute
    // paths; each entry is a table { last_fetch = "<rfc3339>", result =
    // "ok" | "error" }. Look up by canonical project_root path.
    let canon = project_root.canonicalize().ok()?;
    let key = canon.to_string_lossy();
    let entry = value.get(key.as_ref())?;
    let result = entry.get("result").and_then(|v| v.as_str()).unwrap_or("ok");
    if result != "ok" {
        return None;
    }
    let ts_str = entry.get("last_fetch").and_then(|v| v.as_str())?;
    let ts = chrono::DateTime::parse_from_rfc3339(ts_str).ok()?;
    let now = chrono::Utc::now();
    let age = now.signed_duration_since(ts.with_timezone(&chrono::Utc));
    let secs = age.num_seconds();
    if secs < 0 {
        Some(0)
    } else {
        Some(secs as u64)
    }
}

/// `git rev-parse origin/aida-store` against the orphan-store worktree.
/// Returns None when the ref doesn't exist (no remote configured, never
/// fetched) or the git invocation otherwise fails. The caller maps None
/// to `CacheFreshness::Unknown` rather than treating it as
/// "no-difference". trace:STORY-78 | ai:claude
pub(crate) fn rev_parse_origin_aida_store(project_root: &std::path::Path) -> Option<String> {
    let store_path = if project_root.join(".aida-store").exists() {
        project_root.join(".aida-store")
    } else if project_root.join("aida-store").exists() {
        project_root.join("aida-store")
    } else {
        return None;
    };
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&store_path)
        .args(["rev-parse", "--short", "origin/aida-store"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Threshold above which a last-fetch timestamp is treated as "stale"
/// (i.e. we render `cache:?` rather than `cache:fresh|behind`). Matches
/// STORY-79's default background-fetch interval so a single missed fetch
/// cycle is the boundary. Overridable via `AIDA_FETCH_FRESHNESS_SECS` so
/// CI / tests can dial it without hardcoding `300`. trace:STORY-78 | ai:claude
pub(crate) fn cache_freshness_threshold_secs() -> u64 {
    std::env::var("AIDA_FETCH_FRESHNESS_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300)
}

/// STORY-78 Part 2: the implementation of `--sync` on read commands.
/// Mirrors `aida db sync --pull` but without the commit-pending step
/// (read commands have no pending work to commit) and without conflict
/// detection (the caller is about to read the result anyway). Designed
/// to NEVER fail the caller: every failure mode is converted to a
/// warning and the command continues with the local view.
///
/// Failure modes handled here:
///   - store missing / not a git repo → warn, skip
///   - orphan store has uncommitted changes → warn, skip (pull --rebase
///     would abort partway and leave weirder state than we started with)
///   - offline / unreachable origin → warn, skip
///   - any other git error during rebase → abort the rebase, warn, skip
///
/// On success, updates `~/.aida/cache/last-fetch.toml` so the statusline
/// `cache:behind|?` indicator immediately reflects the pull (instead of
/// waiting for STORY-79's background fetcher).
/// trace:STORY-78 | ai:claude
pub(crate) fn maybe_sync_pull(store_path: &std::path::Path) -> Result<()> {
    if !aida_core::git_ops::is_git_repo(store_path) {
        eprintln!(
            "{} --sync: store at {} is not a git repo; skipping pull",
            "Warning:".yellow().bold(),
            store_path.display()
        );
        return Ok(());
    }
    if matches!(aida_core::git_ops::has_changes(store_path), Ok(true)) {
        eprintln!(
            "{} --sync: orphan store has uncommitted changes; skipping pull",
            "Warning:".yellow().bold()
        );
        eprintln!("  Run `aida db sync --pull` manually after committing or stashing.");
        return Ok(());
    }
    let branch =
        aida_core::git_ops::current_branch(store_path).unwrap_or_else(|_| "aida-store".to_string());
    eprintln!("{} pulling origin/{} (--sync)", "→".dimmed(), branch);
    match aida_core::git_ops::pull_rebase(store_path, "origin", &branch) {
        Ok(()) => {
            // Best-effort: refresh the statusline freshness signal so
            // the user doesn't see `cache:?` on the very next prompt
            // after they explicitly pulled. Silent on failure — losing
            // this write costs us a stale-looking indicator for one
            // render cycle, no worse.
            let _ = touch_last_fetch_ok(store_path);
            // TASK-1033: opportunistic store maintenance rides the sync —
            // ensure the lowered gc.auto is set, then `git gc --auto` (a cheap
            // no-op unless the threshold is exceeded). Best-effort.
            aida_core::git_ops::opportunistic_store_gc(store_path);
        }
        Err(e) => {
            // pull --rebase can leave the repo mid-rebase on conflicts
            // or network drops partway through. Abort defensively so
            // the next command doesn't trip on `.git/rebase-merge/`.
            // `git rebase --abort` is harmless when no rebase is in
            // progress (exit code != 0, ignored). trace:STORY-78 | ai:claude
            let _ = std::process::Command::new("git")
                .arg("-C")
                .arg(store_path)
                .args(["rebase", "--abort"])
                .output();
            let first_line = e
                .to_string()
                .lines()
                .next()
                .unwrap_or("unknown error")
                .to_string();
            eprintln!(
                "{} --sync pull failed: {}",
                "Warning:".yellow().bold(),
                first_line
            );
            eprintln!("  Falling back to local view.");
        }
    }
    Ok(())
}

/// Stamp `~/.aida/cache/last-fetch.toml` with "ok"/now for the project
/// whose orphan store is `store_path`. Best-effort — keys on the
/// canonicalized parent of `store_path` so reads (via
/// `read_last_fetch_age_secs(project_root)`) match writes.
/// trace:STORY-78 | ai:claude
pub(crate) fn touch_last_fetch_ok(store_path: &std::path::Path) -> Result<()> {
    write_last_fetch_entry(store_path, "ok")
}

/// Write a result string (`"ok"` for success, `"error: <msg>"` for
/// failure) plus a fresh timestamp into `~/.aida/cache/last-fetch.toml`
/// for the project whose orphan store is `store_path`. Shared between
/// the `--sync` path (STORY-78) and the background fetch worker
/// (STORY-79). trace:STORY-78 | ai:claude
pub(crate) fn write_last_fetch_entry(store_path: &std::path::Path, result: &str) -> Result<()> {
    let project_root = store_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("store path has no parent"))?;
    let canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let home = aida_home_dir().context("no home dir")?;
    let cache_dir = home.join(".aida/cache");
    std::fs::create_dir_all(&cache_dir)?;
    let path = cache_dir.join("last-fetch.toml");

    let mut root: toml::value::Table = match std::fs::read_to_string(&path) {
        Ok(s) => toml::from_str::<toml::Value>(&s)
            .ok()
            .and_then(|v| v.as_table().cloned())
            .unwrap_or_default(),
        Err(_) => toml::value::Table::new(),
    };
    let mut entry = toml::value::Table::new();
    entry.insert(
        "last_fetch".into(),
        toml::Value::String(chrono::Utc::now().to_rfc3339()),
    );
    entry.insert("result".into(), toml::Value::String(result.into()));
    root.insert(
        canon.to_string_lossy().to_string(),
        toml::Value::Table(entry),
    );
    let serialized = toml::to_string(&toml::Value::Table(root))?;
    std::fs::write(&path, serialized)?;
    Ok(())
}

// ───────────────────────────────────────────────────────────────────
// STORY-79 — background fetch from statusline (auto-freshness)
// ───────────────────────────────────────────────────────────────────

/// Default seconds between background fetches. Statusline skips spawning
/// when the most recent fetch (or attempt) is within this window. Overridable
/// via `AIDA_BG_FETCH_INTERVAL_SECS`. trace:STORY-79 | ai:claude
pub(crate) fn bg_fetch_interval_secs() -> u64 {
    std::env::var("AIDA_BG_FETCH_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300)
}

/// Whether the background fetcher is enabled. `AIDA_BG_FETCH=false`
/// (or `0`, `no`, `off`) disables it entirely. Any other value (or
/// unset) leaves it enabled. trace:STORY-79 | ai:claude
pub(crate) fn bg_fetch_enabled() -> bool {
    match std::env::var("AIDA_BG_FETCH") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// Path to the per-project lockfile used to coordinate background
/// fetches across concurrent shells. The basename mixes the project
/// dir name and a hex digest of the canonical path: dir name keeps it
/// debuggable, digest disambiguates two projects with the same
/// basename. trace:STORY-79 | ai:claude
pub(crate) fn bg_fetch_lock_path(store_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let project_root = store_path.parent()?;
    let canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let basename = project_root
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".into());
    // Cheap deterministic digest — we just need enough disambiguation
    // for human + scripted use, not collision resistance. FNV-1a 64-bit
    // by hand to avoid pulling in `siphasher`/`twox-hash`.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in canon.to_string_lossy().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let home = aida_home_dir()?;
    Some(
        home.join(".aida/cache")
            .join(format!(".bg-fetch-{}-{:016x}.lock", basename, h)),
    )
}

/// Best-effort: ensure `~/.aida/cache/` exists for the lock + toml writes.
/// Returns Ok even if creation fails — caller treats missing dir as
/// "skip background fetch this render". trace:STORY-79 | ai:claude
pub(crate) fn ensure_cache_dir() -> Option<std::path::PathBuf> {
    let home = aida_home_dir()?;
    let cache = home.join(".aida/cache");
    std::fs::create_dir_all(&cache).ok()?;
    Some(cache)
}

/// Should statusline kick off a fresh background fetch for this project?
/// Returns true when ALL conditions hold: feature enabled, no recent
/// fetch attempt (per `last-fetch.toml`), and no live lockfile.
/// Stale lockfiles (>3× interval) are considered dead and overridden.
/// Pure decision: callers do the lockfile create + spawn separately.
/// trace:STORY-79 | ai:claude
pub(crate) fn should_spawn_bg_fetch(
    last_fetch_age_secs: Option<u64>,
    lock_age_secs: Option<u64>,
    interval_secs: u64,
) -> bool {
    // Recent successful fetch (or attempt) means we're current.
    if matches!(last_fetch_age_secs, Some(age) if age < interval_secs) {
        return false;
    }
    // Live lockfile: another shell beat us to it. Treat as live for
    // 3× the interval to absorb slow git fetches on weak networks
    // without permanently wedging if a fetch crashed mid-flight.
    if matches!(lock_age_secs, Some(age) if age < interval_secs.saturating_mul(3)) {
        return false;
    }
    true
}

/// Spawn the `_bg-fetch` worker for `store_path` if the project is due
/// for a fetch and no other shell is already on it. All failure modes
/// (no home dir, can't create cache dir, lockfile-create race lost,
/// spawn failed) collapse to "skip silently" so statusline stays
/// silent and sub-50ms regardless. trace:STORY-79 | ai:claude
pub(crate) fn maybe_spawn_bg_fetch(project_root: &std::path::Path, store_path: &std::path::Path) {
    if !bg_fetch_enabled() {
        return;
    }
    let Some(_cache_dir) = ensure_cache_dir() else {
        return;
    };
    let Some(lock_path) = bg_fetch_lock_path(store_path) else {
        return;
    };
    let interval = bg_fetch_interval_secs();
    let last_fetch_age = read_last_fetch_age_secs(project_root);
    let lock_age = std::fs::metadata(&lock_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
        .map(|d| d.as_secs());
    if !should_spawn_bg_fetch(last_fetch_age, lock_age, interval) {
        return;
    }
    // Atomic claim: O_EXCL create. If another shell beat us between the
    // staleness check above and this call, the create fails and we
    // back off. trace:STORY-79 | ai:claude
    let claim = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path);
    let Ok(mut f) = claim else { return };
    use std::io::Write;
    let _ = writeln!(f, "{}", std::process::id());
    // Spawn ourselves as the hidden `_bg-fetch` worker, detached with
    // all stdio nulled. The current binary path comes from argv[0] when
    // it's absolute (cargo dev / installed); otherwise fall back to
    // `aida` on PATH. trace:STORY-79 | ai:claude
    let exe = aida_exe_path();
    let _ = std::process::Command::new(exe)
        .arg("_bg-fetch")
        .arg(store_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn_retrying_etxtbsy();
}

/// Pure source-label resolver for `aida whoami`'s user-id line. Mirrors
/// `current_user_id`'s resolution order (`AIDA_USER` → `USER` → `USERNAME` →
/// default) over already-read env values, so the printed source label can never
/// drift from the value `current_user_id` actually returned. Each arg is the
/// env var's value as `Option`, where an empty string counts as unset (matching
/// the `is_ok_and(|v| !v.is_empty())` checks the resolver uses).
/// trace:TASK-784
#[cfg(test)]
#[path = "tests/statusline_tests.rs"]
mod statusline_tests;

// trace:TASK-1479 | ai:claude
#[cfg(test)]
#[path = "tests/statusline_client_adapters_integration_tests.rs"]
mod statusline_client_adapters_integration_tests;

#[cfg(test)]
#[path = "tests/bug_88_pr_lookup_parse_tests.rs"]
mod bug_88_pr_lookup_parse_tests;

/// BUG-257: `gh_stderr_is_network_error` classifies a `gh` stderr line as a
/// transient network error vs. an auth/parse/other failure. The split powers
/// the `PrLookup::GhUnreachable` variant so the orchestrator can report
/// *Inconclusive* (drain pauses) instead of conflating "no PR" with "can't
/// reach the API". trace:BUG-257 | ai:claude
#[cfg(test)]
#[path = "tests/bug_257_gh_stderr_network_classifier_tests.rs"]
mod bug_257_gh_stderr_network_classifier_tests;

/// BUG-266: `claude_log_indicates_api_outage` classifies the headless
/// implementer's JSONL log as evidence the upstream Anthropic API took the
/// session out (a transient outage), rather than the implementer attempting
/// the work and failing. The orchestrator flips that phase from *failed* to
/// *Inconclusive* (the BUG-257 path), so a 529 / 5xx / stream-timeout no
/// longer marks a spec failed-by-implementer. trace:BUG-266 | ai:claude
#[cfg(test)]
#[path = "tests/bug_266_anthropic_api_outage_classifier_tests.rs"]
mod bug_266_anthropic_api_outage_classifier_tests;

#[cfg(test)]
#[path = "tests/bug_354_text_question_classifier_tests.rs"]
mod bug_354_text_question_classifier_tests;

#[cfg(test)]
#[path = "tests/bug_87_queue_filter_tests.rs"]
mod bug_87_queue_filter_tests;

/// BUG-231: `aida findings promote` flipped status to Approved and printed
/// "joins the work queue" but never called `queue add` — the finding ended
/// up Approved and in no queue (a silent half-success). These cover the
/// fixed `queue_promoted_finding` helper: it routes to a real queue, honors
/// `--for`, and surfaces a queue-add failure as an error.
#[cfg(test)]
#[path = "tests/bug_231_findings_promote_tests.rs"]
mod bug_231_findings_promote_tests;
#[cfg(test)]
#[path = "tests/cr_8_filing_provenance_tests.rs"]
mod cr_8_filing_provenance_tests;
#[cfg(test)]
#[path = "tests/story_1428_gate_promote_tests.rs"]
mod story_1428_gate_promote_tests;

/// BUG-574: reliability papercuts — spurious non-zero exits on the idempotent
/// re-entry paths of `aida session start` (re-running for a scope this clone
/// already leases) and `aida pr ship` (re-running on an already-merged PR).
/// Both are benign no-ops that completed, so they must exit 0 — reserving
/// non-zero for real failures — exactly like the BUG-573 read-only contract.
/// These tests pin the pure decision points that drive the exit-0 behavior.
/// trace:BUG-574 | ai:claude
#[cfg(test)]
#[path = "tests/bug_574_spurious_exit_tests.rs"]
mod bug_574_spurious_exit_tests;

#[cfg(test)]
#[path = "tests/lease_enforcement_tests.rs"]
mod lease_enforcement_tests;

#[cfg(test)]
#[path = "tests/scope_fallback_tests.rs"]
mod scope_fallback_tests;

#[cfg(test)]
#[path = "tests/store_walkup_tests.rs"]
mod store_walkup_tests;

#[cfg(test)]
#[path = "tests/session_end_resolution_tests.rs"]
mod session_end_resolution_tests;

#[cfg(test)]
#[path = "tests/terminal_status_guard_tests.rs"]
mod terminal_status_guard_tests;

/// TASK-292 — `aida queue work --auto-complete` with no positional SPEC id
/// inherits the no-arg "pick the queue head" semantics. These cover the pure
/// pickup-order resolver: drivable-status classification, skipping in-flight
/// heads, and the empty-vs-all-in-flight error split.
#[cfg(test)]
#[path = "tests/auto_complete_head_tests.rs"]
mod auto_complete_head_tests;

/// TASK-293 — the `next` / `nextN` keyword parser. Covers every named form
/// in the acceptance criteria (`next`, `next1`, `next 1`, `next3`, `next 3`)
/// plus the malformed-input edge cases. The drain machinery itself
/// (empty-queue, N > queue-length) is the already-tested `drain_batch` /
/// `pick_auto_complete_head` — this module only covers the pure parse.
/// trace:TASK-293 | ai:claude
#[cfg(test)]
#[path = "tests/next_keyword_tests.rs"]
mod next_keyword_tests;

/// STORY-86 — env-flag opt-out + colorize_status coverage for the new
/// `Done` variant. Integration coverage (set up git+store, exercise the
/// helper end-to-end) lives in the verification script in the plan; the
/// unit slice here protects the bits that are pure functions.
/// trace:STORY-86 | ai:claude
#[cfg(test)]
#[path = "tests/auto_bump_done_tests.rs"]
mod auto_bump_done_tests;

/// BUG-95: regression coverage for the claim that `aida pull --code-only`
/// skips the Done → Completed auto-bump. The bug-as-filed asserted the
/// conditional was nested in the wrong branch; the current structure has
/// the auto-bump correctly inside the code-pull block (gated by
/// `!store_only`), so `--code-only` (code_only=true, store_only=false)
/// reaches it. This test exercises that path end-to-end via
/// `handle_pull_command` and verifies the bump fires.
/// trace:BUG-95 | ai:claude
#[cfg(test)]
#[path = "tests/handle_pull_command_tests.rs"]
mod handle_pull_command_tests;

/// TASK-106: coverage for the `aida push` / `aida pull` scope flags —
/// all four combinations (default / --code-only / --store-only / both)
/// at the clap-parse layer, the `AIDA_PUSH_DEFAULT` env-var override,
/// and the mutual-exclusion guard. trace:TASK-106 | ai:claude
#[cfg(test)]
#[path = "tests/push_pull_scope_tests.rs"]
mod push_pull_scope_tests;

/// TASK-234 / TASK-241: integration coverage for the git-state
/// classifiers behind `aida queue list`'s "Done — awaiting merge"
/// section and `aida show`'s git-linkage section. Both extracted
/// helpers (`classify_in_flight_specs`, `collect_git_linkage`) are
/// gh-free — they only run `git` — so a temp repo with hand-built
/// commits exercises every branch without a GitHub round-trip.
/// trace:TASK-234 TASK-241 | ai:claude
#[cfg(test)]
#[path = "tests/in_flight_linkage_integration_tests.rs"]
mod in_flight_linkage_integration_tests;

/// TASK-241: coverage for the squash-merge PR-number parser that
/// `aida show`'s git-linkage section uses to point shipped specs at
/// their merged PR. trace:TASK-241 | ai:claude
#[cfg(test)]
#[path = "tests/git_linkage_tests.rs"]
mod git_linkage_tests;

/// STORY-511: coverage for the forge-aware change-request (PR/MR) linkage
/// formatter that `aida show`'s git-linkage section renders. Fully
/// isolated — no git repo, no `gh`/`glab` on PATH, no I/O — the rendering
/// is a pure function of (ForgeKind, ChangeLinkageState). EPIC-35 slice 5.
/// trace:STORY-511 | ai:claude
#[cfg(test)]
#[path = "tests/change_linkage_format_tests.rs"]
mod change_linkage_format_tests;

/// TASK-1475 (CR-8 acceptance 6 follow-up): coverage for `filing_drift_hint`
/// — the filing-provenance staleness signal `aida show` / `aida why`
/// render. Pure git, no `gh`/`glab` on PATH.
// trace:TASK-1475 | ai:claude
#[cfg(test)]
#[path = "tests/task_1475_filing_drift_tests.rs"]
mod task_1475_filing_drift_tests;

/// TASK-238: coverage for the queue-list tag surfacing — the inline
/// chip formatter and the `--by-batch` group key. trace:TASK-238
#[cfg(test)]
#[path = "tests/queue_tag_tests.rs"]
mod queue_tag_tests;

/// TASK-107: regression coverage for `aida fetch`. The behaviors we
/// pin: (1) code-leg fetch updates origin/<branch> without touching the
/// worktree, (2) cache-invalidation hook fires after the store leg
/// (best-effort, off the real `~/.aida` since `AIDA_HOME` overrides it
/// for tests), (3) `--code-only` / `--store-only` route correctly,
/// (4) the auto-bump does NOT fire on fetch — that's a pull-only
/// behavior. trace:TASK-107 | ai:claude
#[cfg(test)]
#[path = "tests/handle_fetch_command_tests.rs"]
mod handle_fetch_command_tests;

#[cfg(test)]
#[path = "tests/derive_parent_epic_label_tests.rs"]
mod derive_parent_epic_label_tests;

#[cfg(test)]
#[path = "tests/format_review_story_display_tests.rs"]
mod format_review_story_display_tests;

#[cfg(test)]
#[path = "tests/auto_branch_tests.rs"]
mod auto_branch_tests;

#[cfg(test)]
#[path = "tests/worktree_dirty_entries_tests.rs"]
mod worktree_dirty_entries_tests;

#[cfg(test)]
#[path = "tests/add_aida_gitignore_entries_tests.rs"]
mod add_aida_gitignore_entries_tests;

#[cfg(test)]
#[path = "tests/recent_files_for_branch_tests.rs"]
mod recent_files_for_branch_tests;

#[cfg(test)]
#[path = "tests/branch_behind_main_tests.rs"]
mod branch_behind_main_tests;

#[cfg(all(test, unix))]
mod resolve_gh_binary_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    // Serialize PATH-mutating tests against each other. Prepending (vs
    // replacing) PATH means other parallel tests keep finding `git` and
    // friends, but two concurrent PATH mutators still need to serialize
    // so they don't restore the wrong predecessor value. BUG-697: use the ONE
    // shared env lock so a PATH swap can't race any other env read. trace:BUG-79

    /// Acquire the PATH lock and prepend `dir` to PATH for the test's
    /// duration. Returns an RAII guard that restores PATH on drop.
    /// Prepending (instead of overwriting) keeps system `git` reachable
    /// from any parallel test that spawns subprocesses.
    // trace:TASK-1532 | ai:agy
    fn scoped_prepend_path(dir: &std::path::Path) -> impl Drop {
        crate::test_env::EnvVarGuard::prepend_path(dir)
    }

    fn make_executable(path: &std::path::Path) {
        crate::test_exec::write_executable(path, "#!/bin/sh\necho gh fake\n");
    }

    fn make_non_executable(path: &std::path::Path) {
        std::fs::write(path, "not executable\n").unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(path, perms).unwrap();
    }

    /// PATH walk picks up gh even when the system gh isn't on PATH.
    /// Critical regression guard for BUG-74. trace:BUG-74 | ai:claude
    #[test]
    fn finds_gh_via_path() {
        let tmp = TempDir::new().unwrap();
        let gh = tmp.path().join("gh");
        make_executable(&gh);
        let _g = scoped_prepend_path(tmp.path());
        let resolved = resolve_gh_binary().expect("expected to find fake gh on PATH");
        assert_eq!(
            std::fs::canonicalize(&resolved).unwrap(),
            std::fs::canonicalize(&gh).unwrap()
        );
    }

    /// PATH walk ignores non-executable files named gh. trace:BUG-74
    #[test]
    fn rejects_non_executable() {
        let tmp = TempDir::new().unwrap();
        let gh = tmp.path().join("gh");
        make_non_executable(&gh);
        let _g = scoped_prepend_path(tmp.path());
        let resolved = resolve_gh_binary();
        drop(_g);
        // We may still pick up a real system gh via the absolute-path
        // fallback — but we should NOT have picked up our broken fake.
        if let Some(p) = resolved {
            assert_ne!(
                std::fs::canonicalize(&p).unwrap(),
                std::fs::canonicalize(&gh).unwrap()
            );
        }
    }

    #[test]
    fn is_executable_checks_perm_bit() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("x");
        make_executable(&path);
        assert!(is_executable(&path));
        make_non_executable(&path);
        assert!(!is_executable(&path));
    }

    #[test]
    fn is_executable_rejects_dirs() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_executable(tmp.path()));
    }

    #[test]
    fn is_executable_rejects_missing() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_executable(&tmp.path().join("nope")));
    }

    /// BUG-79: a file that passes is_executable but whose spawn fails
    /// (broken interpreter line) is not returned — we fall back to the
    /// next candidate or return None. trace:BUG-79 | ai:claude
    #[test]
    fn rejects_is_executable_but_spawn_fails() {
        let tmp = TempDir::new().unwrap();
        let bad = tmp.path().join("gh");
        // Shebang points at a non-existent interpreter, so spawn errors
        // with ENOENT even though is_executable returns true.
        crate::test_exec::write_executable(
            &bad,
            "#!/this/interpreter/does/not/exist\necho should never run\n",
        );
        assert!(is_executable(&bad));

        let _g = scoped_prepend_path(tmp.path());
        let resolved = resolve_gh_binary();
        drop(_g);
        // Either None (no other gh) or some other path — never the broken one.
        if let Some(p) = resolved {
            assert_ne!(
                std::fs::canonicalize(&p).unwrap(),
                std::fs::canonicalize(&bad).unwrap()
            );
        }
    }

    /// BUG-79: when PATH has a broken candidate followed by a working one,
    /// the working one wins. trace:BUG-79 | ai:claude
    #[test]
    fn falls_back_past_broken_to_working() {
        let broken_dir = TempDir::new().unwrap();
        let good_dir = TempDir::new().unwrap();

        let broken = broken_dir.path().join("gh");
        crate::test_exec::write_executable(&broken, "#!/this/does/not/exist\n");

        let good = good_dir.path().join("gh");
        make_executable(&good);

        // Prepend BOTH dirs so the broken one is hit first, then the good
        // one. Order matters: broken_dir wins on lookup order.
        let combined = std::path::PathBuf::from(format!(
            "{}:{}",
            broken_dir.path().display(),
            good_dir.path().display()
        ));
        let _g = scoped_prepend_path(&combined);
        let resolved = resolve_gh_binary().expect("should fall back to the working gh");
        drop(_g);
        assert_eq!(
            std::fs::canonicalize(&resolved).unwrap(),
            std::fs::canonicalize(&good).unwrap()
        );
    }
}

#[cfg(test)]
#[path = "tests/session_root_resolution_tests.rs"]
mod session_root_resolution_tests;

/// Apply the user's `--color=auto|always|never` choice to the colored
/// crate's global override. `auto` is the colored crate's default
/// behavior — it uses `isatty(stdout)` and respects `NO_COLOR`.
/// trace:FR-1-041 | ai:claude
pub(crate) fn apply_color_mode(mode: &str) {
    match mode {
        "always" => colored::control::set_override(true),
        "never" => colored::control::set_override(false),
        _ => {} // "auto" — let colored crate decide via tty detection
    }
}

/// Count queue entries in the user's queue file that pass the role filter.
/// When `role` is `Some`, returns the count of entries with `for_role`
/// matching exactly. When `role` is `None`, returns the total entry
/// count (no role filter).
///
/// Reads `<project>/.aida-store/registry/queues/<user>.yaml` directly —
/// keeps statusline off the heavier Storage::load() path so the sub-50ms
/// budget holds even when the orphan store has hundreds of objects.
/// Returns `None` if the file is missing or unreadable.
/// trace:FR-1-041 | ai:claude
/// TASK-648 (ADR-3): non-archived, non-deferred draft count from the cache,
/// read-only and fast (same SQLite-without-Cache::open pattern as the
/// statusline freshness probe — no migration, no write lock on the prompt hot
/// path). The advisor's statusline shows this as the active draft backlog to
/// triage. Returns 0 when the cache is absent or unreadable rather than
/// erroring the prompt.
// trace:BUG-1171 | ai:codex
pub(crate) fn read_draft_backlog_depth(cache_path: &std::path::Path) -> usize {
    if !cache_path.exists() {
        return 0;
    }
    let conn = match rusqlite::Connection::open_with_flags(
        cache_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let has_deferred_column = conn
        .prepare("SELECT deferred FROM requirements_cache LIMIT 0")
        .is_ok();
    let sql = if has_deferred_column {
        "SELECT COUNT(*) FROM requirements_cache \
         WHERE archived = 0 AND LOWER(status) = 'draft' AND deferred = 0"
    } else {
        "SELECT COUNT(*) FROM requirements_cache \
         WHERE archived = 0 AND LOWER(status) = 'draft'"
    };
    conn.query_row(sql, [], |row| row.get::<_, i64>(0))
        .map(|n| n.max(0) as usize)
        .unwrap_or(0)
}

/// Count URGENT unread mailbox messages for the current shell user, for the
/// statusline nag. Returns `None` when there is no mailbox to count (stay
/// silent), `Some(0)` when caught up.
///
/// This MUST agree with `aida mailbox inbox` — the statusline count and the read
/// command are two surfaces of one "unread" state, so reading the inbox has to
/// drive the count to 0. The divergence that left a high count stuck forever was
/// that this read only the LOCAL `.aida/mailbox/` layer while `aida mailbox
/// inbox` reads the MERGED (local + canonical orphan-store) set and advances each
/// identity's watermark to the MERGED per-identity newest. When the two message
/// sets differed, the watermark the inbox-read advanced could not clear a count
/// computed over a different set. The fix reads the SAME merged set and the same
/// identity union, so the surfaces compute "unread" over identical inputs and a
/// read clears the count. Reading the canonical layer is a plain directory read
/// of the already-checked-out `.aida-store/mailbox/` — pure file I/O, no git
/// spawn — so the statusline's no-git contract still holds.
// trace:STORY-539 trace:BUG-625 | ai:claude
pub(crate) fn read_urgent_unread_count(project_root: &std::path::Path) -> Option<usize> {
    let local = mailbox_store::read_local_messages(project_root).ok()?;
    // Merge in the canonical (orphan-store) layer so this sees exactly what
    // `aida mailbox inbox` reads (and advances watermarks against). Best-effort:
    // a missing/unreadable canonical dir degrades to the local layer rather than
    // silencing the nag. trace:BUG-625
    let store_root = project_root.join(".aida-store");
    let canonical = mailbox_store::read_canonical_messages(&store_root).unwrap_or_default();
    let merged = aida_core::mailbox::merge_dedup(&local, &canonical);
    if merged.is_empty() {
        return None;
    }
    // Scope to the same identity union the agent-facing notice and the inbox-read
    // use (shell user + session role + agent type), so a role-/type-addressed
    // urgent message isn't invisible here while the notice surfaces it.
    // build_notice dedups a broadcast across identities and counts urgent across
    // the unread set. trace:STORY-585 trace:BUG-625
    let identities = inbox_identities();
    let watermarks = mailbox_store::read_all_watermarks(project_root).ok()?;
    let summary = aida_core::mailbox::build_notice(
        identities.iter().map(String::as_str),
        &merged,
        &watermarks,
        aida_core::mailbox::NOTICE_DEFAULT_CAP,
    );
    Some(summary.urgent)
}

// ─── STORY-127: Scope-B runtime anti-pattern detectors ──────────────────
//
// Each surface the user TYPES (`aida pull`, `git pull` (via the code leg),
// `aida dev release`, `aida session end`) detects when its precondition is
// not met and emits a one-line WARNING (never a block — the user/agent
// always proceeds). The detection is a single git/queue/lease query.
//
// The detection LOGIC is a set of pure, unit-tested predicates so the
// warning conditions are testable without process/git/forge I/O. The call
// sites do the cheap query and feed the result to the predicate.
// trace:STORY-127 | ai:claude

/// Detector (1) + (2): `aida pull` / `git pull` (code leg) no-op.
///
/// `pull_was_noop` — the code-leg `git pull --ff-only` advanced nothing
/// (local branch already at origin). When that coincides with the user
/// holding an active **reviewer** lease on an **unmerged** PR, the no-op is
/// almost certainly the PR-27-style mistake: running catch-up commands
/// BEFORE the merge has happened. Warn so the user waits for the merge.
///
/// When there is no such reviewer lease the no-op is unremarkable — a plain
/// one-line note still helps ("Already up to date — nothing to pull") but is
/// not the same alarm. The two callers distinguish via the second arg.
/// trace:STORY-127 | ai:claude
pub(crate) fn pull_noop_warning(
    pull_was_noop: bool,
    reviewer_unmerged_pr: Option<&str>,
) -> Option<String> {
    if !pull_was_noop {
        return None;
    }
    match reviewer_unmerged_pr {
        Some(pr) => Some(format!(
            "No new commits on origin. You hold a reviewer lease on {pr}, which is \
             not merged yet — did you mean to wait for {pr} to merge first? \
             (catch-up before the merge is a no-op)"
        )),
        None => Some("Already up to date — origin had no new commits.".to_string()),
    }
}

/// Detector (2): cheap "local main already at origin/main" check, expressed
/// as a pure predicate over the two SHAs so it is testable. `None` for
/// either SHA (couldn't resolve a ref) means "can't tell" → no warning.
/// trace:STORY-127 | ai:claude
pub(crate) fn local_main_already_at_origin(
    local_sha: Option<&str>,
    origin_sha: Option<&str>,
) -> bool {
    match (local_sha, origin_sha) {
        (Some(a), Some(b)) => !a.is_empty() && a == b,
        _ => false,
    }
}

/// Detector (3): `aida dev release` (→ `scripts/release.sh`) with unmerged
/// PRs. Returns a one-line warning naming every open PR — a release tag cut
/// now will NOT include their changes. Pure over the already-collected
/// open-PR numbers. trace:STORY-127 | ai:claude
pub(crate) fn release_unmerged_pr_warning(open_pr_numbers: &[u64]) -> Option<String> {
    if open_pr_numbers.is_empty() {
        return None;
    }
    let mut nums: Vec<u64> = open_pr_numbers.to_vec();
    nums.sort_unstable();
    nums.dedup();
    let list = nums
        .iter()
        .map(|n| format!("PR-{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    let (verb, subj) = if nums.len() == 1 {
        ("is", "PR")
    } else {
        ("are", "PRs")
    };
    Some(format!(
        "{} open {} ({}) — the release {} not merged, so the tag will NOT include \
         their changes. Merge first if they belong in this release.",
        nums.len(),
        subj,
        list,
        verb,
    ))
}

/// Detector (4): `aida session end` while a DIFFERENT role has queued work
/// waiting. `ending_role` is the role of the lease being torn down;
/// `role_counts` is the per-role queue depth for the current user. Returns
/// the `(role, count)` pairs for every OTHER role with at least one waiting
/// item, sorted by role name for stable output. Pure over the inputs.
/// trace:STORY-127 | ai:claude
pub(crate) fn cross_role_queue_waiting(
    ending_role: Option<&str>,
    role_counts: &std::collections::HashMap<String, usize>,
) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = role_counts
        .iter()
        .filter(|(role, &count)| {
            count > 0 && ending_role.map(|er| er != role.as_str()).unwrap_or(true)
        })
        .map(|(role, &count)| (role.clone(), count))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Render the detector-(4) warning body from the cross-role pairs. `None`
/// when nothing is waiting elsewhere. trace:STORY-127 | ai:claude
pub(crate) fn session_end_cross_role_warning(waiting: &[(String, usize)]) -> Option<String> {
    if waiting.is_empty() {
        return None;
    }
    let list = waiting
        .iter()
        .map(|(role, count)| {
            format!(
                "{} has {} item{}",
                role,
                count,
                if *count == 1 { "" } else { "s" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "About to end this session, but another role's queue has work waiting \
         ({list}). Switch role first (`aida role enter <role>`) if you meant to \
         pick that up."
    ))
}

/// STORY-127: per-role queue depth for the current user. Reads the same
/// queue YAML `aida queue list` / `read_queue_depth` consult (keyed off
/// `current_user_id`). Returns an empty map on any read/parse failure —
/// the detectors degrade to silent. trace:STORY-127 | ai:claude
pub(crate) fn read_queue_role_counts(
    project_root: &std::path::Path,
) -> std::collections::HashMap<String, usize> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let user = current_user_id(None);
    let queue_path = project_root
        .join(".aida-store/registry/queues")
        .join(format!("{}.yaml", user));
    let Ok(content) = std::fs::read_to_string(&queue_path) else {
        return counts;
    };
    let Ok(entries) = serde_yaml::from_str::<Vec<serde_yaml::Value>>(&content) else {
        return counts;
    };
    for e in &entries {
        let role = e
            .get("for_role")
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or("implementer");
        *counts.entry(role.to_string()).or_insert(0) += 1;
    }
    counts
}

/// STORY-127: does the current user hold an active **reviewer** lease whose
/// scope is a PR (`PR-N` / `MR-N`) that is still open (unmerged)? Returns
/// the PR label for the warning, else `None`. Cheap: walks the local lease
/// set, then a single forge open-PR snapshot keyed by number. The PR is
/// "unmerged" iff its number appears in the open-PR snapshot.
/// trace:STORY-127 | ai:claude
pub(crate) fn active_reviewer_unmerged_pr(project_root: &std::path::Path) -> Option<String> {
    let leases = list_leases(project_root);
    // Reviewer leases scope to a PR/MR; the scope string is e.g. "PR-27".
    let reviewer_pr_scopes: Vec<String> = leases
        .iter()
        .filter(|l| {
            l.role
                .as_deref()
                .map(|r| canonical_role_name(r) == "reviewer")
                .unwrap_or(false)
        })
        .filter(|l| pr_number_from_scope(&l.scope).is_some())
        .map(|l| l.scope.clone())
        .collect();
    if reviewer_pr_scopes.is_empty() {
        return None;
    }
    let open_numbers: std::collections::HashSet<u64> = collect_open_prs(project_root)
        .by_branch
        .values()
        .map(|p| p.number)
        .collect();
    for scope in reviewer_pr_scopes {
        if let Some(n) = pr_number_from_scope(&scope) {
            if open_numbers.contains(&n) {
                return Some(format!("PR-{n}"));
            }
        }
    }
    None
}

/// Parse a PR/MR number out of a lease scope like `PR-27` / `MR-3` / `pr-27`.
/// trace:STORY-127 | ai:claude
pub(crate) fn pr_number_from_scope(scope: &str) -> Option<u64> {
    let s = scope.trim();
    let rest = s
        .strip_prefix("PR-")
        .or_else(|| s.strip_prefix("pr-"))
        .or_else(|| s.strip_prefix("MR-"))
        .or_else(|| s.strip_prefix("mr-"))?;
    rest.parse::<u64>().ok()
}

pub(crate) fn read_queue_depth(
    project_root: &std::path::Path,
    role: Option<&str>,
) -> Option<usize> {
    // BUG-89 + BUG-675: resolve identity through the SAME two-step path
    // `aida queue list` uses — `current_user_id` first, then the queue-file
    // case-fold (`resolve_queue_user`, TASK-951/TASK-845) — so a shell reporting
    // `Joe` counts the queue keyed under `joe.yaml` instead of reading zero.
    // Reading `<current_user_id>.yaml` directly skipped that fold, so the depth
    // diverged from the queue-list count on a case-only identity mismatch; the
    // BUG-670 band-aid then suppressed the divergent number instead of fixing the
    // resolution. trace:BUG-675 trace:BUG-89 | ai:claude
    let store_path = project_root.join(".aida-store");
    let user = aida_core::db::resolve_queue_user(&store_path, &current_user_id(None));

    let queue_path = store_path
        .join("registry/queues")
        .join(format!("{}.yaml", user));

    // An absent own-queue file is zero own items, NOT "unknown" — a peer may
    // still have routed work to this role, and bailing here would hide it.
    // A file that exists but won't parse stays `None` (a real read failure).
    // trace:BUG-774 | ai:claude
    let entries: Vec<serde_yaml::Value> = match std::fs::read_to_string(&queue_path) {
        Ok(content) if content.trim().is_empty() => Vec::new(),
        Ok(content) => serde_yaml::from_str(&content).ok()?,
        Err(_) => Vec::new(),
    };

    let count = match role {
        Some(want) => entries
            .iter()
            .filter(|e| {
                e.get("for_role")
                    .and_then(serde_yaml::Value::as_str)
                    .map(|r| r == want)
                    .unwrap_or(false)
            })
            .count(),
        None => entries.len(),
    };
    // A `--for <role>` routing written by a PEER lands in that peer's queue
    // file, so a depth read scoped to our own file under-counted the work
    // actually routed to us — the meter disagreed with `aida queue list`.
    // Add the sibling files' entries routed to this role. Read-only; the
    // stored keys are untouched and identity folds through the shared
    // canonical helper. trace:BUG-774 | ai:claude
    let cross_user = match role {
        Some(want) => cross_user_role_routed_depth(&store_path, &user, want),
        None => 0,
    };
    Some(count + cross_user)
}

/// Count entries routed to `role` that live in OTHER users' queue files.
/// Best-effort + allocation-light (the same raw-YAML read the own-file depth
/// uses); an unreadable sibling file contributes zero rather than failing the
/// meter.
// trace:BUG-774 | ai:claude
pub(crate) fn cross_user_role_routed_depth(
    store_path: &std::path::Path,
    self_user: &str,
    role: &str,
) -> usize {
    let dir = store_path.join("registry/queues");
    let Ok(read) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let me = aida_core::node::canonical_user_id(self_user);
    let mut total = 0usize;
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if aida_core::node::canonical_user_id(stem) == me {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(rows) = serde_yaml::from_str::<Vec<serde_yaml::Value>>(&content) else {
            continue;
        };
        total += rows
            .iter()
            .filter(|e| {
                e.get("for_role")
                    .and_then(serde_yaml::Value::as_str)
                    .map(|r| r == role)
                    .unwrap_or(false)
            })
            .count();
    }
    total
}

/// TASK-244: read `[statusline] role_mismatch_warning` from
/// `.aida/config.toml`. Defaults to `true` (warn on mismatch) when the
/// key, section, or file is absent. trace:TASK-244 | ai:claude
pub(crate) fn statusline_role_mismatch_enabled(project_dir: &std::path::Path) -> bool {
    let Ok(content) = std::fs::read_to_string(project_dir.join(".aida").join("config.toml")) else {
        return true;
    };
    let mut in_section = false;
    for raw in content.lines() {
        let line = strip_toml_inline_comment(raw).trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_section = rest.trim_end_matches(']').trim() == "statusline";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(rest) = line.strip_prefix("role_mismatch_warning") {
            if let Some(val) = rest.split('=').nth(1) {
                let v = val
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_ascii_lowercase();
                return !matches!(v.as_str(), "false" | "0" | "no" | "off");
            }
        }
    }
    true
}

/// Read `[statusline] base_freshness_check` (on/off) and
/// `threshold_warn` (commits behind `origin/main` at/above which the
/// statusline surfaces the `base behind by N` indicator) from
/// `.aida/config.toml`. Defaults to enabled + [`BASE_BEHIND_STATUSLINE_THRESHOLD`]
/// when the keys, section, or file are absent — so the feature is on by
/// default and a project can dial it down or off without a code change.
/// Pure over the file contents; best-effort (any parse miss keeps the
/// default).
// trace:TASK-101 | ai:claude
pub(crate) fn statusline_base_freshness_config(project_dir: &std::path::Path) -> (bool, u32) {
    let default = (true, BASE_BEHIND_STATUSLINE_THRESHOLD);
    let Ok(content) = std::fs::read_to_string(project_dir.join(".aida").join("config.toml")) else {
        return default;
    };
    let (mut enabled, mut threshold) = default;
    let mut in_section = false;
    for raw in content.lines() {
        let line = strip_toml_inline_comment(raw).trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_section = rest.trim_end_matches(']').trim() == "statusline";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(rest) = line.strip_prefix("base_freshness_check") {
            if let Some(val) = rest.split('=').nth(1) {
                let v = val
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_ascii_lowercase();
                enabled = !matches!(v.as_str(), "false" | "0" | "no" | "off");
            }
        } else if let Some(rest) = line.strip_prefix("threshold_warn") {
            if let Some(val) = rest.split('=').nth(1) {
                if let Ok(n) = val
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .parse::<u32>()
                {
                    threshold = n.max(1);
                }
            }
        }
    }
    (enabled, threshold)
}

/// TASK-647 (ADR-3): a status whose *production* requires advisor authority —
/// anything that places a spec into the active execution pipeline (Approved
/// and beyond). Draft / Rejected / NeedsAttention don't gate (they are not
/// "approved work" a non-advisor could smuggle past triage).
pub(crate) fn status_requires_advisor_authority(status: &RequirementStatus) -> bool {
    // TASK-739: single-sourced in the lifecycle model — the target half of the
    // advisor-authority predicate. trace:TASK-739 | ai:claude
    aida_core::lifecycle::target_requires_advisor_authority(
        aida_core::lifecycle::State::from_status(status),
    )
}

/// BUG-482: whether advancing a spec from `from` to `to` (via `aida edit
/// --status`) is an advisor-authority act keyed on the (source, target) pair.
///
/// A non-advisor may not lift an **un-triaged** (`Draft`) *or* a **punted**
/// (`NeedsAttention`) spec into the approved+ pipeline — both sources sit
/// awaiting the advisor's (or interactive human's) triage decision.
/// `forbidden_attention_transition` *permits* `NeedsAttention → Approved` as a
/// triage outcome, but *who* may make that call is still advisor authority;
/// before BUG-482 only `Draft` was a gated source, so a non-advisor could
/// self-re-approve a spec it (or the orchestrator) had just punted, bypassing
/// the triage the punt exists to request. Execution flips from a source
/// already in the pipeline (`Approved → InProgress → Done`) are NOT gated, so
/// drains are unaffected. trace:BUG-482 | ai:claude
pub(crate) fn status_advance_requires_advisor_authority(
    from: &RequirementStatus,
    to: &RequirementStatus,
) -> bool {
    // TASK-739: delegate to the lifecycle model's transition guard — the single
    // source for which (from → to) edits are advisor-authority acts. Defined
    // over the full domain (direct edits too, not only declared edges).
    // trace:TASK-739 | ai:claude
    use aida_core::lifecycle::{transition_guard, GuardKind, State};
    transition_guard(State::from_status(from), State::from_status(to))
        == GuardKind::RequiresAdvisorAuthority
}

/// BUG-1611: why `aida queue done` refuses a spec on lifecycle grounds.
/// `queue done` is an execution flip: its legal predecessors are the states a
/// spec reaches only after passing the approval gate (`Approved`, `Planned`,
/// `InProgress`, and an idempotent `Done`). Anything else is refused through
/// the existing lifecycle guard rather than a new rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueDoneLifecycleRefusal {
    /// `Draft` / `NeedsAttention`: never approved, or punted back for triage.
    /// The lifecycle guard marks `→ Done` from these as an advisor-authority
    /// act, and the caller does not hold that authority.
    NotApproved,
    /// `Rejected` / `Completed` / `Superseded`: closed. `queue done` is not a
    /// reopen verb, so it refuses whatever the caller's authority.
    Closed,
}

/// BUG-1611: the lifecycle/authority decision for `aida queue done`, shared by
/// the CLI handler and the MCP `queue_done` tool so the two surfaces cannot
/// drift. `has_advisor_authority` is the caller's resolved authority (CLI:
/// [`has_advisor_authority`]; MCP: the server's role). `None` = proceed.
/// `--force` deliberately has no say here.
// trace:BUG-1611 | ai:claude
pub(crate) fn queue_done_lifecycle_refusal(
    from: &RequirementStatus,
    has_advisor_authority: bool,
) -> Option<QueueDoneLifecycleRefusal> {
    if is_terminal_status(from) {
        return Some(QueueDoneLifecycleRefusal::Closed);
    }
    if status_advance_requires_advisor_authority(from, &RequirementStatus::Done)
        && !has_advisor_authority
    {
        return Some(QueueDoneLifecycleRefusal::NotApproved);
    }
    None
}

/// BUG-1611: the operator-facing refusal line for
/// [`queue_done_lifecycle_refusal`]. Neutral guidance only: it names the
/// legitimate route (approval, or a reopen that is itself authority-gated) and
/// never suggests an environment-variable role override.
// trace:BUG-1611 | ai:claude
pub(crate) fn queue_done_lifecycle_refusal_message(
    display_id: &str,
    from: &RequirementStatus,
    refusal: QueueDoneLifecycleRefusal,
) -> String {
    match refusal {
        QueueDoneLifecycleRefusal::NotApproved => format!(
            "queue done refused: {display_id} is {from} and has not passed the approval \
             gate, so marking it done would skip triage. Ask an advisor to approve it \
             (`aida edit {display_id} --status approved`), then work it and mark it done."
        ),
        QueueDoneLifecycleRefusal::Closed => format!(
            "queue done refused: {display_id} is already {from} (closed), and marking it \
             done would reopen it. To redo the work, have an advisor reopen it \
             (`aida edit {display_id} --status approved --force`) or file a new spec."
        ),
    }
}

/// BUG-1611: whether `aida zen`'s autopilot approve-gate may flip this spec to
/// `Approved`. `[autopilot] approve = "auto"` is a policy knob in an
/// agent-writable config file, so it cannot stand in for approval authority:
/// the auto-approve runs the same (source → Approved) lifecycle guard every
/// other approval path runs.
// trace:BUG-1611 | ai:claude
pub(crate) fn zen_auto_approve_authorized(
    from: &RequirementStatus,
    has_advisor_authority: bool,
) -> bool {
    !status_advance_requires_advisor_authority(from, &RequirementStatus::Approved)
        || has_advisor_authority
}

/// A manual `aida edit <epic> --status <X>` is forbidden — an epic's status is a
/// read-only rollup of its children. This generalizes the earlier "epics cannot
/// be promoted to Approved" rule to EVERY transition; `--force` is the recovery
/// escape.
// trace:BUG-626 trace:TASK-761 | ai:claude
pub(crate) fn manual_epic_status_edit_forbidden(req_type: &RequirementType, force: bool) -> bool {
    *req_type == RequirementType::Epic && !force
}

pub(crate) fn approval_forbidden_for_type(req_type: &RequirementType) -> bool {
    // trace:TASK-761 | ai:codex
    //
    // BUG-751: `Decision` is deliberately NOT in this set. A decision spec
    // (ADR) is stateful — its documented lifecycle is proposed / accepted /
    // superseded / deprecated — and `Approved` is the sanctioned "accepted"
    // state (Draft == proposed). Gating it left an ADR with no way out of
    // Draft, while existing ADRs already sat at Approved by convention.
    // trace:BUG-751 | ai:claude
    matches!(
        req_type,
        RequirementType::Vision
            | RequirementType::Epic
            | RequirementType::Principle
            | RequirementType::Constraint
            | RequirementType::Term
    )
}

// BUG-751: type-aware wrapper over `validate_status_input`. The ADR lifecycle
// verb `accepted` is not a status in the enum; for decision-class specs it is
// an input alias for `Approved` (approved == accepted for ADRs), so a recorded
// acceptance can be applied with either verb. For every other type — and every
// other input — this delegates to the canonical validator, except that a
// non-decision given `accepted` gets a refusal naming the correct verb.
// trace:BUG-751 | ai:claude
pub(crate) fn validate_status_input_for_type(
    raw: &str,
    req_type: &RequirementType,
) -> Result<&'static str, String> {
    if raw.trim().eq_ignore_ascii_case("accepted") {
        if *req_type == RequirementType::Decision {
            return Ok("Approved");
        }
        return Err(format!(
            "invalid status `{}` — `accepted` applies to decision specs (where it \
             records as `approved`); expected one of: {}",
            raw, VALID_STATUS_INPUTS
        ));
    }
    validate_status_input(raw)
}

/// TASK-130: resolve the `human_only` marker for a freshly-added spec from its
/// type plus the explicit `--human-only` / `--no-human-only` flags.
///
/// A Spike is, by definition, time-boxed research driven by operator judgment
/// (candidate selection, experimental design, rubric evaluation, report
/// writing) — not heads-down implementation. So a Spike defaults to
/// `human_only = true`, which the orchestrator's pre-pickup gate honors by
/// skipping it rather than handing it to an implementer agent that would have
/// to bail or fabricate work. Every other type defaults to `false`.
///
/// The flags always win, and they are mutually exclusive at the CLI layer
/// (`conflicts_with`), so at most one is set:
/// - `--human-only`     → `true`  (opt any type in)
/// - `--no-human-only`  → `false` (opt a Spike back out into auto-pickup)
/// - neither            → the per-type default above
///
/// trace:TASK-130 | ai:claude
pub(crate) fn resolve_human_only(
    req_type: &RequirementType,
    human_only: bool,
    no_human_only: bool,
) -> bool {
    if human_only {
        return true;
    }
    if no_human_only {
        return false;
    }
    matches!(req_type, RequirementType::Spike)
}

/// TASK-754: why `aida add --queue` would refuse to enqueue the freshly-filed
/// spec. Pure decision over the new spec's *final* status (post-intake-gate)
/// plus whether the intake gate downgraded the requested Approved to Draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueAtFilingRefusal {
    /// The advisor-authority gate downgraded Approved → Draft (AC5): a
    /// non-advisor / non-TTY session filed but cannot queue — same hard gate
    /// `aida queue add` enforces.
    Downgraded,
    /// The spec's status is simply not enqueueable (AC2): only Approved is
    /// groomable, matching `aida backlog groom`'s policy.
    NotApproved,
}

// trace:TASK-754 trace:BUG-631 | ai:claude
/// `None` ⇒ enqueue is allowed. `Some(reason)` ⇒ refuse with the matching
/// message. AC2 + AC5 collapse to one rule: the new spec must end up Approved,
/// and a downgrade (the authority gate having fired) is reported distinctly so
/// the operator knows to re-run as the advisor rather than just fix the status.
///
/// The `route` (the `--for` target, before defaulting) carves out the
/// request-for-review case. Routing a freshly-filed draft `--for advisor`
/// (or `--for human`/`--for reviewer`) is a REQUEST for review/triage, open to
/// any role — so it is never refused here regardless of status, mirroring the
/// standalone `aida queue add --for advisor`, which enqueues any non-terminal
/// status. Only execution-dispatch routes carry the Approved-only / authority
/// gate.
pub(crate) fn queue_at_filing_refusal(
    final_status: &RequirementStatus,
    intake_downgraded: bool,
    route: Option<&str>,
) -> Option<QueueAtFilingRefusal> {
    // BUG-631: a request-for-review route bypasses the dispatch gate entirely.
    if !for_target_requires_dispatch_authority(route) {
        return None;
    }
    if matches!(final_status, RequirementStatus::Approved) {
        return None;
    }
    if intake_downgraded {
        Some(QueueAtFilingRefusal::Downgraded)
    } else {
        Some(QueueAtFilingRefusal::NotApproved)
    }
}

/// Link the parent checkout's runtime AIDA state into a new worktree
/// (BUG-52). `.aida-store/` is gitignored so a whole-directory symlink works.
/// `.aida/` is partially tracked, so only its gitignored runtime entries are
/// linked. `cache.db.lock-info` is deliberately NOT linked (BUG-1644).
// trace:BUG-52 trace:BUG-1644 | ai:claude
#[cfg(unix)]
pub(crate) fn link_worktree_runtime_state(
    project_root: &std::path::Path,
    worktree_path: &std::path::Path,
) -> Result<()> {
    let store_src = project_root.join(".aida-store");
    let store_dst = worktree_path.join(".aida-store");
    if store_src.exists() && !store_dst.exists() {
        std::os::unix::fs::symlink(&store_src, &store_dst).with_context(|| {
            format!(
                "symlink {} -> {} failed",
                store_dst.display(),
                store_src.display()
            )
        })?;
    }

    let parent_aida = project_root.join(".aida");
    let worktree_aida = worktree_path.join(".aida");
    if parent_aida.exists() {
        std::fs::create_dir_all(&worktree_aida)?;
        for runtime in &[
            "sessions",
            "agents",
            "roles",
            "cache.db",
            "cache.db-shm",
            "cache.db-wal",
            "pgdata",
        ] {
            let src = parent_aida.join(runtime);
            let dst = worktree_aida.join(runtime);
            if src.exists() && !dst.exists() {
                std::os::unix::fs::symlink(&src, &dst).with_context(|| {
                    format!("symlink {} -> {} failed", dst.display(), src.display())
                })?;
            }
        }
        // BUG-1644: `cache.db.lock-info` is deliberately NOT linked. Its
        // path derives from the shared (symlink-resolved) cache location
        // (`aida_core::cache_lock_info_path`), so a reader here and a
        // writer in the main checkout already meet at one sidecar, and a
        // not-yet-created parent cache needs no dangling link. A reused
        // worktree may still hold an older binary's per-worktree sidecar;
        // retire it when its owner is dead. trace:BUG-1644 | ai:claude
        retire_stray_worktree_lock_info(&worktree_aida);
    }
    Ok(())
}

/// Strip the runtime symlinks `link_worktree_runtime_state` created inside a
/// worktree's `.aida/` (BUG-52), so `git worktree remove` doesn't count them
/// as untracked files. `.aida/` itself holds tracked content and stays.
// trace:BUG-52 trace:BUG-1644 | ai:claude
pub(crate) fn unlink_worktree_aida_runtime(aida_dir: &std::path::Path) {
    // BUG-1644: retire an older binary's per-worktree lock-info sidecar
    // (dead owner only) while `cache.db` is still a symlink, i.e. while
    // it is still distinguishable from the shared sidecar.
    // trace:BUG-1644 | ai:claude
    retire_stray_worktree_lock_info(aida_dir);
    for runtime in &[
        "sessions",
        "roles",
        "cache.db",
        "cache.db-shm",
        "cache.db-wal",
        "pgdata",
    ] {
        let p = aida_dir.join(runtime);
        if p.is_symlink() {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// Remove a per-worktree `cache.db.lock-info` that a pre-BUG-1644 binary left
/// beside the worktree's SYMLINKED `cache.db`, but only when its recorded owner
/// is provably dead (the TASK-1484 compare-and-delete). A live owner's file is
/// left for `aida doctor` to report. No-op when `cache.db` is not a symlink:
/// the sidecar there IS the shared one.
// trace:BUG-1644 | ai:claude
pub(crate) fn retire_stray_worktree_lock_info(worktree_aida: &std::path::Path) {
    if let Some(stray) = aida_core::stray_cache_lock_info_path(&worktree_aida.join("cache.db")) {
        let _ = aida_core::reclaim_dead_lock_info(&stray);
    }
}

/// BUG-528: resolve which role queue `aida add --queue` routes the freshly-filed
/// spec to. Mirrors `aida queue add --for`'s `--for any` semantic, but the
/// *default* (no `--for`) is the `implementer` queue — the overwhelmingly common
/// target for filed work — rather than the filer's own session role. Before this,
/// `enqueue_groomed` routed by `AIDA_SESSION_ROLE`, so an advisor filing
/// implementation work with `--queue` silently landed it in the advisor queue,
/// where `burndown run` (which drains the implementer queue) would miss it.
///   * `Some("any")`  → `None` (unrouted, explicit opt-out).
///   * `Some(role)`   → `Some(canonical_role_name(role))`.
///   * `None`         → `Some("implementer")` (canonicalized).
/// trace:BUG-528 | ai:claude
pub(crate) fn add_queue_route_role(r#for: Option<&str>) -> Option<String> {
    match r#for {
        Some("any") => None,
        Some(role) => Some(canonical_role_name(role)),
        None => Some(canonical_role_name("implementer")),
    }
}

// BUG-631: is a `--for <role>` queue route a REQUEST-for-review/triage route
// (open to any role) rather than a DISPATCH-for-execution route?
//
// The advisor-authority gate (TASK-647 / ADR-3) exists to stop a non-advisor
// committing the team to BUILD work. Routing a draft to the advisor / human /
// reviewer is the opposite act: a REQUEST for review or triage. It does not
// dispatch execution and does not bypass approval — the advisor/reviewer still
// decides. So those three targets are exempt from the gate; `implementer` and
// every other (execution or unknown/custom) role stay gated. The exempt set is
// the closed list of review/triage seats: widen it deliberately, never by
// default, so an unrecognized role leans toward the safer (gated) behavior.
// trace:BUG-631 | ai:claude
pub(crate) fn for_route_is_request_only(for_role: &str) -> bool {
    // `canonical_role_name` lowercases only `human`/`dialog`; other roles pass
    // through with their original casing, so compare case-insensitively to
    // accept `--for Reviewer` / `--for ADVISOR` the same as the lowercase forms.
    let canonical = canonical_role_name(for_role).to_ascii_lowercase();
    matches!(canonical.as_str(), "advisor" | "human" | "reviewer")
}

// BUG-631: does this queue-add `--for` target require dispatch authority?
//
// TRUE (gated) for dispatch-for-execution routes — `implementer`, any
// unknown/custom role, and the unrouted cases (`--for any` or no `--for` at
// all), both of which imply execution intent. FALSE (exempt) only for the
// request/review routes classified by `for_route_is_request_only`
// (advisor / human / reviewer). This scopes the TASK-647 gate to the act it was
// meant to guard — committing the team to build — and lets `aida queue add
// --for advisor` (a request for review) succeed from any role. trace:BUG-631
pub(crate) fn for_target_requires_dispatch_authority(for_role: Option<&str>) -> bool {
    match for_role {
        Some("any") => true,
        Some(role) => !for_route_is_request_only(role),
        None => true,
    }
}

// BUG-498: An operator doing advisor work (groom / approve / queue) by
// prefixing individual commands with `AIDA_SESSION_ROLE=advisor` is acting
// as the advisor, but their persistent shell seat is unset — so the
// statusline (which renders `$AIDA_SESSION_ROLE`, defaulting to `implementer`)
// shows `implementer`, and the seat-vs-work mismatch is invisible. This is the
// pure predicate behind the one-time hint suggesting they actually seat the
// role via `aida role enter advisor`.
//
// Discriminator: `aida role enter advisor` exports BOTH `AIDA_SESSION_ROLE`
// AND `AIDA_SESSION_PROJECT` (see `emit_role_enter_eval`), and persists them in
// the shell — so a *seated* advisor's every command carries both. A one-off
// `AIDA_SESSION_ROLE=advisor aida …` prefix sets only `AIDA_SESSION_ROLE` for
// that single process and leaves `AIDA_SESSION_PROJECT` unset. So: role resolves
// to advisor via the env var, but the seat (`AIDA_SESSION_PROJECT`) was never
// established → the operator is advisor-via-prefix, not advisor-seated.
// trace:BUG-498 | ai:claude
pub(crate) fn advisor_seat_hint_warranted(role: &str, session_project_set: bool) -> bool {
    role == "advisor" && !session_project_set
}

/// BUG-498: print a one-time stderr hint when an advisor-gated command is run
/// advisor-style via an `AIDA_SESSION_ROLE=advisor` prefix while the persistent
/// seat was never established (no `aida role enter advisor`). Never blocks the
/// command, never changes the role. Gated on a per-clone marker file under
/// `.aida/` so it fires at most once (the `.aida/*` deny-by-default gitignore
/// convention means the marker needs no allow-list entry). trace:BUG-498
pub(crate) fn maybe_hint_advisor_seat() {
    let role = effective_role();
    let session_project_set = std::env::var("AIDA_SESSION_PROJECT")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    if !advisor_seat_hint_warranted(&role, session_project_set) {
        return;
    }
    let Ok(project_root) = find_project_root() else {
        return;
    };
    let marker = project_root.join(".aida/.advisor-seat-hint-shown");
    if marker.exists() {
        return;
    }
    // Best-effort: ensure `.aida/` exists, then drop the marker. Failure to
    // write is non-fatal — at worst the hint shows again next time.
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&marker, "shown\n");
    eprintln!(
        "{} You're operating as {} — run {} to seat it (your statusline still shows the default role).",
        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
        "advisor".yellow(),
        "aida role enter advisor".cyan()
    );
}

/// they disagree — and the warning is enabled — both are shown with a
/// warning glyph so three-way role confusion (shell vs session vs resumed
/// conversation) is visible at a glance. Returns `(text, is_mismatch)`
/// so the caller picks the colour. Pure — unit-tested independent of
/// the statusline IO. trace:TASK-244 | ai:claude
pub(crate) fn role_segment_text(
    shell_role: &str,
    session_role: Option<&str>,
    warn_enabled: bool,
) -> (String, bool) {
    // BUG-519: `general-purpose` is the harness's generic fallback agent_type
    // (worktree_lease.rs), written onto the lease `role` for a subagent that
    // carries no AIDA role. When the operator deliberately started a
    // general-purpose session, warn-glyphing it as a role mismatch reads as
    // alarmist noise — it isn't a misrouted reviewer/implementer, it's the
    // intended generic seat. Only warn when the session role names a real,
    // scoped AIDA role that disagrees with the shell. trace:BUG-519
    let mismatch = warn_enabled
        && session_role
            .map(|s| {
                !s.eq_ignore_ascii_case(shell_role) && !s.eq_ignore_ascii_case("general-purpose")
            })
            .unwrap_or(false);
    if mismatch {
        (
            format!(
                "role:{} {} session:{}",
                shell_role,
                crate::glyph(crate::glyphs::Glyph::Warning),
                session_role.unwrap()
            ),
            true,
        )
    } else {
        (format!("role:{}", shell_role), false)
    }
}

/// TASK-306: the orchestrator-context badge for the statusline. Built for a
/// corroborated `--auto-complete` phase session; the caller colors the
/// fields. `phase` is the 1-based phase index (`AIDA_AUTO_COMPLETE_PHASE`),
/// `no_human_mode` the `--no-human` scope slug (`AIDA_NO_HUMAN_MODE`).
/// trace:TASK-306 | ai:claude
pub(crate) struct OrchestratorBadge {
    /// `auto:N/6 <phase-name>` — the phase indicator. `auto:?/6` when the
    /// phase env var is missing or unparseable (defensive — the orchestrator
    /// always sets it on the children it spawns).
    pub(crate) phase: String,
    /// `no-human:<mode>` — present only when `--no-human` is in effect.
    pub(crate) no_human: Option<String>,
    /// `pause-here` — the cue that the user is expected to act in this phase.
    /// A statusline only renders for an interactive session, so an
    /// orchestrated badge always carries it.
    pub(crate) pause: &'static str,
}

impl OrchestratorBadge {
    pub(crate) fn build(phase: Option<u8>, no_human_mode: Option<&str>) -> Self {
        let phase = match phase {
            Some(n) => match auto_complete::Phase::from_index(n as i32) {
                Some(p) => format!("auto:{n}/6 {}", p.slug()),
                None => format!("auto:{n}/6"),
            },
            None => "auto:?/6".to_string(),
        };
        OrchestratorBadge {
            phase,
            no_human: no_human_mode.map(|m| format!("no-human:{m}")),
            pause: "pause-here",
        }
    }
}

// ============================================================================
// `aida plan verify` — lint a docs/plans/ file against the structured
// template (TASK-92). Reports drifted line refs, missing files, and absent
// required sections; exits non-zero on any hard failure so it can run as a
// pre-commit hook. trace:TASK-93 | ai:claude
// ============================================================================

/// One section the structured plan template expects. `hard` sections are
/// errors when absent; the rest are warnings. Matched by case-insensitive
/// substring against the plan's `##` headers.
pub(crate) struct PlanSectionSpec {
    pub(crate) keyword: &'static str,
    /// If set, a header matching `keyword` is rejected when it also
    /// contains this string (disambiguates "Files" from "Critical Files").
    pub(crate) exclude: Option<&'static str>,
    pub(crate) label: &'static str,
    pub(crate) hard: bool,
}

pub(crate) const PLAN_SECTIONS: &[PlanSectionSpec] = &[
    PlanSectionSpec {
        keyword: "approach",
        exclude: None,
        label: "Approach",
        hard: false,
    },
    PlanSectionSpec {
        keyword: "decision",
        exclude: None,
        label: "Decisions",
        hard: false,
    },
    PlanSectionSpec {
        keyword: "files",
        exclude: Some("critical"),
        label: "Files",
        hard: false,
    },
    PlanSectionSpec {
        keyword: "critical files",
        exclude: None,
        label: "Critical Files",
        hard: true,
    },
    PlanSectionSpec {
        keyword: "reusable helper",
        exclude: None,
        label: "Reusable helpers",
        hard: false,
    },
    PlanSectionSpec {
        keyword: "risk",
        exclude: None,
        label: "Risks + gotchas",
        hard: false,
    },
    PlanSectionSpec {
        keyword: "test",
        exclude: None,
        label: "Tests",
        hard: false,
    },
    PlanSectionSpec {
        keyword: "verification",
        exclude: None,
        label: "Verification",
        hard: true,
    },
    PlanSectionSpec {
        keyword: "followup",
        exclude: None,
        label: "Followups",
        hard: true,
    },
    PlanSectionSpec {
        keyword: "related",
        exclude: None,
        label: "Related",
        hard: false,
    },
];

/// Source-file extensions a `path:line` ref or bare path is expected to use.
/// Filters out commands (`aida pull`) and prose from the path heuristics.
pub(crate) const PLAN_SOURCE_EXTS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "toml", "md", "sh", "json", "yaml", "yml", "lock", "css",
    "html",
];

// trace:TASK-772 | ai:claude — Debug for test assertions
#[derive(PartialEq, Debug)]
pub(crate) enum PlanFindingLevel {
    Ok,
    Warn,
    Error,
}

pub(crate) struct PlanFinding {
    pub(crate) level: PlanFindingLevel,
    pub(crate) msg: String,
}

/// A confirmed drifted line ref queued for `--fix` rewriting.
pub(crate) struct PlanRefFix {
    pub(crate) line_idx: usize,
    pub(crate) old: String,
    pub(crate) new: String,
}

/// The full result of linting a plan file: the three finding groups plus the
/// confirmed drifted-ref fixes. Split out from `verify_plan` so both the CLI
/// renderer (`verify_plan`) and the read-only MCP `plan_verify` tool compute
/// from the same source of truth. trace:EPIC-27 | ai:claude
pub(crate) struct PlanReport {
    pub(crate) sections: Vec<PlanFinding>,
    pub(crate) files: Vec<PlanFinding>,
    pub(crate) refs: Vec<PlanFinding>,
    pub(crate) fixes: Vec<PlanRefFix>,
}

impl PlanReport {
    pub(crate) fn error_count(&self) -> usize {
        let c = |fs: &[PlanFinding]| {
            fs.iter()
                .filter(|f| f.level == PlanFindingLevel::Error)
                .count()
        };
        c(&self.sections) + c(&self.files) + c(&self.refs)
    }

    pub(crate) fn warn_count(&self) -> usize {
        let c = |fs: &[PlanFinding]| {
            fs.iter()
                .filter(|f| f.level == PlanFindingLevel::Warn)
                .count()
        };
        c(&self.sections) + c(&self.files) + c(&self.refs)
    }
}

// trace:STORY-447 | ai:claude
/// Walk up from the plan file to the enclosing git repo root. Paths inside
/// a plan are repo-relative, so this is what we resolve them against. Falls
/// back to the current directory when no `.git` is found.
pub(crate) fn plan_repo_root(plan_file: &std::path::Path) -> std::path::PathBuf {
    let start = plan_file
        .canonicalize()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf));
    let mut dir = match start {
        Some(d) => d,
        None => return std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    };
    loop {
        if dir.join(".git").exists() {
            return dir;
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => {
                return std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            }
        }
    }
}

/// True when `tok` looks like a placeholder rather than a real path:
/// template stand-ins (`path/to/file.rs`, `<STORY-N>`) and globs (`*.md`).
pub(crate) fn is_plan_placeholder_path(tok: &str) -> bool {
    tok.contains('<')
        || tok.contains('>')
        || tok.contains('*')
        || tok.starts_with("path/to/")
        || tok.contains("...")
        || tok.contains("NNN")
}

// trace:TASK-772 | ai:claude
/// True when the text immediately following a backticked path ref marks the
/// file as one the plan will *create*: optional whitespace, then `(new)` or
/// `(to create)`, case-insensitive. A marked path that doesn't exist yet is
/// OK rather than an error; a marked path that already exists draws a
/// stale-annotation warning.
pub(crate) fn plan_path_marked_new(rest: &str) -> bool {
    let lower = rest.trim_start().to_ascii_lowercase();
    lower.starts_with("(new)") || lower.starts_with("(to create)")
}

// trace:TASK-162 | ai:codex
/// True when the text immediately following a file path in a plan heading
/// marks the file as planned-new, e.g. ``### `src/new.rs` — NEW: parser``.
pub(crate) fn plan_path_heading_marked_new(rest: &str) -> bool {
    let lower = rest.trim_start().to_ascii_lowercase();
    let lower = lower.trim_start_matches(['—', '-', ':']).trim_start();
    lower == "new"
        || lower.starts_with("new:")
        || lower.starts_with("new ")
        || lower.starts_with("new(")
        || lower.starts_with("new —")
}

/// True when `tok` (already stripped of any `:line` suffix) has a known
/// source-file extension.
pub(crate) fn has_plan_source_ext(tok: &str) -> bool {
    match tok.rsplit_once('.') {
        Some((_, ext)) => PLAN_SOURCE_EXTS.contains(&ext.to_ascii_lowercase().as_str()),
        None => false,
    }
}

/// Locate the 1-based line where `symbol` is *defined* in `content`.
/// Recognises Rust item kinds and the common TypeScript/JS forms. Returns
/// the first definition found. A `::`-qualified name is reduced to its
/// final segment before searching.
pub(crate) fn locate_symbol_line(content: &str, symbol: &str) -> Option<usize> {
    use regex::Regex;
    let bare = symbol.rsplit("::").next().unwrap_or(symbol);
    if bare.is_empty() || !bare.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let esc = regex::escape(bare);
    // Leading indent must be `[ \t]*`, never `\s*` — `\s` matches `\n`, so a
    // greedy `^\s*` would start the match on the *preceding* blank line and
    // throw the reported line number off by one.
    let rust_item = format!(
        r#"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?(?:default[ \t]+)?(?:async[ \t]+)?(?:unsafe[ \t]+)?(?:extern[ \t]+(?:"[^"]*"[ \t]+)?)?(?:fn|struct|enum|trait|type|union|mod|const|static)[ \t]+{esc}\b"#
    );
    let pat = format!(
        r"(?m){rust_item}|^[ \t]*macro_rules![ \t]+{esc}\b|^[ \t]*(?:export[ \t]+)?(?:default[ \t]+)?(?:abstract[ \t]+)?(?:async[ \t]+)?(?:function|class|interface)[ \t]+{esc}\b|^[ \t]*(?:export[ \t]+)?(?:const|let|var)[ \t]+{esc}\b"
    );
    let re = Regex::new(&pat).ok()?;
    let m = re.find(content)?;
    Some(content[..m.start()].bytes().filter(|&b| b == b'\n').count() + 1)
}

/// Pull backtick-quoted identifiers that look like code symbols off a line
/// of plan prose: `` `fn foo` `` → `foo`, `` `Storage::update` `` → kept as
/// `Storage::update` (the locator reduces it). Leading item keywords are
/// stripped. Also catches an un-backticked `fn name`.
pub(crate) fn plan_symbols_on_line(line: &str) -> Vec<String> {
    use regex::Regex;
    let mut out = Vec::new();
    let backtick = Regex::new(r"`([^`]+)`").unwrap();
    for cap in backtick.captures_iter(line) {
        let span = cap[1].trim();
        // Drop a leading item keyword: "fn foo" -> "foo".
        let ident = span
            .strip_prefix("fn ")
            .or_else(|| span.strip_prefix("struct "))
            .or_else(|| span.strip_prefix("enum "))
            .or_else(|| span.strip_prefix("trait "))
            .or_else(|| span.strip_prefix("impl "))
            .or_else(|| span.strip_prefix("type "))
            .or_else(|| span.strip_prefix("macro_rules! "))
            .unwrap_or(span)
            .trim();
        // Keep plausible identifiers (optionally `::`-qualified). Reject
        // paths, prose, commands.
        let core = ident.split('(').next().unwrap_or(ident).trim();
        if !core.is_empty()
            && !core.contains(' ')
            && !core.contains('/')
            && !core.contains('.')
            && core
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == ':')
        {
            out.push(core.to_string());
        }
    }
    let unq = Regex::new(r"\bfn\s+([a-z_][A-Za-z0-9_]*)").unwrap();
    for cap in unq.captures_iter(line) {
        out.push(cap[1].to_string());
    }
    out.sort();
    out.dedup();
    out
}

/// Pure plan-lint pass: compute the section / file / line-ref findings (and
/// the drifted-ref fixes) for `content` resolved against repo `root`. Does no
/// I/O beyond reading the source files the plan references, and never writes
/// or exits — the rendering, `--fix` rewrite, and process exit live in the
/// callers (`verify_plan` for the CLI, the `plan_verify` MCP tool for the
/// server). trace:EPIC-27 | ai:claude
pub(crate) fn compute_plan_report(content: &str, root: &std::path::Path) -> PlanReport {
    use regex::Regex;
    let lines: Vec<&str> = content.lines().collect();

    let mut section_findings: Vec<PlanFinding> = Vec::new();
    let mut file_findings: Vec<PlanFinding> = Vec::new();
    let mut ref_findings: Vec<PlanFinding> = Vec::new();
    let mut fixes: Vec<PlanRefFix> = Vec::new();

    // --- Section pass: collect `##` headers, track section membership. ---
    let mut headers_lower: Vec<String> = Vec::new();
    let mut files_section_paths: Vec<String> = Vec::new();
    let mut planned_new_paths: HashSet<String> = HashSet::new();
    let mut critical_section_paths: HashSet<String> = HashSet::new();
    let mut in_fence = false;
    let mut in_html_comment = false;
    let mut current_section = String::new();

    let path_ref_re = Regex::new(r"([A-Za-z0-9_][A-Za-z0-9_./\-]*\.[A-Za-z0-9]+):(\d+)").unwrap();
    let bare_path_re = Regex::new(r"`([A-Za-z0-9_][A-Za-z0-9_./\-]*\.[A-Za-z0-9]+)`").unwrap();
    let sym_ref_re = Regex::new(r"\b([a-z_][A-Za-z0-9_]*):(\d+)\b").unwrap();

    for line in &lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // Skip `<!-- ... -->` regions — meta-commentary, not plan content
        // (the template's own guidance cites an example bad ref there).
        if in_html_comment {
            if line.contains("-->") {
                in_html_comment = false;
            }
            continue;
        }
        if let Some(pos) = line.find("<!--") {
            if !line[pos..].contains("-->") {
                in_html_comment = true;
            }
            continue;
        }
        if let Some(h) = trimmed.strip_prefix("## ") {
            if !trimmed.starts_with("### ") {
                current_section = h.trim().to_ascii_lowercase();
                headers_lower.push(current_section.clone());
            }
        }
        if let Some(h) = trimmed.strip_prefix("### ") {
            // `### `path/to/file.rs` — purpose` names a build-order file.
            if let Some(cap) = bare_path_re.captures(h) {
                let p = cap[1].to_string();
                let rest = &h[cap.get(0).unwrap().end()..];
                if current_section.contains("files") && !current_section.contains("critical") {
                    files_section_paths.push(p.clone());
                }
                if current_section.contains("new files") || plan_path_heading_marked_new(rest) {
                    planned_new_paths.insert(p);
                }
            }
        }
        if current_section.contains("new files") {
            for cap in bare_path_re.captures_iter(line) {
                planned_new_paths.insert(cap[1].to_string());
            }
        }
        if current_section.contains("critical files") {
            for cap in bare_path_re.captures_iter(line) {
                critical_section_paths.insert(cap[1].to_string());
            }
        }
    }

    for spec in PLAN_SECTIONS {
        let present = headers_lower
            .iter()
            .any(|h| h.contains(spec.keyword) && !spec.exclude.is_some_and(|ex| h.contains(ex)));
        if present {
            section_findings.push(PlanFinding {
                level: PlanFindingLevel::Ok,
                msg: format!("{} section present", spec.label),
            });
        } else if spec.hard {
            section_findings.push(PlanFinding {
                level: PlanFindingLevel::Error,
                msg: format!("required section missing: {}", spec.label),
            });
        } else {
            section_findings.push(PlanFinding {
                level: PlanFindingLevel::Warn,
                msg: format!("recommended section missing: {}", spec.label),
            });
        }
    }

    // Critical Files completeness: every build-order file should also be
    // enumerated in the Critical Files section.
    for p in &files_section_paths {
        if !critical_section_paths.contains(p) {
            section_findings.push(PlanFinding {
                level: PlanFindingLevel::Warn,
                msg: format!("`{p}` is in Files but not enumerated in Critical Files"),
            });
        }
    }

    // --- File existence + line-ref pass (prose only, skip code fences
    // and HTML comment regions). ---
    let mut checked_paths: HashSet<String> = HashSet::new();
    let mut current_h3_file: Option<String> = None;
    in_fence = false;
    in_html_comment = false;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if in_html_comment {
            if line.contains("-->") {
                in_html_comment = false;
            }
            continue;
        }
        if let Some(pos) = line.find("<!--") {
            if !line[pos..].contains("-->") {
                in_html_comment = true;
            }
            continue;
        }
        if let Some(h) = trimmed.strip_prefix("### ") {
            if let Some(cap) = bare_path_re.captures(h) {
                current_h3_file = Some(cap[1].to_string());
            }
        }

        let symbols = plan_symbols_on_line(line);

        // Bare file paths in backticks — existence check. Only repo-relative
        // paths (containing `/`) are checked; a slashless `foo.rs` is prose
        // shorthand for a file fully named elsewhere, not a path claim.
        for cap in bare_path_re.captures_iter(line) {
            let tok = cap[1].to_string();
            if is_plan_placeholder_path(&tok) || !has_plan_source_ext(&tok) || !tok.contains('/') {
                continue;
            }
            // trace:TASK-772 | ai:claude — a `(new)` / `(to create)` marker
            // right after the closing backtick sanctions a to-be-created file.
            // trace:TASK-162 | ai:codex — heading-level `NEW` markers and
            // `## New files` sections sanction planned creations too.
            let marked_new = plan_path_marked_new(&line[cap.get(0).unwrap().end()..])
                || planned_new_paths.contains(&tok);
            if !checked_paths.insert(tok.clone()) {
                continue;
            }
            match (root.join(&tok).exists(), marked_new) {
                (true, false) => file_findings.push(PlanFinding {
                    level: PlanFindingLevel::Ok,
                    msg: tok,
                }),
                (true, true) => file_findings.push(PlanFinding {
                    level: PlanFindingLevel::Warn,
                    msg: format!(
                        "{tok} — marked new but already exists; stale annotation? \
                         (plan line {})",
                        idx + 1
                    ),
                }),
                (false, true) => file_findings.push(PlanFinding {
                    level: PlanFindingLevel::Ok,
                    msg: format!("{tok} — marked new (to be created)"),
                }),
                (false, false) => file_findings.push(PlanFinding {
                    level: PlanFindingLevel::Error,
                    msg: format!("{tok} — file not found (plan line {})", idx + 1),
                }),
            }
        }

        // `path.ext:NNN` refs — existence + drift.
        for cap in path_ref_re.captures_iter(line) {
            let path = cap[1].to_string();
            let claimed: usize = cap[2].parse().unwrap_or(0);
            if is_plan_placeholder_path(&path) || !has_plan_source_ext(&path) {
                continue;
            }
            let full = root.join(&path);
            if !full.exists() {
                // A slashless `main.rs:NNN` is shorthand — the file may exist
                // under some directory; warn rather than fail the build. A
                // path with a directory that doesn't resolve is a real break.
                if path.contains('/') {
                    ref_findings.push(PlanFinding {
                        level: PlanFindingLevel::Error,
                        msg: format!("{path}:{claimed} — file not found (plan line {})", idx + 1),
                    });
                } else {
                    ref_findings.push(PlanFinding {
                        level: PlanFindingLevel::Warn,
                        msg: format!(
                            "{path}:{claimed} — bare filename does not resolve; use a \
                             repo-relative path (plan line {})",
                            idx + 1
                        ),
                    });
                }
                continue;
            }
            let body = std::fs::read_to_string(&full).unwrap_or_default();
            let mut resolved = false;
            for sym in &symbols {
                if let Some(actual) = locate_symbol_line(&body, sym) {
                    resolved = true;
                    if actual == claimed {
                        ref_findings.push(PlanFinding {
                            level: PlanFindingLevel::Ok,
                            msg: format!("{path}:{claimed} — `{sym}` confirmed"),
                        });
                    } else {
                        let delta = actual as i64 - claimed as i64;
                        ref_findings.push(PlanFinding {
                            level: PlanFindingLevel::Warn,
                            msg: format!(
                                "{path}:{claimed} — `{sym}` is at line {actual} (drift {delta:+}); \
                                 prefer symbol form `{sym}` or update to {path}:{actual} (plan line {})",
                                idx + 1
                            ),
                        });
                        fixes.push(PlanRefFix {
                            line_idx: idx,
                            old: format!("{path}:{claimed}"),
                            new: format!("{path}:{actual}"),
                        });
                    }
                    break;
                }
            }
            if !resolved {
                ref_findings.push(PlanFinding {
                    level: PlanFindingLevel::Warn,
                    msg: format!(
                        "{path}:{claimed} — no named symbol on this line to verify the \
                         line number against (plan line {})",
                        idx + 1
                    ),
                });
            }
        }

        // `symbol:NNN` refs (no path) — resolve against the current
        // build-order file header, if any.
        for cap in sym_ref_re.captures_iter(line) {
            let sym = cap[1].to_string();
            let claimed: usize = cap[2].parse().unwrap_or(0);
            // Skip if this match is the tail of a longer token (a
            // `path.ext:line` ref or a `dir/seg:line` fragment) rather
            // than a standalone `symbol:line` ref.
            let mstart = cap.get(0).unwrap().start();
            if mstart > 0 {
                let prev = line.as_bytes()[mstart - 1];
                if prev == b'.' || prev == b'/' || prev == b'-' || prev.is_ascii_alphanumeric() {
                    continue;
                }
            }
            let Some(file) = &current_h3_file else {
                continue;
            };
            if is_plan_placeholder_path(file) {
                continue;
            }
            let full = root.join(file);
            let Ok(body) = std::fs::read_to_string(&full) else {
                continue;
            };
            if let Some(actual) = locate_symbol_line(&body, &sym) {
                if actual == claimed {
                    ref_findings.push(PlanFinding {
                        level: PlanFindingLevel::Ok,
                        msg: format!("{sym}:{claimed} — confirmed in {file}"),
                    });
                } else {
                    let delta = actual as i64 - claimed as i64;
                    ref_findings.push(PlanFinding {
                        level: PlanFindingLevel::Warn,
                        msg: format!(
                            "{sym}:{claimed} — `{sym}` is at line {actual} in {file} \
                             (drift {delta:+}); prefer symbol form `{sym}` (plan line {})",
                            idx + 1
                        ),
                    });
                    fixes.push(PlanRefFix {
                        line_idx: idx,
                        old: format!("{sym}:{claimed}"),
                        new: format!("{sym}:{actual}"),
                    });
                }
            }
        }
    }

    PlanReport {
        sections: section_findings,
        files: file_findings,
        refs: ref_findings,
        fixes,
    }
}

// `aida skill lint` — the forward-guard for skills that reference an
// implementation plan. A skill under `.claude/skills/` that points at a
// `docs/plans/*.md` file can rot when that plan drifts or is removed; this
// scans each skill for plan references, runs the same checks `aida plan
// verify` runs on each referenced plan (drifted refs / missing files / absent
// sections), and raw-glyph-checks the skill body. Read-only; never rewrites a
// plan or a skill. trace:TASK-927 | ai:claude

/// Count raw registry-glyph occurrences in a skill body. The set is derived
/// from `Glyph::ALL`'s unicode renderings — the same source `glyphs.rs` and
/// `scripts/glyph-lint.sh` track — so it stays in sync with the registry
/// automatically (and carries no raw glyph literal in this file, which would
/// itself trip the glyph-lint gate). A glyph in a skill body is a warning, not
/// an error: emoji in skill prose is allowed by design (the plan-ref check
/// owns the errors).
// trace:TASK-927 | ai:claude
pub(crate) fn count_registry_glyphs(content: &str) -> usize {
    crate::glyphs::Glyph::ALL
        .iter()
        .map(|g| content.matches(g.unicode()).count())
        .sum()
}

/// One referenced-plan or glyph finding for a single skill.
pub(crate) struct SkillPlanRefFinding {
    /// The repo-relative plan path as written in the skill.
    pub(crate) plan_ref: String,
    /// `Ok` if the plan exists and `aida plan verify` finds no errors;
    /// `Error` otherwise (missing file or verify errors). Glyph notes use
    /// `Warn`.
    pub(crate) level: PlanFindingLevel,
    /// Human-readable detail (e.g. "missing: docs/plans/x.md" or "3 plan
    /// error(s), 1 warning(s)").
    pub(crate) msg: String,
}

/// Extract every `docs/plans/...md` reference from a skill body. Matches the
/// path anywhere on a line — bare, in a markdown link, in backticks, or in
/// prose. De-duplicates while preserving first-seen order.
// trace:TASK-927 | ai:claude
pub(crate) fn extract_plan_refs(content: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut refs = Vec::new();
    let bytes = content.as_bytes();
    let needle = b"docs/plans/";
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            // Walk forward to the end of the path token. A path char is
            // anything that isn't whitespace or a delimiter that markdown /
            // prose uses to close a path (backtick, paren, angle bracket,
            // quote, comma, etc.).
            let start = i;
            let mut j = i + needle.len();
            while j < bytes.len() {
                let c = bytes[j] as char;
                if c.is_whitespace()
                    || matches!(c, '`' | ')' | '>' | '"' | '\'' | ',' | ']' | '(' | '<')
                {
                    break;
                }
                j += 1;
            }
            let mut path = content[start..j].to_string();
            // Trim a trailing sentence period that isn't part of `.md`.
            if path.ends_with('.') {
                path.pop();
            }
            // Skip illustrative placeholders, not real plan paths: glob
            // patterns (`docs/plans/*.md`), ellipsis stand-ins
            // (`docs/plans/...md`), and angle-bracket template markers
            // (`<docs/plans/...md>`). These appear in skill help text /
            // examples and must not be verified. trace:TASK-927
            let is_placeholder = path.contains('*') || path.contains("...");
            // Only keep things that actually look like a real plan markdown
            // file (ends in `.md`, longer than the bare `docs/plans/` prefix).
            if !is_placeholder
                && path.ends_with(".md")
                && path.len() > needle.len() + 3
                && seen.insert(path.clone())
            {
                refs.push(path);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    refs
}

/// Resolve the set of skill files to lint. With an explicit path, lint just
/// that file; otherwise every `*.md` under `.claude/skills/` (sorted, and
/// skipping `*.local.md` overrides which are merged at render time, not linted
/// on their own).
// trace:TASK-927 | ai:claude
pub(crate) fn resolve_skill_files(
    explicit: Option<&std::path::Path>,
    project_root: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>> {
    if let Some(p) = explicit {
        let resolved = if p.is_absolute() {
            p.to_path_buf()
        } else {
            project_root.join(p)
        };
        if !resolved.exists() {
            anyhow::bail!("skill file not found: {}", resolved.display());
        }
        return Ok(vec![resolved]);
    }
    let skills_dir = project_root.join(".claude").join("skills");
    if !skills_dir.is_dir() {
        anyhow::bail!(
            "no skills directory at {} — run `aida init` to scaffold skills",
            skills_dir.display()
        );
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&skills_dir)
        .with_context(|| format!("could not read {}", skills_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.ends_with(".md") && !name.ends_with(".local.md") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Lint each skill: for skills that reference a `docs/plans/*.md` file, verify
/// each referenced plan (reusing `compute_plan_report`, the same engine `aida
/// plan verify` uses) and raw-glyph-check the skill body. Prints a grouped
/// per-skill report (or JSON), and exits non-zero when any plan ref is missing
/// or its plan has verify errors.
// trace:TASK-927 | ai:claude
pub(crate) fn lint_skills(skill: Option<&std::path::Path>, json: bool, quiet: bool) -> Result<()> {
    let project_root = find_project_root().or_else(|_| std::env::current_dir())?;
    let skill_files = resolve_skill_files(skill, &project_root)?;

    // Per-skill collected findings, keyed by display label.
    struct SkillReport {
        label: String,
        findings: Vec<SkillPlanRefFinding>,
    }
    let mut reports: Vec<SkillReport> = Vec::new();
    let mut total_errors = 0usize;
    let mut total_warns = 0usize;
    let mut skills_with_refs = 0usize;

    for skill_path in &skill_files {
        let content = std::fs::read_to_string(skill_path)
            .with_context(|| format!("could not read skill {}", skill_path.display()))?;
        let plan_refs = extract_plan_refs(&content);
        if plan_refs.is_empty() {
            // Skills without a plan reference are out of scope — the lint
            // only fires on the plan-pointing subset. trace:TASK-927
            continue;
        }
        skills_with_refs += 1;
        let label = skill_path
            .strip_prefix(&project_root)
            .unwrap_or(skill_path)
            .display()
            .to_string();
        let mut findings = Vec::new();

        // 1. Verify each referenced plan with the plan-verify engine.
        for plan_ref in &plan_refs {
            let plan_path = project_root.join(plan_ref);
            if !plan_path.exists() {
                total_errors += 1;
                findings.push(SkillPlanRefFinding {
                    plan_ref: plan_ref.clone(),
                    level: PlanFindingLevel::Error,
                    msg: format!("referenced plan not found: {plan_ref}"),
                });
                continue;
            }
            let plan_content = match std::fs::read_to_string(&plan_path) {
                Ok(c) => c,
                Err(e) => {
                    total_errors += 1;
                    findings.push(SkillPlanRefFinding {
                        plan_ref: plan_ref.clone(),
                        level: PlanFindingLevel::Error,
                        msg: format!("could not read referenced plan {plan_ref}: {e}"),
                    });
                    continue;
                }
            };
            let root = plan_repo_root(&plan_path);
            let report = compute_plan_report(&plan_content, &root);
            let perrs = report.error_count();
            let pwarns = report.warn_count();
            if perrs > 0 {
                total_errors += 1;
                findings.push(SkillPlanRefFinding {
                    plan_ref: plan_ref.clone(),
                    level: PlanFindingLevel::Error,
                    msg: format!(
                        "plan verify failed: {perrs} error(s), {pwarns} warning(s) \
                         — run `aida plan verify {plan_ref}`"
                    ),
                });
            } else if pwarns > 0 {
                total_warns += 1;
                findings.push(SkillPlanRefFinding {
                    plan_ref: plan_ref.clone(),
                    level: PlanFindingLevel::Warn,
                    msg: format!("plan verify: 0 errors, {pwarns} warning(s)"),
                });
            } else {
                findings.push(SkillPlanRefFinding {
                    plan_ref: plan_ref.clone(),
                    level: PlanFindingLevel::Ok,
                    msg: format!("plan verify clean: {plan_ref}"),
                });
            }
        }

        // 2. Raw-glyph check on the skill body (warning-only — emoji in
        // skill prose is allowed by design).
        let glyph_count = count_registry_glyphs(&content);
        if glyph_count > 0 {
            total_warns += 1;
            findings.push(SkillPlanRefFinding {
                plan_ref: String::new(),
                level: PlanFindingLevel::Warn,
                msg: format!(
                    "{glyph_count} raw registry glyph literal(s) in skill body \
                     (allowed in prose; flagged for awareness)"
                ),
            });
        }

        reports.push(SkillReport { label, findings });
    }

    if json {
        let arr: Vec<serde_json::Value> = reports
            .iter()
            .map(|r| {
                serde_json::json!({
                    "skill": r.label,
                    "findings": r.findings.iter().map(|f| serde_json::json!({
                        "plan_ref": f.plan_ref,
                        "level": match f.level {
                            PlanFindingLevel::Ok => "ok",
                            PlanFindingLevel::Warn => "warn",
                            PlanFindingLevel::Error => "error",
                        },
                        "message": f.msg,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let out = serde_json::json!({
            "skills_scanned": skill_files.len(),
            "skills_with_plan_refs": skills_with_refs,
            "errors": total_errors,
            "warnings": total_warns,
            "results": arr,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        if total_errors > 0 {
            std::process::exit(1);
        }
        return Ok(());
    }

    println!(
        "{} {} skill(s) scanned, {} reference a plan",
        "Linting skills:".bold(),
        skill_files.len(),
        skills_with_refs
    );
    println!();

    for r in &reports {
        println!("{}", r.label.cyan().bold());
        for f in &r.findings {
            if quiet && f.level == PlanFindingLevel::Ok {
                continue;
            }
            let tag = match f.level {
                PlanFindingLevel::Ok => "  OK   ".green(),
                PlanFindingLevel::Warn => "  WARN ".yellow(),
                PlanFindingLevel::Error => "  ERROR".red().bold(),
            };
            println!("{} {}", tag, f.msg);
        }
        println!();
    }

    let verdict = if total_errors > 0 {
        format!("{total_errors} error(s), {total_warns} warning(s) — FAIL")
            .red()
            .bold()
            .to_string()
    } else if total_warns > 0 {
        format!("0 errors, {total_warns} warning(s) — PASS")
            .yellow()
            .to_string()
    } else {
        "all checks passed — PASS".green().bold().to_string()
    };
    println!("{} {}", "Verdict:".bold(), verdict);

    if total_errors > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// Render a `PlanReport` as a plain (un-colored) text report for the read-only
/// `plan_verify` MCP tool. Mirrors the CLI's grouped layout + verdict line but
/// returns a string instead of printing, and never rewrites or exits (the MCP
/// server must not mutate files or kill its own process). trace:EPIC-27
pub(crate) fn render_plan_report_string(report: &PlanReport, plan_label: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "Verifying plan: {}", plan_label);
    out.push('\n');

    let mut group = |title: &str, findings: &[PlanFinding]| {
        if findings.is_empty() {
            return;
        }
        let _ = writeln!(out, "{}", title);
        for f in findings {
            let tag = match f.level {
                PlanFindingLevel::Ok => "  OK   ",
                PlanFindingLevel::Warn => "  WARN ",
                PlanFindingLevel::Error => "  ERROR",
            };
            let _ = writeln!(out, "{} {}", tag, f.msg);
        }
        out.push('\n');
    };
    group("Sections", &report.sections);
    group("Files", &report.files);
    group("Line refs", &report.refs);

    let errors = report.error_count();
    let warns = report.warn_count();
    if !report.fixes.is_empty() {
        let _ = writeln!(
            out,
            "hint: run `aida plan verify <file> --fix` to rewrite {} drifted ref(s) automatically",
            report.fixes.len()
        );
        out.push('\n');
    }
    let verdict = if errors > 0 {
        format!("{} error(s), {} warning(s) — FAIL", errors, warns)
    } else if warns > 0 {
        format!("0 errors, {warns} warning(s) — PASS")
    } else {
        "all checks passed — PASS".to_string()
    };
    let _ = write!(out, "Verdict: {}", verdict);
    out
}

// ============================================================================
// TASK-94 — `aida plan helpers <spec>`. Derive a "Reusable helpers" section
// by walking the requirement graph (sibling / same-feature / same-tag specs)
// and harvesting those specs' `// trace:` comments from the codebase, so the
// implementer reuses existing helpers instead of re-inventing them.
// trace:TASK-94 | ai:claude
// ============================================================================

/// One harvested trace comment: a related spec touches `file`, and (when a
/// definition sits on or just below the comment) the named `symbol`.
pub(crate) struct TraceHit {
    pub(crate) file: String,
    pub(crate) symbol: Option<String>,
}

/// Recursively collect source files under `dir`, skipping hidden dirs and
/// the usual build/vendor trees.
pub(crate) fn collect_source_files(
    dir: &std::path::Path,
    exts: &[&str],
    out: &mut Vec<std::path::PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.')
                || name == "target"
                || name == "node_modules"
                || name == "vendor"
            {
                continue;
            }
        }
        if path.is_dir() {
            collect_source_files(&path, exts, out);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if exts.contains(&ext) {
                out.push(path);
            }
        }
    }
}

/// Scan the codebase for `trace:<SPEC-ID>` comments whose id is in `wanted`,
/// returning spec_id → the trace hits found. The symbol on each hit is the
/// first definition on the trace line or the two lines below it (comment
/// lines are skipped so the comment's own prose can't false-match).
/// trace:TASK-94 | ai:claude
pub(crate) fn scan_trace_graph(
    project_root: &std::path::Path,
    wanted: &HashSet<String>,
) -> std::collections::HashMap<String, Vec<TraceHit>> {
    scan_trace_graph_bounded(project_root, wanted, None).0
}

/// BUG-1594: [`scan_trace_graph`] with an optional wall-clock `budget`.
/// Returns the hits plus `complete` — `false` when the budget ran out before
/// every source file was read, so a caller can say the file list is partial
/// instead of presenting it as complete (PRIN-5). A file is only line-split
/// and regex-matched when its raw text contains `trace:` and one of the
/// wanted ids — the common case (a file that never mentions the spec) now
/// costs one substring search instead of a regex pass over every line.
// trace:BUG-1594 | ai:claude
pub(crate) fn scan_trace_graph_bounded(
    project_root: &std::path::Path,
    wanted: &HashSet<String>,
    budget: Option<std::time::Duration>,
) -> (std::collections::HashMap<String, Vec<TraceHit>>, bool) {
    use regex::Regex;
    let started = std::time::Instant::now();
    // Capture the full spec id — including the optional third segment of a
    // node-aware id (`FR-1-042`), or `trace:FR-1-042` would bucket under a
    // bogus `FR-1`.
    let trace_re =
        Regex::new(r"\btrace:([A-Z]+-[A-Z0-9]+(?:-[A-Z0-9]+)*-[0-9]+|[A-Z]+-[0-9]+)").unwrap();
    let sym_re = Regex::new(
        r"\b(?:fn|struct|enum|trait|type|mod|const|static|function|class|interface)\s+([A-Za-z_][A-Za-z0-9_]*)",
    )
    .unwrap();
    // BUG-568: trace comments are indexed only from the LOCAL source tree, so
    // trace markers in sibling repos of a shared-store workspace are never
    // found. Warn loudly (deduped per-process). trace:BUG-568 | ai:claude
    warn_multi_repo_scan_limited(project_root, "trace-comment scan");

    let exts = ["rs", "ts", "tsx", "js", "jsx", "py", "sh"];
    let mut files = Vec::new();
    collect_source_files(project_root, &exts, &mut files);

    let mut out: std::collections::HashMap<String, Vec<TraceHit>> =
        std::collections::HashMap::new();
    let mut complete = true;
    for path in &files {
        if budget.is_some_and(|b| started.elapsed() >= b) {
            complete = false;
            break;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        // BUG-1594: cheap prefilter before the per-line regex pass.
        if !content.contains("trace:") || !wanted.iter().any(|id| content.contains(id.as_str())) {
            continue;
        }
        let lines: Vec<&str> = content.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            for cap in trace_re.captures_iter(line) {
                let id = cap[1].to_string();
                if !wanted.contains(&id) {
                    continue;
                }
                // Symbol: first definition on this line or the next two,
                // skipping pure comment lines.
                let mut symbol = None;
                for probe in lines.iter().skip(i).take(3) {
                    let t = probe.trim_start();
                    if t.starts_with("//") || t.starts_with('#') || t.starts_with('*') {
                        continue;
                    }
                    if let Some(m) = sym_re.captures(probe) {
                        symbol = Some(m[1].to_string());
                        break;
                    }
                }
                let rel = path
                    .strip_prefix(project_root)
                    .unwrap_or(path)
                    .display()
                    .to_string();
                out.entry(id)
                    .or_default()
                    .push(TraceHit { file: rel, symbol });
            }
        }
    }
    (out, complete)
}

/// STORY-511: the resolved state of the change-request (PR/MR) line in
/// `aida show`'s git-linkage section, decoupled from the git/forge probes
/// so the *rendering* is a pure function of (forge, state) — and thus
/// unit-testable with no git repo and no `gh`/`glab` on PATH. EPIC-35
/// slice 5 makes this section forge-aware: a GitLab repo reads "MR-47"
/// and "glab not installed", not the GitHub-only "PR-47"/"gh" wording.
/// trace:STORY-511 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChangeLinkageState {
    /// Shipped (merged to the default branch); carries the squash-merge
    /// change number when one was parsed from the subject.
    Shipped { number: Option<u64> },
    /// In flight, an open change was found. `number` + `url`.
    InFlightFound { number: u64, url: String },
    /// In flight, the forge CLI ran cleanly and reported no open change.
    InFlightNoChange,
    /// In flight, the forge CLI is not on PATH.
    CliMissing,
    /// In flight, the forge CLI ran but errored (auth / parse).
    CliFailed,
    /// In flight, the forge API was unreachable (transient).
    Unreachable,
    /// Work is committed but no local branch holds it.
    BranchNotFound,
    /// BUG-1594: the forge lookup was killed at its per-call ceiling.
    // trace:BUG-1594 | ai:claude
    TimedOut,
    /// BUG-1594: the shared forge-lookup budget was already spent, so this
    /// branch's lookup never ran.
    // trace:BUG-1594 | ai:claude
    LookupSkipped,
}

/// BUG-1594: map a forge lookup onto the rendered linkage state (plus the
/// open change's url), distinguishing a ceiling timeout from a generic
/// unreachable API. Shared by the primary and sibling-branch lookups.
// trace:BUG-1594 | ai:claude
pub(crate) fn change_linkage_state_for(
    lookup: crate::forge::ChangeLookup,
) -> (ChangeLinkageState, Option<String>) {
    match lookup {
        crate::forge::ChangeLookup::Found(c) => (
            ChangeLinkageState::InFlightFound {
                number: c.id,
                url: c.url.clone(),
            },
            Some(c.url),
        ),
        crate::forge::ChangeLookup::NoChange => (ChangeLinkageState::InFlightNoChange, None),
        crate::forge::ChangeLookup::CliMissing => (ChangeLinkageState::CliMissing, None),
        crate::forge::ChangeLookup::CliFailed(_) => (ChangeLinkageState::CliFailed, None),
        crate::forge::ChangeLookup::Unreachable(msg) if msg == FORGE_LOOKUP_TIMED_OUT => {
            (ChangeLinkageState::TimedOut, None)
        }
        crate::forge::ChangeLookup::Unreachable(_) => (ChangeLinkageState::Unreachable, None),
    }
}

/// BUG-1594: the note `aida show` prints when the in-flight branch search
/// stopped before covering every referencing commit.
// trace:BUG-1594 | ai:claude
pub(crate) fn format_branch_scan_truncated_note(scanned: usize, total: usize) -> String {
    format!(
        "branch search incomplete: checked the newest {scanned} of {total} commits \
         — other branches may also reference this spec"
    )
}

/// STORY-511: render the change-request linkage lines for `aida show`'s
/// git-linkage section, forge-aware. Returns `(label, value)` pairs of
/// **plain, uncolored** text in render order (caller applies styling).
/// The change noun comes from [`crate::forge::ForgeKind::change_noun`]
/// ("PR" / "MR" / "change") and the CLI name from
/// [`crate::forge::ForgeKind::cli_name`] ("gh" / "glab" / "the forge CLI")
/// so a GitLab project reads "MR-47" and "glab not installed" rather than
/// the GitHub-only wording. Pure: no git, no forge CLI, no I/O — fully
/// unit-testable in isolation. trace:STORY-511 | ai:claude
pub(crate) fn format_change_linkage(
    forge: crate::forge::ForgeKind,
    state: &ChangeLinkageState,
) -> Vec<(String, String)> {
    let noun = forge.change_noun(); // "PR" | "MR" | "change"
    let cli = forge.cli_name(); // "gh" | "glab" | ""
                                // Pure-git has no forge CLI binary; name it generically rather than
                                // printing an empty token in "<cli> not installed".
    let cli_label = if cli.is_empty() { "the forge CLI" } else { cli };
    let mut out: Vec<(String, String)> = Vec::new();
    match state {
        ChangeLinkageState::Shipped { number } => {
            out.push(("Branch".to_string(), "merged to main".to_string()));
            if let Some(num) = number {
                out.push((noun.to_string(), format!("{noun}-{num}")));
            }
        }
        ChangeLinkageState::InFlightFound { number, url } => {
            out.push((noun.to_string(), format!("{noun}-{number} {url}")));
        }
        ChangeLinkageState::InFlightNoChange => {
            out.push((noun.to_string(), format!("no {noun} opened yet")));
        }
        ChangeLinkageState::CliMissing => {
            out.push((
                noun.to_string(),
                format!("{cli_label} not installed — {noun} state unknown"),
            ));
        }
        ChangeLinkageState::CliFailed => {
            out.push((
                noun.to_string(),
                format!("{cli_label} lookup failed — {noun} state unknown"),
            ));
        }
        ChangeLinkageState::Unreachable => {
            out.push((
                noun.to_string(),
                format!("{noun} API unreachable — {noun} state unknown (transient)"),
            ));
        }
        ChangeLinkageState::TimedOut => {
            // trace:BUG-1594 | ai:claude
            out.push((
                noun.to_string(),
                format!(
                    "{cli_label} lookup timed out after {}s — {noun} state unknown",
                    FORGE_CLI_CALL_TIMEOUT.as_secs()
                ),
            ));
        }
        ChangeLinkageState::LookupSkipped => {
            // trace:BUG-1594 | ai:claude
            out.push((
                noun.to_string(),
                format!("{noun} lookup skipped (time budget spent) — {noun} state unknown"),
            ));
        }
        ChangeLinkageState::BranchNotFound => {
            // BUG-1528 (PRIN-5): a failed branch lookup must never silently
            // suppress the PR line. Before this fix, `BranchNotFound` was
            // the one arm of this enum that emitted no PR/MR line at all —
            // every other "in-flight but uncertain" arm (CliMissing /
            // CliFailed / Unreachable) already says "state unknown" instead
            // of going quiet. Match that convention here too, distinctly
            // from "no PR was ever opened" (`InFlightNoChange`).
            // trace:BUG-1528 | ai:claude
            out.push((
                "Branch".to_string(),
                "work committed but branch not found locally".to_string(),
            ));
            out.push((
                noun.to_string(),
                format!("{noun} state unknown — branch not found locally"),
            ));
        }
    }
    out
}

/// TASK-241: extract the PR number from a GitHub squash-merge commit
/// subject, which ends with `(#NN)`. Returns None when the subject has
/// no such suffix. trace:TASK-241 | ai:claude
pub(crate) fn parse_squash_pr_number(subject: &str) -> Option<u64> {
    subject
        .trim_end()
        .rsplit_once("(#")
        .and_then(|(_, tail)| tail.strip_suffix(')'))
        .and_then(|n| n.parse::<u64>().ok())
}

/// TASK-238/STORY-442: the inline tag-chip body for an `aida queue list`
/// row. `batch:*` and `lifecycle:*` tags come first and are always shown
/// (they alter pickup/routing behavior); plain tags are sorted, capped at
/// 3, and any remainder collapses to a `+N` overflow marker. Returns None
/// when the requirement carries no tags. trace:TASK-238 STORY-442
pub(crate) fn format_tag_chip(tags: &std::collections::HashSet<String>) -> Option<String> {
    if tags.is_empty() {
        return None;
    }
    const MAX_PLAIN: usize = 3;
    let mut batch_tags: Vec<&str> = Vec::new();
    let mut lifecycle_tags: Vec<&str> = Vec::new();
    let mut plain_tags: Vec<&str> = Vec::new();
    for t in tags {
        if t.to_ascii_lowercase().starts_with("batch:") {
            batch_tags.push(t.as_str());
        } else if t.to_ascii_lowercase().starts_with("lifecycle:") {
            lifecycle_tags.push(t.as_str());
        } else {
            plain_tags.push(t.as_str());
        }
    }
    batch_tags.sort_unstable();
    lifecycle_tags.sort_unstable();
    plain_tags.sort_unstable();
    let mut shown: Vec<String> = batch_tags.iter().map(|s| s.to_string()).collect();
    shown.extend(lifecycle_tags.iter().map(|s| s.to_string()));
    shown.extend(plain_tags.iter().take(MAX_PLAIN).map(|s| s.to_string()));
    if plain_tags.len() > MAX_PLAIN {
        shown.push(format!("+{}", plain_tags.len() - MAX_PLAIN));
    }
    Some(shown.join(", "))
}

/// TASK-238: does the tag set contain `want` (case-insensitive)? The
/// predicate behind `aida queue list --tag`. trace:TASK-238 | ai:claude
pub(crate) fn tag_matches_exact(tags: &std::collections::HashSet<String>, want: &str) -> bool {
    tags.iter().any(|t| t.eq_ignore_ascii_case(want))
}

/// TASK-238: does any tag start with `prefix` (case-insensitive)? The
/// predicate behind `aida queue list --tag-prefix`. trace:TASK-238
pub(crate) fn tag_matches_prefix(tags: &std::collections::HashSet<String>, prefix: &str) -> bool {
    let lp = prefix.to_ascii_lowercase();
    tags.iter().any(|t| t.to_ascii_lowercase().starts_with(&lp))
}

/// TASK-238: the `batch:*` tag a requirement carries (lowest-sorting
/// when several), used as the group key for `aida queue list
/// --by-batch`. None when the requirement is un-batched.
/// trace:TASK-238 | ai:claude
pub(crate) fn batch_tag_of(tags: &std::collections::HashSet<String>) -> Option<&str> {
    let mut found: Vec<&str> = tags
        .iter()
        .filter(|t| t.to_ascii_lowercase().starts_with("batch:"))
        .map(|s| s.as_str())
        .collect();
    found.sort_unstable();
    found.into_iter().next()
}

/// TASK-270: strip a redundant leading `batch:` prefix off a string,
/// returning `Some(rest)` only when the prefix was present
/// (case-insensitive). `batch:` is the literal tag printed by `aida
/// queue list`; first-users reflexively copy that whole token back into
/// `--batch` or a positional id. trace:TASK-270 | ai:claude
pub(crate) fn strip_batch_prefix(s: &str) -> Option<&str> {
    let prefix = "batch:";
    s.get(..prefix.len())
        .filter(|p| p.eq_ignore_ascii_case(prefix))
        .map(|_| &s[prefix.len()..])
}

/// TASK-270: normalize a `--batch` flag value so `--batch NAME` and the
/// redundant `--batch batch:NAME` (the literal tag from `aida queue
/// list`) both resolve to `NAME`. trace:TASK-270 | ai:claude
pub(crate) fn normalize_batch_name(s: &str) -> &str {
    strip_batch_prefix(s).unwrap_or(s)
}

/// TASK-310: parse comma-separated batch-chain names. Names are kept in the
/// caller's declared order; `batch:` prefixes are tolerated per TASK-270.
pub(crate) fn parse_batch_chain(raw: &str) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for part in raw.split(',') {
        let name = normalize_batch_name(part.trim()).trim();
        if name.is_empty() {
            anyhow::bail!(
                "batch list contains an empty name — use comma-separated names like `--batches A,B,C`"
            );
        }
        names.push(name.to_string());
    }
    if names.is_empty() {
        anyhow::bail!("batch list is empty — use comma-separated names like `--batches A,B,C`");
    }
    Ok(names)
}

/// TASK-270: resolve the effective positional id and batch name for
/// `aida queue work`. Accepts `batch:NAME` as a positional id
/// (equivalent to `--batch NAME`) and strips a redundant `batch:`
/// prefix off the `--batch` flag value. At most one element of the
/// returned `(id, batch)` pair is `Some` — clap's
/// `conflicts_with(batch, id)` already rejects passing both, and a
/// `batch:`-prefixed positional is rerouted to the batch slot.
/// trace:TASK-270 | ai:claude
/// TASK-322: true when a `queue work` positional is a `next` / `nextN`
/// head-pickup keyword (vs a SPEC-ID or `batch:NAME`). Used to catch the
/// `--batch` + `nextN` collision that otherwise silently dropped the keyword.
/// trace:TASK-322 | ai:claude
pub(crate) fn is_next_keyword_id(id: &str) -> bool {
    id.to_ascii_lowercase()
        .strip_prefix("next")
        .map(|suffix| suffix.is_empty() || suffix.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false)
}

pub(crate) fn resolve_queue_work_batch<'a>(
    id: Option<&'a str>,
    batch: Option<&'a str>,
) -> (Option<&'a str>, Option<&'a str>) {
    if let Some(b) = batch {
        return (None, Some(normalize_batch_name(b)));
    }
    if let Some(i) = id {
        if let Some(name) = strip_batch_prefix(i) {
            return (None, Some(name));
        }
        return (Some(i), None);
    }
    (None, None)
}

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

/// TASK-241: surface git linkage for a spec inside `aida show` — the
/// commits that reference it (AIDA commit format puts `(SPEC-ID)` in
/// the message), the files carrying its `trace:` comments, the branch +
/// worktree the work lives on, and PR state. Every sub-probe is
/// best-effort: a missing `gh`, an absent `origin/main`, or zero
/// matches degrades to a dim note rather than failing the show.
/// `--no-git` skips the whole section; `--verbose` un-caps the commit
/// list and adds per-commit diff stats. trace:TASK-241 | ai:claude
/// Resolve a refname for the default branch that is **known to exist**
/// in `project_root`, so callers can pass it to `git merge-base
/// --is-ancestor` without risking a `fatal: Not a valid object name <X>`
/// stderr splice when the repo's default branch is not `main`.
///
/// Resolution order:
///   1. `origin/HEAD` symbolic ref (the actual default branch on origin)
///   2. `init.defaultBranch` config — `origin/<name>` then bare `<name>`
///   3. The `"main"` / `"master"` literals — `origin/<name>` then bare
///
/// Returns `None` when no candidate ref exists; callers should then
/// skip the ancestor check rather than guess (the prior code passed
/// `"main"` as a literal, which leaked stderr on master-default repos).
/// trace:BUG-380 | ai:claude
pub(crate) fn resolve_default_branch_ref(project_root: &std::path::Path) -> Option<String> {
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
            .filter(|s| !s.is_empty())
    };

    if let Some(short) = git(&[
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ]) {
        return Some(short);
    }

    let mut candidates: Vec<String> = Vec::new();
    if let Some(cfg) = git(&["config", "--get", "init.defaultBranch"]) {
        candidates.push(cfg);
    }
    for lit in ["main", "master"] {
        if !candidates.iter().any(|c| c == lit) {
            candidates.push(lit.to_string());
        }
    }

    for name in candidates {
        let origin = format!("origin/{}", name);
        if git(&["rev-parse", "--verify", "--quiet", &origin]).is_some() {
            return Some(origin);
        }
        if git(&["rev-parse", "--verify", "--quiet", &name]).is_some() {
            return Some(name);
        }
    }
    None
}

/// TASK-241: the git-linkage data for a spec — extracted from
/// [`print_git_linkage`] so the collection (git-only, never gh) is
/// unit-testable against a temp repo. trace:TASK-241 | ai:claude
// trace:STORY-82 | ai:claude — `pub(crate)` so the MCP `show_requirement`
// tool can reuse the same git-linkage collection `aida show` renders.
pub(crate) struct GitLinkage {
    /// (full_sha, short_sha, subject), newest first.
    pub(crate) commits: Vec<(String, String, String)>,
    /// (file, symbol) per trace comment — deduped and sorted.
    pub(crate) files: Vec<(String, Option<String>)>,
    /// The newest referencing commit is an ancestor of main.
    pub(crate) shipped: bool,
    /// Feature branch holding the work (in-flight case only).
    pub(crate) branch: Option<String>,
    /// BUG-1528: other branches (besides `branch`) whose name matches the
    /// spec id and that also carry a referencing commit — a branch-crossing
    /// signal (e.g. `bug-1420-work` + `bug-1420-round2` both open). Rendered
    /// as a note so a reviewer sees the fan-out instead of one branch picked
    /// silently. Empty in the common single-branch case.
    // trace:BUG-1528 | ai:claude
    pub(crate) other_branches: Vec<String>,
    /// BUG-1594: `Some((scanned, total))` when the in-flight branch search
    /// stopped before walking every referencing commit (commit cap or wall-
    /// clock budget hit), so `branch`/`other_branches` may be incomplete.
    /// Rendered as an explicit note — partial linkage is never shown as
    /// complete (PRIN-5). `None` when the search covered every commit.
    // trace:BUG-1594 | ai:claude
    pub(crate) branch_scan_truncated: Option<(usize, usize)>,
    /// BUG-1594: the trace-comment walk hit its time budget, so `files` may
    /// be missing entries. Rendered as an explicit note (PRIN-5).
    // trace:BUG-1594 | ai:claude
    pub(crate) files_scan_incomplete: bool,
    /// Worktree path checked out at `branch`, if any.
    pub(crate) worktree: Option<String>,
    /// PR number parsed from a squash-merge subject (shipped case only).
    pub(crate) shipped_pr: Option<u64>,
    /// Workspace repo slug this linkage was scanned from (ADR-12 D5: the
    /// `origin.repo` join key qualifying commits/branch/PR). `None` in the
    /// single-repo case — rendering stays unqualified, unchanged.
    // trace:STORY-634 | ai:claude
    pub(crate) repo: Option<String>,
}

/// BUG-1594: the most referencing commits the in-flight branch search walks
/// (newest first) before it stops and reports itself partial.
// trace:BUG-1594 | ai:claude
pub(crate) const LINKAGE_BRANCH_SCAN_MAX_COMMITS: usize = 20;

/// BUG-1594: the wall-clock budget for the in-flight branch search.
// trace:BUG-1594 | ai:claude
pub(crate) const LINKAGE_BRANCH_SCAN_BUDGET: std::time::Duration =
    std::time::Duration::from_secs(2);

/// BUG-1594: the wall-clock budget for the trace-comment source walk.
// trace:BUG-1594 | ai:claude
pub(crate) const LINKAGE_TRACE_SCAN_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);

/// BUG-1594: the total wall-clock budget `aida show` spends on forge
/// (PR/MR) lookups across the primary branch and any sibling branches; each
/// single call is additionally capped at [`FORGE_CLI_CALL_TIMEOUT`].
// trace:BUG-1594 | ai:claude
pub(crate) const SHOW_FORGE_LOOKUP_BUDGET: std::time::Duration = std::time::Duration::from_secs(4);

/// TASK-1475: wall-clock ceiling for EACH bounded git call the filing-drift
/// hint makes (at most two: the linked-commits file-touch lookup, and the
/// `code_sha..HEAD` log walk). Local git only, no network — this is a
/// safety net against a pathological huge history, not the expected cost.
// trace:TASK-1475 | ai:claude
pub(crate) const DRIFT_HINT_GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// TASK-1475 (follow-up to CR-8 acceptance 6): a cheap filing-provenance
/// staleness signal — how many commits since `filed_at.code_sha` touched a
/// file this spec's `trace:` comments or linked commits point at. Local git
/// only (no network), every subprocess bounded via
/// [`command_output_with_timeout`] so a slow/huge repo can only ever cost
/// this function its fixed per-call budget, never hang `aida show`/`aida
/// why`.
///
/// `linkage_files`/`linkage_commits` are the already-collected
/// [`GitLinkage::files`]/[`GitLinkage::commits`] — this never re-runs the
/// trace-comment tree walk or the referencing-commit log.
///
/// Returns `None` when there is nothing to say: no provenance, no
/// `code_sha`, an unresolvable sha (shallow clone / a different repo —
/// TASK-1475 AC2), no traced files at all, or a genuinely driftless zero
/// (no commits since filing touched any traced file — not worth a line).
/// When the sha resolves but the bounded walk can't finish in budget,
/// returns `Some(..)` naming the state "unknown" rather than silently
/// reporting zero — a truncated scan must never be mistaken for a complete,
/// driftless one (PRIN-5).
// trace:TASK-1475 | ai:claude
pub(crate) fn filing_drift_hint(
    project_root: &std::path::Path,
    filed_at: Option<&aida_core::FilingProvenance>,
    linkage_files: &[(String, Option<String>)],
    linkage_commits: &[(String, String, String)],
) -> Option<String> {
    let code_sha = filed_at.and_then(|p| p.code_sha.as_deref())?.trim();
    // `code_sha` comes from the shared store, which another writer controls:
    // only a commit ID may reach `git log`. trace:BUG-1622 | ai:claude
    if !git_arg_guard::is_hex_sha(code_sha) {
        return None;
    }

    // Traced files: every location a `trace:` comment names this spec, plus
    // every file the spec's own linked (referencing) commits touched — one
    // bounded `git show` covering all of them at once.
    let mut traced: std::collections::BTreeSet<String> =
        linkage_files.iter().map(|(f, _)| f.clone()).collect();
    if !linkage_commits.is_empty() {
        let shas: Vec<&str> = linkage_commits
            .iter()
            .take(LINKAGE_BRANCH_SCAN_MAX_COMMITS)
            .map(|(full, _, _)| full.as_str())
            .collect();
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C")
            .arg(project_root)
            .args(["show", "--format=", "--name-only"])
            // trace:BUG-1622 | ai:claude
            .arg(git_arg_guard::END_OF_OPTIONS)
            .args(&shas);
        if let Some(out) = command_output_with_timeout(cmd, DRIFT_HINT_GIT_TIMEOUT) {
            if out.status.success() {
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    let line = line.trim();
                    if !line.is_empty() {
                        traced.insert(line.to_string());
                    }
                }
            }
        }
    }
    if traced.is_empty() {
        return None;
    }

    // One bounded `git log <code_sha>..HEAD --name-only -- <traced files>`
    // does double duty: a non-zero exit means the sha didn't resolve
    // (shallow clone / different repo — AC2, degrade to silence); a
    // timeout means the walk itself couldn't finish (degrade to "unknown",
    // never a guessed zero); success parses into a commit count plus the
    // distinct traced files those commits actually touched (pathspec-
    // restricted, so this is always <= traced.len()).
    let mut log_cmd = std::process::Command::new("git");
    log_cmd
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args([
            "log",
            "--name-only",
            "--pretty=format:\u{1}",
            git_arg_guard::END_OF_OPTIONS,
            &format!("{code_sha}..HEAD"),
        ])
        .arg("--")
        .args(traced.iter());
    match command_output_with_timeout(log_cmd, DRIFT_HINT_GIT_TIMEOUT) {
        Some(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut commit_count = 0usize;
            let mut touched: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            for chunk in text.split('\u{1}') {
                let chunk = chunk.trim();
                if chunk.is_empty() {
                    continue;
                }
                commit_count += 1;
                for line in chunk.lines() {
                    let line = line.trim();
                    if !line.is_empty() {
                        touched.insert(line);
                    }
                }
            }
            if commit_count == 0 {
                // Genuinely driftless — nothing to hint at.
                None
            } else {
                Some(format!(
                    "{commit_count} commit{} since filing touched {} traced file{}",
                    if commit_count == 1 { "" } else { "s" },
                    touched.len(),
                    if touched.len() == 1 { "" } else { "s" },
                ))
            }
        }
        Some(_) => None,
        None => Some("commits since filing: unknown (scan didn't finish in budget)".to_string()),
    }
}

/// TASK-241: collect the git linkage for `ids` — commits referencing the
/// AIDA `(SPEC-ID)` format, files carrying `trace:` comments, and
/// branch/worktree/shipped state. gh-free by design: the in-flight
/// open-PR lookup happens later, in [`print_git_linkage`].
/// trace:TASK-241 | ai:claude
pub(crate) fn collect_git_linkage(project_root: &std::path::Path, ids: &[String]) -> GitLinkage {
    collect_git_linkage_opts(project_root, ids, true)
}

/// The orphan git-canonical requirements-store branch name (see CLAUDE.md
/// "Storage model"). Its `update SPEC-NNN` / `add SPEC-NNN` bookkeeping
/// commits — and cross-node store-lineage merges — mention a bare spec id in
/// the subject, which is indistinguishable at the grep layer from a real
/// code commit. Any scan that resolves a spec id to a CODE branch/commit
/// range must exclude this branch, or it mistakes the requirements store
/// itself for the spec's feature branch (`aida review` / `aida human review`
/// offered to open a PR from `aida-store`). `scan_completed_without_commit`
/// avoids the same trap structurally, by only ever walking the resolved
/// default CODE branch.
///
/// Accepts either a bare branch name (`aida-store`) or a remote-qualified
/// short ref (`origin/aida-store`, `gitlab/aida-store`, …) — a multi-hub
/// project (STORY-760) can have the orphan branch mirrored under several
/// remote prefixes, and `git branch --all --contains` reports it under
/// each, so matching only the bare literal would miss every remote but one.
// trace:BUG-720 | ai:claude
pub(crate) fn is_orphan_store_branch(branch: &str) -> bool {
    let name = branch.rsplit('/').next().unwrap_or(branch);
    name.eq_ignore_ascii_case("aida-store")
}

/// BUG-550: variant of [`collect_git_linkage`] with the full source-tree walk
/// (`scan_trace_graph`, which populates `GitLinkage::files`) gated behind
/// `scan_trace`. Surfaces that classify a spec's *review state* — the `aida
/// human` reviews bucket runs this once per candidate spec — only read
/// `commits`/`branch`/`shipped`/`shipped_pr`, never `files`, so they pass
/// `false` to skip the per-spec tree scan. That scan reads every source file
/// in the repo; multiplying it by a widened candidate set is the dominant
/// cost on this hot, offline command. With `scan_trace = false`, `files` is
/// always empty. trace:BUG-550 | ai:claude
pub(crate) fn collect_git_linkage_opts(
    project_root: &std::path::Path,
    ids: &[String],
    scan_trace: bool,
) -> GitLinkage {
    use std::process::Command as PCmd;

    // BUG-568: linkage (commits/files/PR) is collected only from the LOCAL
    // repo. In a shared-store multi-repo workspace, work done in a sibling repo
    // shows zero linkage here — warn loudly so the gap is visible rather than
    // read as "no work done". Suppressible via AIDA_QUIET for hot batch callers
    // (e.g. the `aida human` reviews sweep). trace:BUG-568 | ai:claude
    warn_multi_repo_scan_limited(project_root, "git-linkage scan");

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
    // terminal — belt-and-braces alongside `resolve_default_branch_ref`,
    // which already filters `target` to a ref that exists.
    let is_ancestor = |sha: &str, target: &str| -> bool {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["merge-base", "--is-ancestor", sha, target])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };

    // ---- Commits referencing the spec (across all refs) ----
    // BUG-546: grep the BARE id across all refs, then keep only commits whose
    // FULL message resolves to the wanted id through the same robust parser
    // the auto-bump / reconcile surfaces use (`(SPEC-ID)` trailers — including
    // multi-spec / prose parens like `(TASK-800 / STORY-610 slice 1a)` — plus
    // `trace:SPEC-ID` lines in the body). The old approach grepped the fixed
    // string `(SPEC-ID)`, so a real PR whose subject paren named the spec
    // alongside prose was invisible to `aida review <spec>` and the `aida
    // human` reviews bucket. The post-grep resolver replaces the parens-only
    // filter that previously excluded the orphan `aida-store` bookkeeping
    // commits ("update TASK-92"): those carry the bare id but neither a
    // trailer nor a `trace:` line, so the resolver drops them. trace:BUG-546
    let wanted_lc: HashSet<String> = ids.iter().map(|s| s.to_ascii_uppercase()).collect();
    let mut args: Vec<&str> = vec![
        "log",
        "--all",
        "-F",
        "--date-order",
        "--pretty=format:%H\u{1f}%h\u{1f}%s\u{1f}%B%x1e",
    ];
    for id in ids {
        args.push("--grep");
        args.push(id.as_str());
    }
    let log = git(&args).unwrap_or_default();
    let commits: Vec<(String, String, String)> = log
        .split('\u{1e}')
        .filter_map(|record| {
            let record = record.trim_start_matches(['\n', '\r']);
            if record.is_empty() {
                return None;
            }
            let mut p = record.splitn(4, '\u{1f}');
            let full = p.next()?.to_string();
            let short = p.next()?.to_string();
            let subject = p.next()?.to_string();
            let body = p.next().unwrap_or("");
            // Resolve the commit's full message and keep it only when it
            // actually references one of the wanted ids via a trailer or a
            // `trace:` line — not just a bare mention. trace:BUG-546
            let mut resolved = extract_spec_ids_from_commit(body);
            resolved.extend(extract_referenced_spec_ids_from_commit(body));
            resolved.extend(extract_trace_line_spec_ids(body));
            if resolved
                .iter()
                .any(|r| wanted_lc.contains(&r.to_ascii_uppercase()))
            {
                Some((full, short, subject))
            } else {
                None
            }
        })
        .collect();

    // ---- Files carrying trace comments for the spec ----
    // BUG-550: gated — the per-spec source-tree walk only matters to callers
    // that render `files`; the reviews-bucket classifier skips it.
    // BUG-1594: bounded — a cold page cache made this walk alone take tens
    // of seconds; past the budget the list is flagged partial, not complete.
    let mut files_scan_incomplete = false;
    let files: Vec<(String, Option<String>)> = if scan_trace {
        let wanted: HashSet<String> = ids.iter().cloned().collect();
        let (trace_hits, complete) =
            scan_trace_graph_bounded(project_root, &wanted, Some(LINKAGE_TRACE_SCAN_BUDGET));
        files_scan_incomplete = !complete;
        let mut f: Vec<(String, Option<String>)> = trace_hits
            .values()
            .flatten()
            .map(|h| (h.file.clone(), h.symbol.clone()))
            .collect();
        f.sort();
        f.dedup();
        f
    } else {
        Vec::new()
    };

    // ---- Branch / worktree / shipped state (anchored on newest commit) ----
    let mut shipped = false;
    let mut branch: Option<String> = None;
    let mut other_branches: Vec<String> = Vec::new();
    let mut branch_scan_truncated: Option<(usize, usize)> = None;
    let mut worktree: Option<String> = None;
    let mut shipped_pr: Option<u64> = None;
    if let Some((full, _, _)) = commits.first() {
        // BUG-380: resolve a ref that exists rather than passing a
        // literal "main"; on master-default repos the literal leaks
        // `fatal: Not a valid object name 'main'` from git through
        // is_ancestor's inherited stderr.
        shipped = resolve_default_branch_ref(project_root)
            .map(|main_ref| is_ancestor(full, &main_ref))
            .unwrap_or(false);
        if shipped {
            // A squash-merge subject ends with `(#NN)` — the cheapest,
            // most reliable PR pointer for shipped work.
            shipped_pr = commits
                .iter()
                .find_map(|(_, _, s)| parse_squash_pr_number(s));
        } else {
            // In flight: find the feature branch that holds the work.
            //
            // BUG-1528: don't anchor solely on the SINGLE newest referencing
            // commit. When a spec has multiple branches in flight (a
            // branch-crossing — e.g. `bug-1420-work` plus a later
            // `bug-1420-round2`), the newest commit across ALL of them can
            // live on a branch that gets filtered out below (main/master/
            // orphan-store) or simply isn't the spec's own branch, leaving
            // the spec's actual local branch entirely unreachable via
            // `--contains <newest-sha>` even though `git branch --list`
            // plainly shows it. Union the `--contains` result over EVERY
            // referencing commit (still newest-first, so ties still prefer
            // recency) so a branch holding an older-but-still-relevant
            // commit is found too. trace:BUG-1528 | ai:claude
            let norm_id = |s: &str| s.to_ascii_lowercase().replace([' ', '_'], "-");
            let mut candidates: Vec<String> = Vec::new();
            let mut local_candidates: Vec<String> = Vec::new();
            // BUG-1594: `git branch --contains` is one subprocess per
            // referencing commit, and a spec with a long commit trail (or
            // commits old enough that every agent branch contains them) made
            // this loop dominate `aida show`. Walk newest-first under a
            // commit cap AND a wall-clock budget; if either stops the walk,
            // record it so the renderer says the branch search is partial.
            // trace:BUG-1594 | ai:claude
            let scan_started = std::time::Instant::now();
            let mut scanned = 0usize;
            for (commit_full, _, _) in &commits {
                if scanned >= LINKAGE_BRANCH_SCAN_MAX_COMMITS
                    || scan_started.elapsed() >= LINKAGE_BRANCH_SCAN_BUDGET
                {
                    break;
                }
                scanned += 1;
                let Some(contains) = git(&[
                    "branch",
                    "--all",
                    "--contains",
                    commit_full,
                    "--format=%(refname:short)",
                ]) else {
                    continue;
                };
                // BUG-553: a commit can be reachable from MULTIPLE branches when a
                // later spec's branch was stacked on this one's unmerged commit
                // (the BUG-554 anti-pattern). Picking the first arbitrarily then
                // mis-attributes the spec to a sibling's branch (e.g. TASK-806
                // shown on `task-805`). Prefer the branch whose name matches one of
                // the spec ids being resolved (the spec's OWN branch, `TASK-806` →
                // `task-806`); fall back to the first only when none matches.
                // trace:BUG-553 | ai:claude
                //
                // BUG-720: also exclude the orphan `aida-store` branch. A commit
                // can be reachable ONLY from `aida-store` (its own bookkeeping
                // commit, or a cross-node store-lineage merge that names the spec
                // in parens) — never offer it as the spec's review branch, or
                // `aida review`/`aida human review` prompts to PR the entire
                // requirements store as a code change.
                for raw in contains.lines() {
                    let raw = raw.trim();
                    // BUG-1591: remember which candidates exist as a LOCAL
                    // branch — stripping `origin/` erased that, so selection
                    // fell to commit recency and a newer remote-only branch
                    // beat the spec's own local branch. trace:BUG-1591 | ai:claude
                    let is_remote = raw.starts_with("origin/") || raw.starts_with("remotes/");
                    let b = raw
                        .trim_start_matches("remotes/")
                        .trim_start_matches("origin/");
                    if !is_remote && !b.is_empty() && !local_candidates.iter().any(|c| c == b) {
                        local_candidates.push(b.to_string());
                    }
                    if !b.is_empty()
                        && b != "HEAD"
                        && b != "main"
                        && b != "master"
                        && !is_orphan_store_branch(b)
                        && !candidates.iter().any(|c| c == b)
                    {
                        candidates.push(b.to_string());
                    }
                }
            }
            if scanned < commits.len() {
                branch_scan_truncated = Some((scanned, commits.len()));
            }
            // BUG-1528: every candidate whose name matches the spec's own
            // id, in first-seen (recency) order. The first becomes `branch`;
            // any rest are a branch-crossing signal surfaced as
            // `other_branches` rather than silently dropped. trace:BUG-1528
            let mut id_matches: Vec<String> = candidates
                .iter()
                .filter(|b| {
                    let bl = b.to_ascii_lowercase();
                    ids.iter().any(|id| {
                        let nid = norm_id(id);
                        !nid.is_empty() && (bl == nid || bl.contains(&nid))
                    })
                })
                .cloned()
                .collect();
            // BUG-1591: a spec's own LOCAL branch outranks a remote-only one;
            // recency order is kept within each group (stable sort), so the
            // choice no longer depends on which commit git lists first.
            // trace:BUG-1591 | ai:claude
            id_matches.sort_by_key(|b| !local_candidates.iter().any(|c| c == b));
            if id_matches.is_empty() {
                // No candidate matches the spec's own id by name — fall back
                // to the first (newest) candidate found, as before.
                branch = candidates.into_iter().next();
            } else {
                branch = Some(id_matches.remove(0));
                other_branches = id_matches;
            }
            if let (Some(b), Some(wt)) =
                (branch.as_deref(), git(&["worktree", "list", "--porcelain"]))
            {
                let mut cur_path: Option<String> = None;
                for line in wt.lines() {
                    if let Some(p) = line.strip_prefix("worktree ") {
                        cur_path = Some(p.to_string());
                    } else if let Some(r) = line.strip_prefix("branch ") {
                        if r.trim_start_matches("refs/heads/") == b {
                            worktree = cur_path.clone();
                        }
                    }
                }
            }
        }
    }

    GitLinkage {
        commits,
        files,
        shipped,
        branch,
        other_branches,
        branch_scan_truncated,
        files_scan_incomplete,
        worktree,
        shipped_pr,
        // trace:STORY-634 | ai:claude — the scanned repo's workspace slug.
        repo: workspace_repo_slug(project_root),
    }
}

pub(crate) fn print_git_linkage(
    project_root: &std::path::Path,
    ids: &[String],
    verbose: bool,
    filed_at: Option<&aida_core::FilingProvenance>,
) {
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

    let GitLinkage {
        commits,
        files,
        shipped,
        branch,
        other_branches,
        branch_scan_truncated,
        files_scan_incomplete,
        worktree,
        shipped_pr,
        // trace:STORY-634 | ai:claude
        repo,
    } = collect_git_linkage(project_root, ids);

    if commits.is_empty() && files.is_empty() && files_scan_incomplete {
        // BUG-1594 (PRIN-5): an empty result from a scan that did not finish
        // is "unknown", not "no linkage" — never print the newcomer hint.
        // trace:BUG-1594 | ai:claude
        println!(
            "\n{}: {}",
            "Git linkage".green().bold(),
            "no referencing commits; trace-comment scan incomplete (time budget) — \
             traced files unknown"
                .yellow()
        );
        return;
    }
    if commits.is_empty() && files.is_empty() {
        // TASK-726: the link is AIDA's whole magic — it's what makes "recall why"
        // work — but a newcomer has no idea how to create one. When there's no
        // linkage yet, say exactly how, using their real spec id. Found by
        // running a novice's first session: `aida show` reported "no linkage"
        // and left them at a dead end. trace:TASK-726 | ai:claude
        let example_id = ids.first().map(|s| s.as_str()).unwrap_or("TASK-1");
        println!(
            "\n{}: {}",
            "Git linkage".green().bold(),
            "no commits or trace comments reference this spec yet".dimmed()
        );
        println!(
            "  {} Link your code to it: add a {} comment where you implement it, \
             or name {} in a commit message — then this fills in on its own.",
            crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
            format!("// trace:{example_id}").cyan(),
            format!("({example_id})").cyan(),
        );
        return;
    }

    println!("\n{}:", "Git linkage".green().bold());

    // ---- Branch / worktree / PR-or-MR (anchored on the newest commit) ----
    // STORY-511: resolve the forge once so the change-request line uses the
    // right noun ("PR"/"MR") and CLI ("gh"/"glab") — a GitLab repo now reads
    // "MR-47" + "glab not installed", not the GitHub-only wording. The
    // (label, value) text is produced by the pure `format_change_linkage`;
    // this block only adds color + the worktree/branch decoration that
    // depends on live data. trace:STORY-511 | ai:claude
    if !commits.is_empty() {
        let forge = crate::forge::resolve_forge_kind(project_root);
        // Helper: print a formatted change-linkage line list with styling.
        let render = |lines: Vec<(String, String)>, found_url: Option<&str>| {
            for (label, value) in lines {
                if label == "Branch" {
                    // "merged to main" is the only Branch line this path
                    // produces (in-flight Branch is rendered below with
                    // worktree decoration).
                    println!("  {}     {}", "Branch".bold(), value.green());
                } else if let Some(url) = found_url {
                    // An open change: split the trailing url so it can be dimmed.
                    let id_part = value.strip_suffix(url).map(|s| s.trim_end());
                    match id_part {
                        Some(id) => {
                            println!("  {}         {} {}", label.bold(), id.cyan(), url.dimmed())
                        }
                        None => println!("  {}         {}", label.bold(), value.cyan()),
                    }
                } else {
                    // Shipped change number, or a dimmed "unknown" diagnostic.
                    let is_diag = value.contains("unknown")
                        || value.starts_with("no ")
                        || value.contains("not installed");
                    if is_diag {
                        println!("  {}         {}", label.bold(), value.dimmed());
                    } else {
                        println!("  {}         {}", label.bold(), value.cyan());
                    }
                }
            }
        };

        if shipped {
            let mut lines =
                format_change_linkage(forge, &ChangeLinkageState::Shipped { number: shipped_pr });
            // STORY-634 (ADR-12 D5): in a multi-repo workspace a bare change
            // number / "merged to main" is ambiguous — qualify both with the
            // scanned repo's slug ("api#141"). Single-repo (`repo` = None)
            // renders exactly as before.
            if let Some(slug) = repo.as_deref() {
                for (label, value) in lines.iter_mut() {
                    if label == "Branch" {
                        *value = format!("{value} ({slug})");
                    } else if let Some(num) = shipped_pr {
                        *value = format!("{slug}#{num}");
                    }
                }
            }
            render(lines, None);
        } else {
            match branch.as_deref() {
                Some(b) => {
                    let wt = worktree
                        .map(|p| format!(" (worktree: {})", p))
                        .unwrap_or_default();
                    // STORY-634: name the repo the branch lives in when the
                    // store spans a multi-repo workspace; empty otherwise.
                    let repo_note = repo
                        .as_deref()
                        .map(|slug| format!(" · repo: {slug}"))
                        .unwrap_or_default();
                    println!(
                        "  {}     {}{}{} · {}",
                        "Branch".bold(),
                        b.cyan(),
                        wt,
                        repo_note,
                        "in flight".yellow()
                    );
                    // STORY-516: forge-routed lookup → STORY-511 forge-aware
                    // rendering. trace:STORY-516 trace:STORY-511 | ai:claude
                    // BUG-1594: every lookup is capped per call and the whole
                    // set shares one budget; a lookup the budget no longer
                    // covers is rendered as skipped, never as "no PR".
                    // trace:BUG-1594 | ai:claude
                    let forge_started = std::time::Instant::now();
                    let bounded_lookup = |branch_name: &str| {
                        if forge_started.elapsed() >= SHOW_FORGE_LOOKUP_BUDGET {
                            return (ChangeLinkageState::LookupSkipped, None);
                        }
                        change_linkage_state_for(with_forge_lookup_timeout(
                            FORGE_CLI_CALL_TIMEOUT,
                            || change_lookup_for_branch(project_root, branch_name),
                        ))
                    };
                    let (state, url) = bounded_lookup(b);
                    render(format_change_linkage(forge, &state), url.as_deref());
                    // BUG-1528 (AC3/AC4): more than one branch references
                    // this spec — a branch-crossing. Say so, with each
                    // sibling's own PR/MR state, rather than silently
                    // picking `b` above and leaving the rest invisible.
                    // trace:BUG-1528 | ai:claude
                    for other in &other_branches {
                        let (ostate, ourl) = bounded_lookup(other);
                        println!(
                            "  {}     {} {}",
                            "Branch".bold(),
                            other.cyan(),
                            "also references this spec".yellow()
                        );
                        render(format_change_linkage(forge, &ostate), ourl.as_deref());
                    }
                }
                None => render(
                    format_change_linkage(forge, &ChangeLinkageState::BranchNotFound),
                    None,
                ),
            }
            // BUG-1594 (PRIN-5): the branch search stopped early — say so,
            // so the branch lines above are never read as complete.
            // trace:BUG-1594 | ai:claude
            if let Some((scanned, total)) = branch_scan_truncated {
                println!(
                    "  {}",
                    format_branch_scan_truncated_note(scanned, total).yellow()
                );
            }
        }
    }

    // ---- Commits list ----
    if !commits.is_empty() {
        let cap = if verbose {
            commits.len()
        } else {
            5.min(commits.len())
        };
        println!(
            "  {} ({}{})",
            "Commits".bold(),
            commits.len(),
            if !verbose && commits.len() > cap {
                format!(", showing {}", cap)
            } else {
                String::new()
            }
        );
        for (full, short, subject) in commits.iter().take(cap) {
            println!("    {} {}", short.yellow(), subject);
            if verbose {
                if let Some(stat) =
                    git(&["show", "--shortstat", "--format=", full]).filter(|s| !s.is_empty())
                {
                    if let Some(last) = stat.lines().last() {
                        println!("      {}", last.trim().dimmed());
                    }
                }
            }
        }
        if !verbose && commits.len() > cap {
            println!(
                "    {}",
                format!("… {} more (--verbose to expand)", commits.len() - cap).dimmed()
            );
        }
    }

    // ---- Files traced ----
    if !files.is_empty() {
        println!("  {} ({})", "Files traced".bold(), files.len());
        for (file, symbol) in &files {
            match symbol {
                Some(sym) => println!("    {} — {}", file, sym.dimmed()),
                None => println!("    {}", file),
            }
        }
    }
    // BUG-1594 (PRIN-5): a partial trace walk is labeled, never passed off
    // as the complete file list. trace:BUG-1594 | ai:claude
    if files_scan_incomplete {
        println!("  {}", LINKAGE_TRACE_SCAN_INCOMPLETE_NOTE.yellow());
    }
    // TASK-1475 (CR-8 acceptance 6 follow-up): a one-line filing-drift
    // signal — how much of the traced surface has moved since the code
    // state this spec was filed against. Silent when there's nothing to
    // say (no provenance, unresolvable sha, no traced files, or no drift).
    // trace:TASK-1475 | ai:claude
    if let Some(hint) = filing_drift_hint(project_root, filed_at, &files, &commits) {
        println!("  {}", hint.dimmed());
    }
}

/// BUG-1594: the note `aida show` prints when the trace-comment walk stopped
/// at its time budget.
// trace:BUG-1594 | ai:claude
pub(crate) const LINKAGE_TRACE_SCAN_INCOMPLETE_NOTE: &str =
    "trace-comment scan incomplete (time budget) — more files may reference this spec";

/// Build the `## Reusable helpers` markdown section for `target` by walking
/// the requirement graph (siblings / tag-mates / same-feature specs) and
/// harvesting their `trace:` comments. Returns `None` when no related spec
/// contributes a named helper. Shared by `aida plan helpers` and the
/// `aida ultraplan` prompt assembler. trace:TASK-94 TASK-113 | ai:claude
pub(crate) fn build_reusable_helpers_section(
    store: &aida_core::RequirementsStore,
    project_root: &std::path::Path,
    target: &aida_core::models::Requirement,
) -> Option<String> {
    // ── Related-spec discovery ── first reason wins (sibling beats
    // feature beats tag) so each spec appears once with its strongest tie.
    let parent_uuids: Vec<uuid::Uuid> = target
        .relationships
        .iter()
        .filter(|r| r.rel_type == aida_core::RelationshipType::Child)
        .map(|r| r.target_id)
        .collect();
    let target_feature = target.feature.trim().to_string();
    let target_tags = target.tags.clone();

    let mut related: Vec<(&aida_core::models::Requirement, &'static str)> = Vec::new();
    let mut seen: HashSet<uuid::Uuid> = HashSet::new();
    seen.insert(target.id);

    if !parent_uuids.is_empty() {
        for req in &store.requirements {
            if seen.contains(&req.id) {
                continue;
            }
            let is_sibling = req.relationships.iter().any(|r| {
                r.rel_type == aida_core::RelationshipType::Child
                    && parent_uuids.contains(&r.target_id)
            });
            if is_sibling {
                seen.insert(req.id);
                related.push((req, "sibling"));
            }
        }
    }
    // Shared tags before feature: a shared tag is a deliberate grouping
    // (a `batch:`, a topic) and a stronger signal than `feature`, which
    // is often a coarse project-wide bucket.
    if !target_tags.is_empty() {
        for req in &store.requirements {
            if seen.contains(&req.id) {
                continue;
            }
            if req.tags.iter().any(|t| target_tags.contains(t)) {
                seen.insert(req.id);
                related.push((req, "shared tag"));
            }
        }
    }
    if !target_feature.is_empty() {
        let feature_matches: Vec<&aida_core::models::Requirement> = store
            .requirements
            .iter()
            .filter(|r| !seen.contains(&r.id) && r.feature.trim() == target_feature)
            .collect();
        // A feature shared by dozens of specs is a project-wide bucket,
        // not a meaningful relation — skip it rather than flood the
        // section with weakly-related noise.
        const FEATURE_DISCRIMINATION_CAP: usize = 25;
        if feature_matches.len() <= FEATURE_DISCRIMINATION_CAP {
            for req in feature_matches {
                seen.insert(req.id);
                related.push((req, "same feature"));
            }
        }
    }

    let target_display = target.display_id();
    if related.is_empty() {
        return None;
    }

    // Every id-string a trace comment might use (canonical spec_id or the
    // agreed short id) → index into `related`.
    let mut id_to_idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (idx, (req, _)) in related.iter().enumerate() {
        if let Some(s) = &req.spec_id {
            id_to_idx.insert(s.clone(), idx);
        }
        if let Some(a) = &req.agreed_id {
            id_to_idx.insert(a.clone(), idx);
        }
    }
    let wanted: HashSet<String> = id_to_idx.keys().cloned().collect();

    let hits = scan_trace_graph(project_root, &wanted);

    // Fold hits onto each related spec, deduped by (file, symbol).
    let mut per_spec: Vec<(usize, std::collections::BTreeMap<String, Vec<String>>)> = Vec::new();
    for (idx, _) in related.iter().enumerate() {
        per_spec.push((idx, std::collections::BTreeMap::new()));
    }
    for (id, id_hits) in &hits {
        let Some(&idx) = id_to_idx.get(id) else {
            continue;
        };
        let by_file = &mut per_spec[idx].1;
        for hit in id_hits {
            let syms = by_file.entry(hit.file.clone()).or_default();
            if let Some(sym) = &hit.symbol {
                if !syms.contains(sym) {
                    syms.push(sym.clone());
                }
            }
        }
    }

    // Keep only specs that introduce at least one *named* helper — the
    // section is "reusable helpers", and a bare file-touched hit (every
    // spec touches main.rs) carries no reuse signal. Rank by tier
    // (siblings are the closest relatives, then tag-mates, then the
    // coarse same-feature set) and within a tier by helper count, then
    // cap so the section stays a focused brief.
    const HELPERS_SPEC_CAP: usize = 12;
    const HELPERS_PER_FILE_CAP: usize = 8;
    let tier = |reason: &str| -> u8 {
        match reason {
            "sibling" => 0,
            "shared tag" => 1,
            _ => 2, // same feature — project-wide, weakest signal
        }
    };
    let mut ranked: Vec<(usize, u8, usize)> = per_spec
        .iter()
        .filter_map(|(idx, by_file)| {
            let symbols: usize = by_file.values().map(Vec::len).sum();
            (symbols > 0).then_some((*idx, tier(related[*idx].1), symbols))
        })
        .collect();
    ranked.sort_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    let truncated = ranked.len().saturating_sub(HELPERS_SPEC_CAP);
    ranked.truncate(HELPERS_SPEC_CAP);

    // No related spec names a helper → nothing worth a section.
    if ranked.is_empty() {
        return None;
    }

    // ── Render the section ──
    let mut md = String::new();
    md.push_str("## Reusable helpers (do not reimplement)\n\n");
    md.push_str(&format!(
        "Derived from the trace graph for {target_display} — related specs that already \
         touch this area. Call these rather than re-implementing.\n\n"
    ));

    for (idx, _, _) in &ranked {
        let (req, reason) = related[*idx];
        let by_file = &per_spec[*idx].1;
        md.push_str(&format!(
            "**{}** — {} · {} · {}\n\n",
            req.display_id(),
            req.title,
            reason,
            req.status
        ));
        for (file, syms) in by_file {
            if syms.is_empty() {
                md.push_str(&format!("- `{file}`\n"));
            } else {
                let mut sorted = syms.clone();
                sorted.sort();
                let extra = sorted.len().saturating_sub(HELPERS_PER_FILE_CAP);
                sorted.truncate(HELPERS_PER_FILE_CAP);
                let mut joined = sorted
                    .iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                if extra > 0 {
                    joined.push_str(&format!(", …+{extra}"));
                }
                md.push_str(&format!("- `{file}` — {joined}\n"));
            }
        }
        md.push('\n');
    }
    if truncated > 0 {
        md.push_str(&format!(
            "_…and {truncated} more related spec(s) — narrow the scope or inspect with \
             `aida list`._\n"
        ));
    }
    Some(md)
}

// ============================================================================
// TASK-113 — `aida ultraplan <SPEC>`. Assemble a rich, structured planning
// prompt from a spec's full context (description, acceptance, related specs,
// the AIDA plan template, trace-graph reusable helpers) so planners anchor on
// concrete requirements instead of a terse user prompt.
// trace:TASK-113 | ai:claude
// ============================================================================

/// The AIDA plan-template section list, inlined into the planning prompt so
/// the returned plan already matches `docs/plans/_TEMPLATE.md` (TASK-92) and
/// `aida plan verify` (TASK-93) passes on the saved plan.
pub(crate) const ULTRAPLAN_STRUCTURE: &str = "\
Produce the plan in AIDA's structured format — a header line with Date / \
Specs / Status / Complexity, then these sections in order:

1. **Approach** — one-paragraph executive summary, plus an ASCII diagram \
where a state machine or control flow would compress prose.
2. **Decisions** — each significant choice resolved, with rationale (not an \
open-questions list).
3. **Files (in build-order)** — symbol-anchored edits per file, ordered so \
each commit builds clean.
4. **Critical Files** — flat list of every must-touch path.
5. **Reusable helpers** — existing helpers to call rather than re-invent \
(seeded below).
6. **Risks + gotchas** — numbered, each with a mitigation.
7. **Tests** — named test functions, not \"add tests\".
8. **Verification** — an executable bash smoke test (positive + negative).
9. **Followups** — out-of-scope items to file as TASKs at completion time.
10. **Related** — builds-on / blocks / see-also links.";

/// One-line summary of a related requirement for the prompt's context block.
pub(crate) fn ultraplan_spec_line(req: &aida_core::models::Requirement) -> String {
    format!("{} — {} [{}]", req.display_id(), req.title, req.status)
}

/// Build the `## Comments` section for the ultraplan prompt — the spec's
/// enrichment comments, most recent first. `None` when the spec has no
/// non-empty comments. Caps both the comment count and the total
/// characters so a spec with a long discussion thread can't blow the
/// prompt budget; the richest planning context (long enrichment
/// comments) is exactly what TASK-247 exists to surface, so the budget
/// is deliberately generous. trace:TASK-247 | ai:claude
pub(crate) fn ultraplan_comments_section(
    comments: &[aida_core::models::Comment],
) -> Option<String> {
    const COMMENT_COUNT_CAP: usize = 20;
    const COMMENTS_CHAR_CAP: usize = 16_000;

    let non_empty = comments
        .iter()
        .filter(|c| !c.content.trim().is_empty())
        .count();
    if non_empty == 0 {
        return None;
    }

    let mut s = String::from(
        "## Comments\n\nPlanning context captured on the spec as comments, most recent first.\n\n",
    );
    let mut used = 0usize;
    let mut shown = 0usize;
    for c in comments.iter().rev() {
        let content = c.content.trim();
        if content.is_empty() {
            continue;
        }
        if shown >= COMMENT_COUNT_CAP || used >= COMMENTS_CHAR_CAP {
            break;
        }
        let reply_note = match c.replies.len() {
            0 => String::new(),
            n => format!(" · {n} repl{}", if n == 1 { "y" } else { "ies" }),
        };
        s.push_str(&format!(
            "### {} · {}{}\n\n",
            c.author,
            c.created_at.format("%Y-%m-%d"),
            reply_note
        ));
        let budget = COMMENTS_CHAR_CAP - used;
        if content.chars().count() > budget {
            let trimmed: String = content.chars().take(budget).collect();
            s.push_str(&trimmed);
            s.push_str("\n\n_(comment truncated to fit the prompt budget)_\n\n");
            used = COMMENTS_CHAR_CAP;
        } else {
            s.push_str(content);
            s.push_str("\n\n");
            used += content.chars().count();
        }
        shown += 1;
    }
    if shown < non_empty {
        s.push_str(&format!(
            "_(+{} older comment(s) omitted to fit the prompt budget)_\n\n",
            non_empty - shown
        ));
    }
    Some(s)
}

/// Assemble the AIDA planning prompt for `target`. `helpers_section` is the
/// pre-built trace-graph reusable-helpers markdown (None when there is
/// none — see TASK-94). `include_comments` pulls the spec's enrichment
/// comments into a `## Comments` section (TASK-247). Returns the prompt
/// and any warnings to surface. Pure over its inputs so it is
/// unit-testable. trace:TASK-113 TASK-247 | ai:claude
/// Assemble the rich, ultraplan-grade punt payload (STORY-306) a headless
/// advisor judges. The structured fork fields come from the punted spec's
/// recorded [`AttentionReason`](aida_core::AttentionReason) — what `aida
/// punt` captured — and the `context_markdown` body reuses the `aida
/// ultraplan` machinery (`assemble_ultraplan_prompt` + the trace-graph
/// `build_reusable_helpers_section`), so an advisor with **no session
/// context** has everything the implementer's planning prompt would have.
/// Split out as a deterministic, unit-testable transform — no session, no
/// subprocess. trace:STORY-306 | ai:claude
pub(crate) fn assemble_punt_payload(
    store: &aida_core::RequirementsStore,
    target: &aida_core::models::Requirement,
    project_root: &std::path::Path,
    attention: &aida_core::AttentionReason,
) -> punt::PuntRequest {
    let helpers = build_reusable_helpers_section(store, project_root, target);
    let (reservations, _reservation_warnings) = read_reserved_paths(project_root);
    let (context_markdown, _warnings) =
        assemble_ultraplan_prompt(store, target, helpers.as_deref(), true, &reservations);
    punt::PuntRequest {
        spec: target.display_id(),
        category: attention.category,
        question: attention.detail.clone(),
        // `aida punt` records the fork as free-form `detail` + `lean`; it
        // does not enumerate options / code-area / stakes separately, so
        // those structured fields stay empty until a future punt does.
        options: Vec::new(),
        code_area: None,
        stakes: None,
        lean: attention.lean.clone(),
        context_markdown,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ReservedPath {
    pub(crate) path: String,
    pub(crate) reason: String,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ReservedPathsFile {
    #[serde(default)]
    pub(crate) reservations: Vec<ReservedPath>,
}

pub(crate) fn read_reserved_paths(
    project_root: &std::path::Path,
) -> (Vec<ReservedPath>, Vec<String>) {
    let path = project_root.join(".aida").join("reserved-paths.toml");
    let Ok(body) = std::fs::read_to_string(&path) else {
        return (
            Vec::new(),
            vec![format!(
                "{} not found — reserved namespace guidance omitted",
                path.display()
            )],
        );
    };

    match toml::from_str::<ReservedPathsFile>(&body) {
        Ok(parsed) => (parsed.reservations, Vec::new()),
        Err(err) => (
            Vec::new(),
            vec![format!(
                "failed to parse {} — reserved namespace guidance omitted: {err}",
                path.display()
            )],
        ),
    }
}

pub(crate) fn reserved_paths_section(reservations: &[ReservedPath]) -> Option<String> {
    if reservations.is_empty() {
        return None;
    }

    let mut out = String::from("## Reserved namespaces and conventions\n\n");
    out.push_str(
        "Before proposing new files, check these project-reserved paths and avoid \
         collisions unless the plan explicitly updates the owning convention.\n\n",
    );
    for reservation in reservations {
        out.push_str(&format!(
            "- `{}` — {}\n",
            reservation.path.trim(),
            reservation.reason.trim()
        ));
    }
    out.push('\n');
    Some(out)
}

pub(crate) fn assemble_ultraplan_prompt(
    store: &aida_core::RequirementsStore,
    target: &aida_core::models::Requirement,
    helpers_section: Option<&str>,
    include_comments: bool,
    reservations: &[ReservedPath],
) -> (String, Vec<String>) {
    // TASK-247: raised from 1800 — a truncated description dropped the
    // densest planning context. Comments now carry the long-form
    // enrichment, but the description still deserves real room.
    const DESC_CHAR_CAP: usize = 16_000;
    const SIBLING_CAP: usize = 12;
    let mut warnings: Vec<String> = Vec::new();
    let display = target.display_id();
    let resolve = |id: &uuid::Uuid| store.requirements.iter().find(|r| &r.id == id);

    let mut p = String::new();
    p.push_str(&format!(
        "Plan the implementation of {display}: {}.\n\n",
        target.title
    ));

    // ── Requirement body ──
    p.push_str("## Requirement\n\n");
    let desc = target.description.trim();
    if desc.is_empty() {
        p.push_str("_(no description on the spec)_\n\n");
        warnings.push("spec has no description — the plan will be weakly grounded".to_string());
    } else if desc.chars().count() > DESC_CHAR_CAP {
        let truncated: String = desc.chars().take(DESC_CHAR_CAP).collect();
        p.push_str(&truncated);
        p.push_str("\n\n_(description truncated to fit the prompt budget)_\n\n");
    } else {
        p.push_str(desc);
        p.push_str("\n\n");
    }

    // ── Acceptance criteria ──
    p.push_str("## Acceptance criteria\n\n");
    match extract_acceptance_section(&target.description) {
        Some(acc) if !acc.trim().is_empty() => {
            p.push_str(acc.trim());
            p.push('\n');
        }
        _ => {
            p.push_str(
                "_(none specified — add a `## Acceptance` section to the spec for a sharper plan)_\n",
            );
            warnings.push(
                "spec has no `## Acceptance` section — the plan will be weaker without \
                 explicit acceptance criteria"
                    .to_string(),
            );
        }
    }
    p.push('\n');

    // ── Comments (enrichment context) ──
    // TASK-247: a spec's comments accumulate the densest planning
    // context (anti-patterns, design forks, reusable helpers). Pull them
    // in unless the caller opted out with `--no-comments`.
    if include_comments {
        if let Some(section) = ultraplan_comments_section(&target.comments) {
            p.push_str(&section);
        }
    }

    // TASK-517: surface reserved namespaces before the plan proposes files.
    // This catches likely collisions (notably docs/aida/) while the model is
    // still choosing the implementation shape.
    if let Some(section) = reserved_paths_section(reservations) {
        p.push_str(&section);
    }

    // ── Related-spec context ──
    let parents: Vec<&aida_core::models::Requirement> = target
        .relationships
        .iter()
        .filter(|r| r.rel_type == aida_core::RelationshipType::Child)
        .filter_map(|r| resolve(&r.target_id))
        .collect();
    let children: Vec<&aida_core::models::Requirement> = target
        .relationships
        .iter()
        .filter(|r| r.rel_type == aida_core::RelationshipType::Parent)
        .filter_map(|r| resolve(&r.target_id))
        .collect();
    // Other typed relationships (verifies / references / custom) — the
    // "composes / depends-on" signal the prompt should anchor on.
    let other_rels: Vec<(String, &aida_core::models::Requirement)> = target
        .relationships
        .iter()
        .filter(|r| {
            !matches!(
                r.rel_type,
                aida_core::RelationshipType::Parent | aida_core::RelationshipType::Child
            )
        })
        .filter_map(|r| resolve(&r.target_id).map(|req| (r.rel_type.to_string(), req)))
        .collect();
    let parent_ids: HashSet<uuid::Uuid> = parents.iter().map(|r| r.id).collect();
    let siblings: Vec<&aida_core::models::Requirement> = if parent_ids.is_empty() {
        Vec::new()
    } else {
        store
            .requirements
            .iter()
            .filter(|r| {
                r.id != target.id
                    && r.relationships.iter().any(|rel| {
                        rel.rel_type == aida_core::RelationshipType::Child
                            && parent_ids.contains(&rel.target_id)
                    })
            })
            .collect()
    };

    if !parents.is_empty() || !children.is_empty() || !siblings.is_empty() || !other_rels.is_empty()
    {
        p.push_str("## Related specs\n\n");
        for parent in &parents {
            p.push_str(&format!("- Parent: {}\n", ultraplan_spec_line(parent)));
        }
        for (rel, req) in &other_rels {
            p.push_str(&format!("- {}: {}\n", rel, ultraplan_spec_line(req)));
        }
        if !children.is_empty() {
            p.push_str("- Children:\n");
            for child in &children {
                p.push_str(&format!("  - {}\n", ultraplan_spec_line(child)));
            }
        }
        if !siblings.is_empty() {
            p.push_str(&format!(
                "- Siblings (share a parent — {} total):\n",
                siblings.len()
            ));
            for sib in siblings.iter().take(SIBLING_CAP) {
                p.push_str(&format!("  - {}\n", ultraplan_spec_line(sib)));
            }
            if siblings.len() > SIBLING_CAP {
                p.push_str(&format!(
                    "  - _(+{} more siblings omitted to fit the prompt budget)_\n",
                    siblings.len() - SIBLING_CAP
                ));
            }
        }
        p.push('\n');
    }

    // ── Plan structure ──
    p.push_str("## Plan structure\n\n");
    p.push_str(ULTRAPLAN_STRUCTURE);
    p.push_str("\n\n");

    // ── Reusable helpers (trace-graph derived) ──
    match helpers_section {
        Some(section) => {
            p.push_str(section);
            if !section.ends_with('\n') {
                p.push('\n');
            }
            p.push('\n');
        }
        None => {
            warnings.push(
                "no trace-graph reusable helpers found — Reusable helpers section omitted"
                    .to_string(),
            );
        }
    }

    // ── Style notes ──
    p.push_str("## Style notes\n\n");
    p.push_str(
        "- Prefer symbol refs (`fn handle_pull_command`) over line refs (`main.rs:19713`) — \
         line refs drift fast.\n\
         - Order the Files section so each commit builds clean.\n\
         - The Verification section must be an executable bash smoke test.\n\
         - Keep the Followups section to one TASK-title-sized line per item.\n",
    );

    (p, warnings)
}

/// Does the project's working tree have uncommitted changes? Conservative: any
/// `git status --porcelain` output (excluding the gitignored store/cache noise
/// git already omits) counts as dirty. trace:STORY-659
pub(crate) fn working_tree_is_dirty(project_root: &std::path::Path) -> bool {
    std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(project_root)
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

/// The curated "Getting started" set — the core spec loop a newcomer needs to
/// be productive on day one. Kept deliberately short so the default help reads
/// as approachable, not as a 40-command wall. trace:STORY-556
// STORY-758: the first screen LEADS with the magic — `aida why` (code↔decision)
// and the plain-markdown on-ramp — then the spec core. The autonomy verbs
// (zen/ship/solo) are earned depth: still in `aida help --all` and their groups,
// just not the newcomer's first impression. trace:STORY-758 | ai:claude
pub(crate) const GETTING_STARTED: &[(&str, &str)] = &[
    (
        "why",
        "Ask your code why it exists — the decision behind any line",
    ),
    (
        "init",
        "Set up AIDA (`init --minimal` = a folder of markdown, zero setup)",
    ),
    ("show", "Show a spec — its intent, status, and links"),
    ("add", "Capture a new spec (a title + a line of why)"),
    ("list", "List specs"),
    ("search", "Search specs"),
    ("graph", "Trace impact — what a change affects"),
];

/// The full grouped command surface, organized by function. Used by both the
/// tiered help (group headings only, no command rows) and `aida help --all`
/// (every row). Dev-only / orchestrator-internal commands live in their own
/// trailing section so they stay out of the novice's eyeline. trace:STORY-556
pub(crate) fn command_groups() -> &'static [(&'static str, &'static [(&'static str, &'static str)])]
{
    &[
        (
            "Getting started",
            &[
                // STORY-758: the magic leads here too.
                (
                    "why",
                    "Ask your code why it exists — the decision behind any line",
                ),
                ("init", "Set up AIDA in the current project"),
                ("add", "Add a new requirement"),
                ("list", "List requirements"),
                ("show", "Show details for one requirement"),
                ("done", "Mark a spec done — the simple \"I finished it\""),
            ],
        ),
        (
            "Specs",
            &[
                ("edit", "Edit an existing requirement"),
                ("del", "Delete a requirement"),
                ("search", "Search requirements (case-insensitive)"),
                ("grep", "Advanced regex search across requirements"),
                ("comment", "Manage comments on requirements"),
                ("graph", "Query the cross-spec relationship graph"),
                ("rel", "Relationship management"),
                ("trace", "Code-to-requirement traceability"),
                ("archive", "Hide a requirement from default views"),
                ("unarchive", "Restore an archived requirement"),
                // trace:BUG-520 — defer/undefer were real commands missing from any group
                ("defer", "Park a spec as primed/conditional work"),
                ("undefer", "Restore a deferred spec to the active view"),
                ("lint", "EARS-style requirement quality lens"),
            ],
        ),
        (
            "Work & autonomy",
            &[
                ("queue", "Personal work queue"),
                ("backlog", "Backlog grooming views"),
                (
                    "human",
                    "Human-attention views + the unblock grooming prompt",
                ),
                ("rework", "Send a spec back for rework"),
                ("burndown", "Autonomous backlog burn-down"),
                // trace:TASK-879 | ai:claude — the solo loop alongside the other drain verbs.
                (
                    "solo",
                    "Run your project solo — groom → implement → integrate, end-to-end",
                ),
                ("findings", "Triage findings filed by drain phases"),
                ("punt", "Pause a spec at a design-fork"),
                ("questions", "The async decision inbox"),
                ("brief", "Route work to an agent via a pickup brief"),
                ("agent", "Supervised agent launcher"),
                ("triage", "Triage incoming work"),
            ],
        ),
        (
            "Git & lifecycle",
            &[
                ("fetch", "Refresh remote refs (read-only)"),
                ("pull", "Pull code + store; auto-bump done → completed"),
                ("push", "Push code + store"),
                ("rebase", "Rebase code + store"),
                ("pr", "Pull-request helpers"),
                ("review", "Send a held spec for human review"),
                ("changelog", "Refresh / generate CHANGELOG.md"),
            ],
        ),
        (
            "Planning",
            &[
                ("plan", "Verify plans + derive reusable-helper sections"),
                ("ultraplan", "Assemble a rich planner prompt from a spec"),
                ("import-plan", "Import a saved plan under docs/plans/"),
                ("goal", "Derive a machine-checkable /goal condition"),
                ("deps", "Dependency views + trace sweep"),
            ],
        ),
        (
            "Roles & sessions",
            &[
                ("role", "Per-shell role identity (advisor / implementer)"),
                ("session", "Work sessions + worktree leases"),
                ("mailbox", "Inter-agent peer↔peer messaging"),
                ("node", "Per-clone node identity"),
                ("advisor", "Live-advisor registration"),
            ],
        ),
        (
            "Project setup & maintenance",
            &[
                ("scaffold", "Scaffolding management (skills, hooks, MCP)"),
                ("config", "ID configuration (prefixes, formats, etc.)"),
                ("type", "Requirement-type management"),
                ("feature", "Feature management"),
                ("memories", "Starter memory-pack drift check"),
                ("docs", "Project documentation management"),
                ("statusline", "AIDA-aware statusline setup"),
                // trace:BUG-520 — rules/doctor/remote were real commands missing from any
                // group; release/upgrade are maintainer verbs better placed here than under
                // Git & lifecycle.
                (
                    "rules",
                    "Sync Claude Code path-gated rules from the spec graph",
                ),
                ("doctor", "Diagnose and heal multi-agent state drift"),
                ("remote", "Set up a git origin for a project that has none"),
                ("release", "Manage a release"),
                ("upgrade", "Upgrade aida to the latest release"),
                ("export", "Export requirements"),
                ("import", "Import requirements from a tree JSON file"),
            ],
        ),
        (
            "Reporting",
            &[
                ("status", "Where am I right now — unified project view"),
                ("history", "Recent activity"),
                ("report", "Report generation"),
                ("digest", "Narrative advisor report"),
                ("usage", "Inspect locally-recorded CLI usage"),
                ("metrics", "Agent-lift metrics over telemetry"),
                (
                    "criteria",
                    "Acceptance-criteria trace coverage for one spec",
                ),
                ("why", "Explain a spec's current state"),
                // trace:STORY-631 — AI WHY-comprehension, complementary to `why`.
                (
                    "intent",
                    "Plain-terms AI comprehension of why a spec exists",
                ),
                ("user-guide", "Open the user guide in the default browser"),
                // trace:STORY-600 — the CLI manual's when/why beside --help's what.
                ("manual", "Print a command's CLI-manual rationale section"),
                // trace:STORY-667 — the discoverable registry of built-in shortcuts.
                ("alias", "List AIDA's built-in shortcuts"),
            ],
        ),
        (
            "Integrations & servers",
            &[
                ("server", "Connect to or manage a remote AIDA server"),
                ("mcp-serve", "Start MCP server over stdio (for Claude Code)"),
                ("github", "GitHub Issues integration"),
                ("gitlab", "GitLab Issues integration"),
                ("jira", "Jira integration"),
            ],
        ),
        (
            "Storage & data",
            &[
                ("db", "Database management (migrate, sync, merge-gate)"),
                ("cache", "SQLite cache view (rebuild, status)"),
            ],
        ),
        (
            "Working on aida itself",
            &[
                (
                    "dev",
                    "Activate dev binary, run dev servers, install shell helpers",
                ),
                (
                    "help-all",
                    "Full inventory grouped by topic (same as `help --all`)",
                ),
            ],
        ),
    ]
}

/// Bare `aida` and `aida help`: lead with the small "Getting started" set, then
/// list the group headings so the depth is visible but not in the newcomer's
/// face. Full rows are one step away via `aida help --all`. trace:STORY-556
pub(crate) fn print_tiered_help() {
    // STORY-758: lead with the magic, matching the README front door.
    println!("{}", "AIDA — ask your codebase why".bold());
    println!(
        "{}",
        "Point at any line of code and get the decision behind it.".dimmed()
    );
    println!();
    println!("{}", "Usage: aida <command> [options]".dimmed());
    println!();

    println!("{}", "Getting started".cyan().bold());
    for (name, desc) in GETTING_STARTED {
        println!("  {:<10} {}", name.green(), desc);
    }
    // The TUI is the flagship surface (EPIC-26) — invite it from the first
    // screen, but only when it's actually compiled in so a --no-default-features
    // build doesn't list a command that errors. trace:SPIKE-66 | ai:claude
    #[cfg(feature = "tui")]
    println!(
        "  {:<10} {}",
        "tui".green(),
        "Launch the terminal UI — the at-a-glance home for your project"
    );
    println!();

    println!("{}", "More, grouped by topic".cyan().bold());
    // Skip "Getting started" (already shown above) and the dev-only group.
    // STORY-723: render the topic HEADERS bold (not command-green) — green is
    // the runnable-command colour used for the Getting-started rows above, and
    // styling a non-runnable header the same way invited `aida Specs` (which
    // errors). Bold reads as a section label, distinct from a command.
    for (group, _cmds) in command_groups()
        .iter()
        .filter(|(g, _)| *g != "Getting started" && *g != "Working on aida itself")
    {
        println!("  {}", group.bold());
    }
    println!();

    println!(
        "Run {} for the full command list, or {} for one command's options.",
        "`aida help --all`".bold(),
        "`aida <command> --help`".bold()
    );
    // STORY-758: the magic is the entry point; `aida status` is for orientation
    // once you're set up.
    println!(
        "{} — point at code, get the decision behind it.  {} for \"what's going on here?\"",
        "`aida why <file:line>`".bold(),
        "`aida status`".bold()
    );
    // trace:SPIKE-66 | ai:claude
    #[cfg(feature = "tui")]
    println!(
        "{} opens the visual home for your project — at a glance, all at once.",
        "`aida tui`".bold()
    );
}

pub(crate) fn print_help_all() {
    let groups = command_groups();

    println!(
        "{}",
        "AIDA — full command inventory (run `aida <command> --help` for details)".bold()
    );
    println!();
    for (group, cmds) in groups {
        println!("{}", group.cyan().bold());
        for (name, desc) in *cmds {
            println!("  {:<14} {}", name.green(), desc);
        }
        println!();
    }
    println!(
        "Bare `aida` / `aida help` shows the curated Getting-started view. {}",
        "Tip:".bold()
    );
    println!("  - `aida help <topic>` expands a single group (e.g. `aida help queue`)");
    // trace:TASK-1098 | ai:claude
    println!("  - `aida help commands` lists EVERY command and subcommand, one line each");
    println!("  - `aida <topic> --help` works for any command, even hidden ones");
    println!("  - `aida status` is the best entry point for \"what's going on here?\"");
    println!("  - `aida dev shell-init --install` to wire up the `aida` shell wrapper");
}

/// Resolve a user-supplied `<topic>` to one command group. Matches a group name
/// case-insensitively; falls back to a unique case-insensitive prefix so
/// `aida help git` resolves "Git & lifecycle". Returns `Err(Vec)` (the valid
/// topic names) when nothing matches or the prefix is ambiguous.
/// trace:TASK-861 | ai:claude
#[allow(clippy::type_complexity)]
pub(crate) fn resolve_help_topic(
    topic: &str,
) -> Result<(&'static str, &'static [(&'static str, &'static str)]), Vec<&'static str>> {
    let groups = command_groups();
    let needle = topic.trim().to_lowercase();

    // Exact (case-insensitive) name match wins outright.
    if let Some((g, cmds)) = groups.iter().find(|(g, _)| g.to_lowercase() == needle) {
        return Ok((*g, *cmds));
    }

    // Otherwise accept a UNIQUE case-insensitive prefix.
    let matches: Vec<&(&str, &[(&str, &str)])> = groups
        .iter()
        .filter(|(g, _)| g.to_lowercase().starts_with(&needle))
        .collect();
    if matches.len() == 1 {
        let (g, cmds) = matches[0];
        return Ok((*g, *cmds));
    }

    Err(groups.iter().map(|(g, _)| *g).collect())
}

/// `aida help <topic>`: expand one command group — its commands + descriptions —
/// the middle granularity between bare `aida help` (group names only) and
/// `aida help --all` (every group). An unknown/ambiguous topic errors with the
/// list of valid topic names rather than silently falling through.
/// trace:TASK-861 | ai:claude
pub(crate) fn print_help_topic(topic: &str) -> Result<()> {
    match resolve_help_topic(topic) {
        Ok((group, cmds)) => {
            println!("{}", group.cyan().bold());
            for (name, desc) in cmds {
                println!("  {:<14} {}", name.green(), desc);
            }
            println!();
            println!(
                "Run {} for one command's options, or {} for every group.",
                "`aida <command> --help`".bold(),
                "`aida help --all`".bold()
            );
            Ok(())
        }
        Err(valid) => {
            let project_root = find_main_worktree_root()
                .or_else(|_| find_project_root())
                .ok();
            if help_catalog::print_semantic_help_topic(topic, project_root.as_deref()) {
                return Ok(());
            }
            eprintln!();
            eprintln!("Unknown help topic: {}", topic.bold());
            eprintln!();
            eprintln!("Valid topics (try `aida help <topic>`):");
            for name in valid {
                eprintln!("  {}", name.green());
            }
            eprintln!();
            // trace:TASK-1098 | ai:claude
            eprintln!(
                "Or `aida help` for the curated view, {} for everything, {} for the flat catalog.",
                "`aida help --all`".bold(),
                "`aida help commands`".bold()
            );
            anyhow::bail!("unknown help topic: {topic}");
        }
    }
}

#[cfg(test)]
#[path = "tests/help_grouping_tests.rs"]
mod help_grouping_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShaMatch {
    Exact,
    Ancestor,
    Unrelated,
    Unknown,
}

/// TASK-221: shell out to `git rev-parse HEAD` inside the repo. Returns
/// the full 40-char SHA, or None if git is unavailable or HEAD missing.
pub(crate) fn current_branch_head_sha(repo: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// TASK-221: extract the SHA embedded by build.rs from a binary's
/// --version output. The banner shape is:
///   "aida X.Y.Z (built TIMESTAMP, sha SHA[+dirty])"
/// Returns None if the binary is missing, refuses to run, or the banner
/// shape changed.
pub(crate) fn binary_embedded_sha(binary: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new(binary)
        .arg("--version")
        .output_retrying_etxtbsy()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout);
    parse_embedded_sha(&s)
}

/// Pure parser for the --version banner SHA. Split out so the unit tests
/// don't need a real binary. trace:TASK-221 | ai:claude
pub(crate) fn parse_embedded_sha(banner: &str) -> Option<String> {
    // Match "sha <HEX>" — accept 7+ hex chars; trim trailing "+dirty" or
    // ")" or whitespace.
    let lower = banner.to_ascii_lowercase();
    let idx = lower.find("sha ")?;
    let after = &banner[idx + 4..];
    let mut end = 0;
    for (i, c) in after.char_indices() {
        if c.is_ascii_hexdigit() {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    if end < 7 {
        return None;
    }
    Some(after[..end].to_string())
}

/// TASK-221: classify a binary's embedded SHA against current HEAD.
/// - Exact when the SHAs match (treating the binary's SHA as a prefix of
///   HEAD, since build.rs may stamp a short SHA).
/// - Ancestor when `git merge-base --is-ancestor <binary-sha> HEAD` says
///   so — current branch is ahead of the build, but they're related.
/// - Unrelated otherwise (likely a different branch's build).
/// - Unknown when git isn't available (graceful fallback).
pub(crate) fn classify_sha_match(
    repo: &std::path::Path,
    binary_sha: &str,
    head_sha: &str,
) -> ShaMatch {
    let bin_lower = binary_sha.to_ascii_lowercase();
    let head_lower = head_sha.to_ascii_lowercase();
    if head_lower.starts_with(&bin_lower) || bin_lower.starts_with(&head_lower) {
        return ShaMatch::Exact;
    }
    // git merge-base --is-ancestor returns exit 0 when the first commit
    // is an ancestor of the second.
    //
    // BUG-702: use `.output()` (not `.status()`) so git's stderr is CAPTURED,
    // not inherited — a `binary_sha` that a history rewrite purged (force-push
    // + gc) makes git print `fatal: Not a valid object name <sha>`, which would
    // otherwise leak into `aida pull` output. trace:BUG-702 | ai:claude
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", binary_sha, head_sha])
        .output();
    match output {
        Ok(o) => classify_from_merge_base_exit(o.status.code()),
        Err(_) => ShaMatch::Unknown,
    }
}

/// BUG-702: map `git merge-base --is-ancestor` exit code → [`ShaMatch`]. Exit
/// `0` = ancestor; `1` = a clean "not an ancestor" (both commits resolved);
/// anything else — notably `128` = bad/missing object from a purged SHA, or git
/// unavailable — is Unknown: the SHA can't be placed, so no stale-binary nudge
/// and (with the captured stderr above) no leaked git fatal. A purged SHA is
/// genuinely Unknown, NOT a misleading Unrelated. Pure so the exit-code
/// contract is unit-testable.
// trace:BUG-702 | ai:claude
pub(crate) fn classify_from_merge_base_exit(code: Option<i32>) -> ShaMatch {
    match code {
        Some(0) => ShaMatch::Ancestor,
        Some(1) => ShaMatch::Unrelated,
        _ => ShaMatch::Unknown,
    }
}

/// BUG-665: pure predicate — should `aida pull` warn that the built binary is
/// now stale after the code leg advanced? True only when the in-repo build is
/// dev-activated AND its embedded SHA is a STRICT ancestor of HEAD (genuinely
/// behind the freshly-pulled changes). A non-dev-activated (released) binary on
/// PATH is expected to differ, so no warning; a binary that already matches
/// HEAD ([`ShaMatch::Exact`]) or has diverged ([`ShaMatch::Unrelated`] /
/// [`ShaMatch::Unknown`]) does NOT get this "run cargo build" nudge. Reuses the
/// same SHA classification `aida dev status` shows.
// trace:BUG-665 | ai:claude
pub(crate) fn pull_binary_is_stale(dev_activated: bool, verdict: ShaMatch) -> bool {
    dev_activated && matches!(verdict, ShaMatch::Ancestor)
}

/// BUG-665: after a successful `aida pull` code leg, warn when a dev-activated
/// in-repo binary is now behind the freshly-pulled HEAD — so `aida tui` /
/// `aida integrate` don't silently keep running old code until the next
/// `cargo build`. No-op when not dev-activated, when the binary already matches
/// HEAD, or when the pulled repo isn't the one the active binary was built from.
/// Best-effort: any missing signal (env unset, git/binary unavailable) → silent.
// trace:BUG-665 | ai:claude
pub(crate) fn warn_if_pulled_binary_stale(project_root: &std::path::Path) {
    // Dev-activation: an in-repo build is on PATH. Same signals `aida dev
    // status` reads (AIDA_DEV_ACTIVE + AIDA_DEV_BIN + AIDA_DEV_REPO).
    let dev_activated = std::env::var("AIDA_DEV_ACTIVE").is_ok();
    if !dev_activated {
        return;
    }
    let (Ok(bin_dir), Ok(repo)) = (
        std::env::var("AIDA_DEV_BIN"),
        std::env::var("AIDA_DEV_REPO"),
    ) else {
        return;
    };
    let repo_path = std::path::PathBuf::from(&repo);
    // Only reason about the binary when the repo we just pulled IS the one the
    // active binary was built from — otherwise the SHAs aren't comparable and a
    // warning would be a false alarm.
    let same_repo = match (project_root.canonicalize(), repo_path.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !same_repo {
        return;
    }
    let aida_path = std::path::PathBuf::from(&bin_dir).join("aida");
    let head = current_branch_head_sha(&repo_path);
    let bin_sha = binary_embedded_sha(&aida_path);
    if let (Some(h), Some(b)) = (head.as_deref(), bin_sha.as_deref()) {
        let verdict = classify_sha_match(&repo_path, b, h);
        // trace:TASK-1719 | ai:antigravity
        if pull_binary_is_stale(dev_activated, verdict) {
            eprintln!();
            eprintln!(
                "  {} your aida binary is now behind HEAD — run `make build-fast` to pick up the pulled changes.",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
            );
        }
    }
}

/// TASK-667: render a runtime hint for one of the shell-modifying subcommands
/// (`role enter <role>`, `role end`, `role add <name>`, `session start`,
/// `session end`, `dev activate`, `dev deactivate`) in the form the caller's
/// shell will actually honor.
///
/// The `aida()` wrapper installed by `aida dev shell-init` auto-evals these
/// subcommands, so under the wrapper the BARE form (`aida role enter <role>`)
/// is correct — the function evals the binary's stdout. Printing
/// `eval "$(aida role enter <role>)"` there double-evals (the inner eval runs
/// in a subshell) and the shell change is silently lost — the long-standing
/// double-eval footgun.
///
/// Without the wrapper (raw binary on PATH), the bare form would just print
/// shell code that never executes, so the `eval "$(...)"` form is required.
///
/// The wrapper signals its presence via `AIDA_SHELL_WRAPPER` (exported from
/// SHELL_HELPERS), which is how we branch.
pub(crate) fn eval_subcommand_hint(subcommand: &str) -> String {
    if std::env::var_os("AIDA_SHELL_WRAPPER").is_some() {
        format!("aida {subcommand}")
    } else {
        format!("eval \"$(aida {subcommand})\"")
    }
}

/// STORY-472: resolve the bump kind from the mutually-exclusive
/// `--patch`/`--minor`/`--major` flags. None set → "patch" (smallest, safe
/// default); more than one set → a clean error. trace:STORY-472 | ai:claude
pub(crate) fn resolve_release_bump(
    patch: bool,
    minor: bool,
    major: bool,
) -> Result<&'static str, String> {
    match (patch, minor, major) {
        (_, false, false) => Ok("patch"), // explicit --patch or nothing
        (false, true, false) => Ok("minor"),
        (false, false, true) => Ok("major"),
        _ => Err("specify only one of --patch / --minor / --major".to_string()),
    }
}

/// STORY-472: compute the next semver from `current` for a patch/minor/major
/// bump (lower components reset to 0), for the `--check` preview. Returns None
/// if `current` isn't a plain MAJOR.MINOR.PATCH. trace:STORY-472 | ai:claude
pub(crate) fn preview_next_version(current: &str, bump: &str) -> Option<String> {
    let core = current.split(['-', '+']).next().unwrap_or(current);
    let mut it = core.split('.');
    let maj: u64 = it.next()?.parse().ok()?;
    let min: u64 = it.next()?.parse().ok()?;
    let pat: u64 = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some(match bump {
        "minor" => format!("{maj}.{}.0", min + 1),
        "major" => format!("{}.0.0", maj + 1),
        _ => format!("{maj}.{min}.{}", pat + 1),
    })
}

/// STORY-472: `aida release` — a memorable top-level verb over the dev release
/// flow. `--check` previews the planned release (read-only); otherwise it
/// delegates to `handle_dev_release`, forwarding `--skip-xplat-check` via the
/// env the release script honors. trace:STORY-472 | ai:claude
/// STORY-527: `aida burndown` dispatch.
/// STORY-560: `aida intake` — the headless advisor INTAKE pass. The
/// advisor-side analog of `handle_burndown_run`: load the `[intake]` policy +
/// flag overrides, self-load the store, compute the BOUNDED candidate fence
/// (the do-not-approve classes + `needs-human`/`strategic` specs are excluded
/// TASK-1147: the advisor-autopilot auditability + reversal surface.
///
/// This is the read-only + audit + reversal HALF of EPIC-0428. `inspect`
/// dry-runs the four-gate policy envelope over the live groom candidates and
/// shows, per spec, what it WOULD decide — without touching a single spec.
/// `audit` lists recorded verdicts; `challenge` reverses one. Autopilot has NO
/// authority to approve/reject/queue here — the envelope is never wired to
/// execute a disposition. That keystone-autonomy slice is deferred.
// trace:TASK-1147 | ai:claude
pub(crate) fn handle_autopilot_command(cmd: &crate::cli::AutopilotCommand) -> Result<()> {
    use crate::cli::AutopilotCommand;
    match cmd {
        AutopilotCommand::Inspect {
            risk,
            only_tag,
            exclude_tag,
            record,
            json,
        } => handle_autopilot_inspect(
            risk,
            only_tag.as_deref(),
            exclude_tag.as_deref(),
            *record,
            *json,
        ),
        AutopilotCommand::Audit { limit, open, json } => {
            handle_autopilot_audit(*limit, *open, *json)
        }
        AutopilotCommand::Challenge { target, note } => {
            handle_autopilot_challenge(target, note.as_deref())
        }
        // TASK-1018: the DURABLE half — what autopilot actually did, and the
        // one command that undoes it.
        AutopilotCommand::Executions {
            limit,
            open,
            from_product,
            mode,
            json,
        } => handle_autopilot_executions(*limit, *open, *from_product, mode.as_deref(), *json),
        AutopilotCommand::Revert {
            target,
            dry_run,
            note,
            json,
        } => handle_autopilot_revert(target, *dry_run, note.as_deref(), *json),
        AutopilotCommand::Reindex { dry_run } => handle_autopilot_reindex(*dry_run),
    }
}

/// `aida autopilot reindex` — refill the per-clone execution index from the
/// durable audit comments on the specs. The comments are the source of truth;
/// the index is rebuildable, exactly like `.aida/cache.db` vs the orphan-branch
/// YAML.
// trace:TASK-1018 | ai:claude
pub(crate) fn handle_autopilot_reindex(dry_run: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let indexed = autopilot_audit::read_executions(&project_root)?;
    let recovered = autopilot_audit::recover_from_comments(&project_root)?;
    let missing = autopilot_audit::reindex_missing(&indexed, &recovered);

    if missing.is_empty() {
        println!(
            "{} the index already holds every recorded autopilot action ({} recovered, {} indexed).",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            recovered.len(),
            indexed.len()
        );
        return Ok(());
    }
    if !dry_run {
        autopilot_audit::append_executions(&project_root, &missing)?;
    }
    println!(
        "{} {} {} action(s) recovered from the durable trail:",
        if dry_run {
            "·".dimmed().to_string()
        } else {
            crate::glyph(crate::glyphs::Glyph::Check)
                .green()
                .to_string()
        },
        if dry_run { "would restore" } else { "restored" },
        missing.len()
    );
    for rec in &missing {
        println!(
            "    {} {}  {:<12} {}",
            "→".green(),
            rec.id.cyan(),
            rec.spec_id,
            rec.action
        );
    }
    Ok(())
}

/// `aida autopilot executions` — list the actions autopilot actually EXECUTED
/// (as opposed to `audit`'s dry-run projection), newest last.
///
/// TASK-1013: `--from-product` narrows the list to the actions whose evidence
/// recorded a product handoff. Product input is evidence, never authority — the
/// filter exists so an operator can see at a glance whether a non-privileged
/// product seat is steering what autopilot dispositions.
///
/// TASK-1014: `--mode` narrows to the actions taken under one composition mode
/// — the supervision context (`autopilot` alone, `zen+autopilot`,
/// `solo+autopilot`) that was in effect when the action landed.
// trace:TASK-1018 trace:TASK-1013 trace:TASK-1014 | ai:claude
pub(crate) fn handle_autopilot_executions(
    limit: usize,
    open_only: bool,
    from_product: bool,
    mode: Option<&str>,
    json: bool,
) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let executions = autopilot_audit::read_executions(&project_root)?;
    let reversals = autopilot_audit::read_reversals(&project_root)?;

    let mut rows: Vec<&autopilot_audit::ExecutionRecord> = executions.iter().collect();
    if open_only {
        rows.retain(|e| !autopilot_audit::is_reversed(&reversals, &e.id));
    }
    // Narrow BEFORE the limit so `--from-product --limit N` means "the last N
    // product-sourced actions", not "the product-sourced ones among the last N".
    // trace:TASK-1013 | ai:claude
    if from_product {
        rows.retain(|e| autopilot_audit::is_from_product(e));
    }
    // Same rule for the composition filter: narrow first, cap second.
    // trace:TASK-1014 | ai:claude
    if let Some(wanted) = mode {
        rows.retain(|e| autopilot_audit::mode_matches(e, wanted));
    }
    if limit > 0 && rows.len() > limit {
        rows = rows.split_off(rows.len() - limit);
    }

    if json {
        let out: Vec<_> = rows
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "ts": e.ts,
                    "spec_id": e.spec_id,
                    "action": e.action,
                    "authority": e.authority,
                    "gate": e.gate,
                    "grounding": e.grounding,
                    "risk": e.risk,
                    "actor": e.actor,
                    "source": e.source,
                    "reason": e.reason,
                    "evidence": e.evidence,
                    // `mode` is the raw recorded field; `composition` is the
                    // mode the filter and the table report (the field, else the
                    // surface it is derived from on a pre-`mode` row).
                    // trace:TASK-1014 | ai:claude
                    "mode": e.mode,
                    "composition": autopilot_audit::record_mode(e),
                    // `from_product` is the raw recorded field; `is_from_product`
                    // is the filter's verdict (flag, else evidence markers), and
                    // `product` names the seat when the marker carries a who.
                    // trace:TASK-1013 | ai:claude
                    "from_product": e.from_product,
                    "is_from_product": autopilot_audit::is_from_product(e),
                    "product": autopilot_audit::product_provenance(&e.evidence),
                    // The composition of the two: product input consumed with
                    // nobody in the loop. Evidence only — no authority came with
                    // it — but it is the row to review first after an unattended
                    // drain. trace:TASK-1022 | ai:claude
                    "unattended_product": autopilot_audit::unattended_product_decision(e),
                    "prior": e.prior,
                    "reverted": autopilot_audit::is_reversed(&reversals, &e.id),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&serde_json::json!(out))?);
        return Ok(());
    }

    if rows.is_empty() {
        if let Some(wanted) = mode {
            // trace:TASK-1014 | ai:claude
            println!(
                "{} no autopilot execution here ran under `{wanted}`.",
                "·".dimmed()
            );
        } else if from_product {
            println!(
                "{} no autopilot execution here acted on a product handoff.",
                "·".dimmed()
            );
        } else {
            println!(
                "{} autopilot has not executed any action here yet.",
                "·".dimmed()
            );
        }
        return Ok(());
    }

    println!(
        "{} autopilot executions — {} {}action(s)",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
        rows.len(),
        // Name whatever narrowed the list, so a short table is never mistaken
        // for a quiet trail. trace:TASK-1014 | ai:claude
        match (from_product, mode) {
            (true, Some(m)) => format!("product-sourced {m} "),
            (true, None) => "product-sourced ".to_string(),
            (false, Some(m)) => format!("{m} "),
            (false, None) => "recorded ".to_string(),
        }
    );
    for e in &rows {
        let reverted = autopilot_audit::is_reversed(&reversals, &e.id);
        let mark = if reverted {
            "↩ reverted".yellow().to_string()
        } else {
            "· in effect".dimmed().to_string()
        };
        println!(
            "  {} {}  {:<12} {:<8} {:<9} {}  {}",
            e.id.cyan(),
            e.ts.dimmed(),
            e.spec_id,
            e.action,
            e.authority,
            e.source.dimmed(),
            mark
        );
        if let Some(restore) = e.prior.describe() {
            println!("      {}", format!("restores: {restore}").dimmed());
        }
        // Only a COMPOSED mode is annotated: the bare envelope is the default
        // and needs no comment, so calling it out on every row would bury the
        // rows where something else was steering. trace:TASK-1014 | ai:claude
        if let Some(composition) = autopilot_audit::mode_annotation(e) {
            println!("      {}", composition.blue());
        }
        // The handoff is visible on every listing, not just under the filter —
        // an operator scanning the trail should not have to know to ask.
        // trace:TASK-1013 | ai:claude
        if let Some(handoff) = autopilot_audit::product_annotation(e) {
            println!("      {}", handoff.magenta());
        }
        // Loudest of the three, and only when both layers are present: a
        // non-privileged seat's input was consumed with the least supervision
        // the system offers. The gates guarantee it granted nothing; this line
        // guarantees the operator can still SEE it.
        // trace:TASK-1022 | ai:claude
        if let Some(unattended) = autopilot_audit::unattended_product_annotation(e) {
            println!("      {}", unattended.yellow());
        }
    }
    println!(
        "\n  {} undo one with `aida autopilot revert <id|SPEC-ID>` (add --dry-run to preview).",
        "note:".dimmed()
    );
    Ok(())
}

/// `aida autopilot revert <target>` — the one-command reversal. Resolves the
/// execution record, derives the restore plan from it alone, applies it through
/// the same in-process paths the CLI verbs use, and records the reversal.
// trace:TASK-1018 | ai:claude
pub(crate) fn handle_autopilot_revert(
    target: &str,
    dry_run: bool,
    note: Option<&str>,
    json: bool,
) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let executions = autopilot_audit::read_executions(&project_root)?;
    let reversals = autopilot_audit::read_reversals(&project_root)?;

    let record = autopilot_audit::resolve_revert_target(&executions, &reversals, target)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let plan = autopilot_audit::plan_reversal(&record).map_err(|e| anyhow::anyhow!("{e}"))?;

    let applied = autopilot_audit::apply_reversal(&project_root, &plan, dry_run)?;
    if !dry_run {
        // A reversal is itself an audited event — durable comment first (the
        // trail that replicates), then the fast index.
        let actor = current_user_id(None);
        let entry =
            autopilot_audit::reversal_record(&chrono::Utc::now().to_rfc3339(), &plan, &actor, note);
        let storage = Storage::new(project_root.join(".aida-store"));
        if let Err(e) = comment_cmd::add_comment_cli(
            &storage,
            &plan.spec_id,
            &autopilot_audit::reversal_comment(&entry),
            Some("autopilot"),
            None,
        ) {
            eprintln!("  warning: the durable reversal comment could not be written: {e}");
        }
        autopilot_audit::append_reversals(&project_root, &[entry])?;
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "target": plan.target,
                "spec_id": plan.spec_id,
                "action": record.action,
                "dry_run": dry_run,
                "complete": plan.complete,
                "steps": applied,
            }))?
        );
        return Ok(());
    }

    let verb = if dry_run { "would restore" } else { "restored" };
    println!(
        "{} {} {} ({} {}) — {verb}:",
        if dry_run {
            "·".dimmed().to_string()
        } else {
            crate::glyph(crate::glyphs::Glyph::Check)
                .green()
                .to_string()
        },
        plan.target.cyan(),
        plan.spec_id.bold(),
        record.action,
        record.source.dimmed(),
    );
    for line in &applied {
        println!("    {} {line}", "→".green());
    }
    if !plan.complete {
        println!(
            "  {} the appended note stays on the record — an append-only trail is \
             retracted, never erased.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }
    if dry_run {
        println!(
            "  {} nothing was written. Drop --dry-run to apply.",
            "·".dimmed()
        );
    }
    Ok(())
}

/// `aida autopilot inspect` — dry-run the envelope over the current groom
/// candidates. Reuses the SAME candidate fence as `aida groom`
/// (`select_intake_candidates`) so the projection can never disagree with the
/// disposition path about what is touchable.
// trace:TASK-1147 | ai:claude
pub(crate) fn handle_autopilot_inspect(
    risk: &str,
    only_tag: Option<&str>,
    exclude_tag: Option<&str>,
    record: bool,
    json: bool,
) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let cfg = intake::IntakeConfig::load(&project_root);
    let max_risk = backlog::RiskLevel::parse(risk)?;
    let filters = intake::IntakeFilters {
        only_tag: only_tag.map(|s| s.to_string()),
        exclude_tag: exclude_tag.map(|s| s.to_string()),
        max_risk,
    };

    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;
    let queued_ids = all_queued_requirement_ids(&project_root);

    // Build the same candidate facts `aida groom` builds — plus the proposed
    // ACTION per spec (Draft -> approve, Approved-but-unqueued -> queue), which
    // the envelope grades. trace:TASK-1147
    let mut specs: Vec<intake::IntakeSpec> = Vec::new();
    let mut proposed: std::collections::HashMap<String, autopilot::ActionClass> =
        std::collections::HashMap::new();
    for req in &store.requirements {
        if req.archived {
            continue;
        }
        if is_standing_artifact_type(&format!("{:?}", req.req_type)) {
            continue;
        }
        let is_draft = matches!(req.status, aida_core::RequirementStatus::Draft);
        let is_approved_unqueued = matches!(req.status, aida_core::RequirementStatus::Approved)
            && !queued_ids.contains(&req.id);
        if !(is_draft || is_approved_unqueued) {
            continue;
        }
        let disp = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| req.id.to_string());
        let has_plan = !find_plan_files_for_spec(&project_root, &disp).is_empty();
        let (risk, risk_reason) = backlog::classify_risk_with_reason(req, has_plan);
        let tags: Vec<String> = req.tags.iter().cloned().collect();
        let predicted_files = backlog::collect_spec_files(&project_root, &disp);
        let deferred = intake::is_deferred(req.deferred, &tags);
        let action = if is_draft {
            autopilot::ActionClass::Approve
        } else {
            autopilot::ActionClass::Queue
        };
        proposed.insert(disp.clone(), action);
        specs.push(intake::IntakeSpec {
            id: disp,
            req_type: format!("{:?}", req.req_type).to_ascii_lowercase(),
            tags,
            trivial_footprint: intake::predicted_footprint_is_trivial(&predicted_files),
            deferred,
            risk,
            risk_reason,
        });
    }
    specs.sort_by(|a, b| a.id.cmp(&b.id));

    let (eligible, fenced) = intake::select_intake_candidates(&specs, &cfg, &filters);
    let fenced_ids: std::collections::HashSet<String> = eligible.iter().cloned().collect();
    let risk_of: std::collections::HashMap<&str, backlog::RiskLevel> =
        specs.iter().map(|s| (s.id.as_str(), s.risk)).collect();

    // The envelope: the conservative default table, widened only by an explicit
    // `[autopilot]` config posture, then TIGHTENED (demote-only) by the runtime
    // context — a headless run cannot pause-and-ask so its propose tier
    // escalates; an active solo posture inherits the drain's safe/keystone
    // partition. `effective_envelope` is the ratified TASK-0432 precedence
    // contract, so this dry-run grades under exactly the envelope a real groom
    // in this context would. No auto-execution is wired regardless.
    // trace:TASK-1020 | ai:claude
    let config_text =
        std::fs::read_to_string(project_root.join(".aida").join("config.toml")).unwrap_or_default();
    let overrides = autopilot::parse_authority_overrides(&config_text);
    // trace:TASK-1022 | ai:claude — one reader of the flag, so the envelope
    // tightening and the audit trail's `headless` layer cannot disagree.
    let headless = autopilot::current_headless();
    let solo_active = presence::current_solo(chrono::Utc::now());
    // The candidates graded below are post-fence: gate 1 already excluded
    // keystone via the same `is_keystone_class` classifier, so an active solo
    // resolves to its safe partition here.
    let posture = presence::resolve_solo_posture(solo_active, false);
    let env = autopilot::effective_envelope(
        autopilot::AutopilotEnvelope::default().with_overrides(overrides),
        headless,
        posture,
    );

    // Build one Decision per eligible spec. Grounding is the advisor's runtime
    // judgment (it needs the live corpus read); the dry-run assumes the
    // best case — a groundable Type-A — so the projection reflects what the
    // OTHER gates would do. The verdict is therefore an UPPER BOUND on autonomy.
    let decisions: Vec<autopilot::Decision> = eligible
        .iter()
        .map(|id| {
            let action = proposed
                .get(id)
                .copied()
                .unwrap_or(autopilot::ActionClass::Approve);
            autopilot::Decision {
                spec_id: id.clone(),
                action,
                grounding: autopilot::Grounding::TypeA,
                risk: risk_of.get(id.as_str()).copied().unwrap_or(max_risk),
                reason: String::new(),
                evidence: vec![],
            }
        })
        .collect();
    let rows = autopilot::project_decisions(&env, &fenced_ids, &decisions);

    if json {
        let eligible_json: Vec<_> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "spec_id": r.spec_id,
                    "action": r.action.token(),
                    "verdict": r.outcome.verdict_token(),
                    "gate": r.outcome.gate_label(),
                })
            })
            .collect();
        let fenced_json: Vec<_> = fenced
            .iter()
            .map(|(id, reason)| serde_json::json!({ "spec_id": id, "fenced": reason.describe() }))
            .collect();
        let out = serde_json::json!({
            "risk_ceiling": max_risk.token(),
            "grounding_assumed": "type-a (best case; advisor grounds at run time)",
            "auto_execution": false,
            // TASK-1020: the runtime context the effective envelope was
            // composed under (demote-only tightening; autonomy doc §8).
            "context": { "headless": headless, "solo": solo_active },
            "eligible": eligible_json,
            "fenced": fenced_json,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        // Recording still honored under --json.
        if record {
            record_inspect_rows(&project_root, &rows)?;
        }
        return Ok(());
    }

    println!(
        "{} autopilot inspect — dry-run (nothing is written to any spec)",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    println!(
        "  {}",
        format!(
            "envelope: risk≤{} · grounding=required (assumed groundable) · approve={} reject={}",
            max_risk.token(),
            env.authority_for(autopilot::ActionClass::Approve).token(),
            env.authority_for(autopilot::ActionClass::Reject).token(),
        )
        .dimmed()
    );
    // TASK-1020: name the composition context whenever it tightened (or could
    // tighten) the envelope, so a headless/solo dry-run is legibly different
    // from the keyboard one.
    if headless || solo_active {
        let mut parts: Vec<&str> = Vec::new();
        if headless {
            parts.push("headless — propose-tier escalates (cannot pause-and-ask)");
        }
        if solo_active {
            parts.push("solo — safe partition (keystone already fenced at gate 1)");
        }
        println!(
            "  {}",
            format!(
                "context: {} · tightening is demote-only (autonomy doc §8)",
                parts.join(" · ")
            )
            .dimmed()
        );
    }

    if rows.is_empty() {
        println!(
            "  {} 0 specs in the fence ({} fenced out). Nothing for the envelope to weigh.",
            "→".green(),
            fenced.len()
        );
    } else {
        println!(
            "\n  {} envelope verdict per eligible candidate:",
            "→".green()
        );
        for r in &rows {
            let (glyph, label) = match r.outcome {
                autopilot::Outcome::Execute => (
                    crate::glyph(crate::glyphs::Glyph::Check)
                        .green()
                        .to_string(),
                    "EXECUTE".green().bold(),
                ),
                autopilot::Outcome::Hold => ("⏸".yellow().to_string(), "HOLD".yellow().bold()),
                autopilot::Outcome::Escalate(_) => (
                    crate::glyph(crate::glyphs::Glyph::Warning)
                        .red()
                        .to_string(),
                    "ESCALATE".red().bold(),
                ),
            };
            println!(
                "    {} {:<12} {:<8} {}  {}",
                glyph,
                r.spec_id,
                r.action.token(),
                label,
                r.outcome.gate_label().dimmed()
            );
        }
    }

    if !fenced.is_empty() {
        println!(
            "\n  {} {} fenced out at gate 1 (never touchable — the authority map cannot widen this):",
            "·".dimmed(),
            fenced.len()
        );
        for (id, reason) in &fenced {
            println!(
                "    {} {} — {}",
                "✕".dimmed(),
                id,
                reason.describe().dimmed()
            );
        }
    }

    if record {
        let n = record_inspect_rows(&project_root, &rows)?;
        println!(
            "\n  {} recorded {} verdict(s) to {}",
            "→".green(),
            n,
            autopilot::audit_log_path(&project_root).display()
        );
    }

    println!(
        "\n  {} projection only — autopilot has NO authority to approve/reject/queue here; \n     that keystone-autonomy is deliberately not wired. Review with `aida autopilot audit`,\n     reverse with `aida autopilot challenge <id>`.",
        "note:".dimmed()
    );
    Ok(())
}

/// Append the inspected verdicts to the local audit log; returns the count.
pub(crate) fn record_inspect_rows(
    project_root: &std::path::Path,
    rows: &[autopilot::InspectRow],
) -> Result<usize> {
    let ts = chrono::Utc::now().to_rfc3339();
    let entries: Vec<autopilot::AuditEntry> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            autopilot::decision_entry(
                &ts, i, &r.spec_id, r.action, r.outcome, &r.reason, "inspect",
            )
        })
        .collect();
    autopilot::append_audit_entries(project_root, &entries)?;
    Ok(entries.len())
}

/// `aida autopilot audit` — list recorded decisions (and their reversals).
// trace:TASK-1147 | ai:claude
pub(crate) fn handle_autopilot_audit(limit: usize, open_only: bool, json: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let entries = autopilot::read_audit_entries(&project_root)?;

    // Fold challenges onto the decisions they target.
    let mut decisions: Vec<&autopilot::AuditEntry> =
        entries.iter().filter(|e| e.kind == "decision").collect();
    if open_only {
        decisions.retain(|d| !autopilot::is_challenged(&entries, &d.id));
    }
    if limit > 0 && decisions.len() > limit {
        decisions = decisions.split_off(decisions.len() - limit);
    }

    if json {
        let rows: Vec<_> = decisions
            .iter()
            .map(|d| {
                serde_json::json!({
                    "id": d.id,
                    "ts": d.ts,
                    "spec_id": d.spec_id,
                    "action": d.action,
                    "verdict": d.verdict,
                    "gate": d.gate,
                    "source": d.source,
                    "challenged": autopilot::is_challenged(&entries, &d.id),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!(rows))?
        );
        return Ok(());
    }

    if decisions.is_empty() {
        println!(
            "{} no recorded autopilot decisions yet. Run `aida autopilot inspect --record` to log a dry-run.",
            "·".dimmed()
        );
        return Ok(());
    }

    println!(
        "{} autopilot audit — {} recorded decision(s)",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
        decisions.len()
    );
    for d in &decisions {
        let challenged = autopilot::is_challenged(&entries, &d.id);
        let mark = if challenged {
            "⨯ CHALLENGED".red().to_string()
        } else {
            "· open".dimmed().to_string()
        };
        println!(
            "  {} {}  {:<12} {:<8} {:<9} {}  {}",
            d.id.cyan(),
            d.ts.dimmed(),
            d.spec_id.as_deref().unwrap_or("-"),
            d.action.as_deref().unwrap_or("-"),
            d.verdict.as_deref().unwrap_or("-"),
            d.gate.as_deref().unwrap_or("-").dimmed(),
            mark
        );
    }
    Ok(())
}

/// `aida autopilot challenge <target>` — record a reversal of a decision.
// trace:TASK-1147 | ai:claude
pub(crate) fn handle_autopilot_challenge(target: &str, note: Option<&str>) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let entries = autopilot::read_audit_entries(&project_root)?;

    let Some(decision_id) = autopilot::resolve_challenge_target(&entries, target) else {
        anyhow::bail!(
            "no matching autopilot decision for `{target}` — pass a decision id from \
             `aida autopilot audit`, or a SPEC-ID with an un-challenged recorded decision."
        );
    };
    if autopilot::is_challenged(&entries, &decision_id) {
        println!(
            "{} decision {} is already challenged — nothing to do.",
            "·".dimmed(),
            decision_id.cyan()
        );
        return Ok(());
    }
    let ts = chrono::Utc::now().to_rfc3339();
    let entry = autopilot::challenge_entry(&ts, &decision_id, note);
    autopilot::append_audit_entries(&project_root, &[entry])?;
    println!(
        "{} challenged autopilot decision {}{}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        decision_id.cyan(),
        note.map(|n| format!(" — {n}")).unwrap_or_default()
    );
    Ok(())
}

/// STORY-560: `aida groom` self-loads the store, computes its candidate fence
/// (P1/P2/P3 policy + always-on tag exclusions + keystone/deferred fences applied
/// HERE, programmatically — the agent never sees them as actionable), then
/// launch a headless `claude -p "/aida-assess [--apply]"` that reads the fenced
/// set, proposes dispositions, and (under `--apply`) approves within the fence
/// and grooms the queue. Propose-mode is the ultimate gate.
/// trace:STORY-560 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_intake_command(
    apply: bool,
    max_approvals: Option<usize>,
    only_tag: Option<&str>,
    exclude_tag: Option<&str>,
    risk: &str,
    then_drain: bool,
    dry_run: bool,
    permission_mode: Option<&str>,
) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let cfg = intake::IntakeConfig::load(&project_root);
    let max_risk = backlog::RiskLevel::parse(risk)?;
    let filters = intake::IntakeFilters {
        only_tag: only_tag.map(|s| s.to_string()),
        exclude_tag: exclude_tag.map(|s| s.to_string()),
        max_risk,
    };

    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;
    let queued_ids = all_queued_requirement_ids(&project_root);

    // The intake action set = open specs the agent could move forward: Drafts
    // (approve/reject/park candidates) + Approved-but-not-queued (queue
    // candidates). Build the IntakeSpec facts for each. trace:STORY-560
    let mut specs: Vec<intake::IntakeSpec> = Vec::new();
    for req in &store.requirements {
        if req.archived {
            continue;
        }
        // BUG-593: standing-artifact types (meta / folder / vision / principle /
        // term / constraint) are perpetual reference rows, NOT intake work — the
        // same set `aida status` / `aida advisor` / `aida list` exclude. A fresh
        // `aida init` seeds 6 META prompts; without this they surfaced as assess
        // candidates the cold-boot advisor could approve/queue. trace:BUG-593
        if is_standing_artifact_type(&format!("{:?}", req.req_type)) {
            continue;
        }
        let is_draft = matches!(req.status, aida_core::RequirementStatus::Draft);
        let is_approved_unqueued = matches!(req.status, aida_core::RequirementStatus::Approved)
            && !queued_ids.contains(&req.id);
        if !(is_draft || is_approved_unqueued) {
            continue;
        }
        let disp = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| req.id.to_string());
        let has_plan = !find_plan_files_for_spec(&project_root, &disp).is_empty();
        // BUG-595: carry the per-spec risk REASON so the assess fence is legible
        // (a fenced spec shows WHY, not just an opaque chip). trace:BUG-595
        let (risk, risk_reason) = backlog::classify_risk_with_reason(req, has_plan);
        let tags: Vec<String> = req.tags.iter().cloned().collect();
        let predicted_files = backlog::collect_spec_files(&project_root, &disp);
        // BUG-561: mirror the `aida list` honor-both deferred predicate
        // (STORY-584) so the operator's deferral shelf is fenced out, not
        // re-blessed. trace:BUG-561 | ai:claude
        let deferred = intake::is_deferred(req.deferred, &tags);
        specs.push(intake::IntakeSpec {
            id: disp,
            req_type: format!("{:?}", req.req_type).to_ascii_lowercase(),
            tags,
            trivial_footprint: intake::predicted_footprint_is_trivial(&predicted_files),
            deferred,
            risk,
            risk_reason,
        });
    }
    // Deterministic output order.
    specs.sort_by(|a, b| a.id.cmp(&b.id));

    let (eligible, fenced) = intake::select_intake_candidates(&specs, &cfg, &filters);

    let effective_on_apply = if then_drain {
        intake::OnApply::Drain
    } else {
        cfg.on_apply
    };

    println!(
        "{} intake pass",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    println!(
        "  {}",
        format!(
            "bias={} · do-not-approve=[{}] · on_apply={} · risk≤{}",
            cfg.disposition_bias.as_str(),
            cfg.do_not_approve_classes.join(","),
            effective_on_apply.as_str(),
            max_risk.token(),
        )
        .dimmed()
    );

    if eligible.is_empty() {
        println!(
            "  {} 0 specs in the intake fence ({} fenced out). Nothing for the advisor to weigh.",
            "→".green(),
            fenced.len()
        );
        return Ok(());
    }

    println!(
        "  {} {} spec(s) the advisor will weigh: {}",
        "→".green(),
        eligible.len(),
        eligible.join(", ").cyan()
    );
    let trivial_proposals: Vec<String> = specs
        .iter()
        .filter(|spec| eligible.contains(&spec.id) && intake::proposes_trivial_lifecycle(spec))
        .map(|spec| spec.id.clone())
        .collect();
    for id in &trivial_proposals {
        println!("  {}", intake::trivial_lifecycle_proposal_line(id));
    }
    if !fenced.is_empty() {
        println!(
            "  {} {} fenced out (do-not-approve class / needs-human / keystone / deferred / tag / risk):",
            "·".dimmed(),
            fenced.len()
        );
        for (id, reason) in &fenced {
            println!(
                "    {} {} — {}",
                "✕".dimmed(),
                id,
                reason.describe().dimmed()
            );
        }
    }

    // Seed the cold-boot prompt with the live advisor's context file when present,
    // so the headless advisor doesn't re-derive priorities from scratch. trace:STORY-626
    let prompt = intake::seeded_assess_prompt(&project_root, apply);
    let mode = permission_mode.unwrap_or("bypassPermissions");

    if dry_run {
        println!("\n{} dry run — not launching. Would run:", "·".dimmed());
        println!("  claude -p {:?} --permission-mode {}", prompt, mode.cyan());
        println!(
            "  {} AIDA_INTAKE_CANDIDATES={}",
            "env".dimmed(),
            eligible.join(",")
        );
        return Ok(());
    }

    if permission_mode.is_none() {
        println!(
            "  {} launching headless `claude -p` with {} permissions (override: --permission-mode <mode>)",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            "BYPASSED".yellow().bold()
        );
    }
    if apply {
        println!(
            "  {} {} — the advisor APPROVES within the fence + grooms the queue{}",
            "→".green(),
            "--apply".yellow().bold(),
            if effective_on_apply == intake::OnApply::Drain {
                ", then DRAINS"
            } else {
                ""
            }
        );
    } else {
        println!(
            "  {} propose-mode — writes NOTHING; review the proposal, then re-run with --apply",
            "→".green()
        );
    }
    println!();

    // The skill reads the resolved policy + the bounded fence from env; the
    // prompt stays the human-facing surface (mirrors the advisor tier's
    // env-passed payload). trace:STORY-560
    let child_grant =
        seat_authority::issue_child(&project_root, "advisor", &current_user_id(None))?;
    let mut command = std::process::Command::new("claude");
    command
        .arg("-p")
        .arg(&prompt)
        .arg("--permission-mode")
        .arg(mode)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env(seat_authority::GRANT_ENV, child_grant.id)
        .env("AIDA_INTAKE_APPLY", if apply { "1" } else { "0" })
        .env("AIDA_INTAKE_CANDIDATES", eligible.join(","))
        .env("AIDA_INTAKE_TRIVIAL_PROPOSALS", trivial_proposals.join(","))
        .env(
            "AIDA_INTAKE_DISPOSITION_BIAS",
            cfg.disposition_bias.as_str(),
        )
        .env(
            "AIDA_INTAKE_DO_NOT_APPROVE_CLASSES",
            cfg.do_not_approve_classes.join(","),
        )
        .env("AIDA_INTAKE_ON_APPLY", effective_on_apply.as_str())
        .env("AIDA_INTAKE_RISK", max_risk.token());
    if let Some(n) = max_approvals {
        command.env("AIDA_INTAKE_MAX_APPROVALS", n.to_string());
    }

    let status_code = command.status().map_err(|e| {
        anyhow::anyhow!(
            "failed to launch `claude -p` ({e}) — the headless intake pass needs the Claude Code \
             CLI on PATH. Install it, or run `/aida-backlog-groom` interactively instead."
        )
    })?;

    if status_code.success() {
        Ok(())
    } else {
        let code = status_code.code().unwrap_or(1);
        std::process::exit(code);
    }
}

/// Install a per-invocation `--order` override by exporting
/// `AIDA_BURNDOWN_ORDER`, the top tier of [`resolved_burndown_ready_order`].
/// Exporting (rather than threading a parameter) means the headless drain
/// `burndown run` launches — and every `burndown plan` that drain re-resolves
/// through — inherits the operator's one-run choice for free. An unrecognized
/// token is a hard error rather than a silent fallback: the operator asked for a
/// specific order, so guessing would be worse than refusing.
// trace:TASK-1175 | ai:claude
pub(crate) fn install_burndown_order_override(raw: Option<&String>) -> Result<()> {
    let Some(raw) = raw else { return Ok(()) };
    let order = burndown::parse_ready_order(raw).ok_or_else(|| {
        anyhow::anyhow!("unknown --order value `{raw}` (expected: priority, queue)")
    })?;
    std::env::set_var(
        "AIDA_BURNDOWN_ORDER",
        match order {
            burndown::ReadyOrder::Priority => "priority",
            burndown::ReadyOrder::Queue => "queue",
        },
    );
    Ok(())
}

pub(crate) fn handle_burndown_command(cmd: &crate::cli::BurndownCommand) -> Result<()> {
    match cmd {
        crate::cli::BurndownCommand::Plan {
            status,
            tag,
            batch,
            r#type,
            candidates,
            order,
            json,
        } => {
            install_burndown_order_override(order.as_ref())?;
            handle_burndown_plan(
                status,
                tag.as_deref(),
                batch.as_deref(),
                r#type.as_deref(),
                *candidates,
                *json,
            )
        }
        crate::cli::BurndownCommand::Explain { json } => handle_burndown_explain(*json),
        crate::cli::BurndownCommand::Readiness {
            hours,
            lanes,
            specs,
            json,
        } => machine_readiness::run_command(*hours, *lanes, *specs, *json),
        crate::cli::BurndownCommand::Run {
            status,
            tag,
            batch,
            max,
            concurrency,
            order,
            permission_mode,
            dry_run,
            verbose,
            quiet,
            force,
            vendor,
            panes,
            require_head,
        } => {
            install_burndown_order_override(order.as_ref())?;
            handle_burndown_run(
                status,
                tag.as_deref(),
                batch.as_deref(),
                *max,
                *concurrency,
                permission_mode.as_deref(),
                *dry_run,
                *verbose,
                *quiet,
                *force,
                vendor.as_deref(),
                panes.as_deref(),
                *require_head,
            )
        }
        crate::cli::BurndownCommand::Status { json } => handle_burndown_status(*json),
    }
}

/// STORY-545: build the `/aida-burndown` slash-command string the headless
/// session runs, from the selector + caps. Pure + unit-testable. The skill
/// reads these args ($ARGUMENTS): `--status`, `--tag`, `--batch`, `--max`,
/// `--concurrency`. trace:STORY-545 | ai:claude
pub(crate) fn burndown_skill_prompt(
    status: &str,
    tag: Option<&str>,
    batch: Option<&str>,
    max: Option<usize>,
    concurrency: Option<usize>,
) -> String {
    let mut s = format!("/aida-burndown --status {status}");
    if let Some(t) = tag {
        s.push_str(&format!(" --tag {t}"));
    }
    if let Some(b) = batch {
        s.push_str(&format!(" --batch {b}"));
    }
    if let Some(m) = max {
        s.push_str(&format!(" --max {m}"));
    }
    if let Some(c) = concurrency {
        s.push_str(&format!(" --concurrency {c}"));
    }
    s
}

/// TASK-1159: resolve whether `aida burndown run` uses the live stream-json
/// launch. Precedence is explicit `--verbose`, then explicit `--quiet`, then
/// `[burndown] verbose` config (project before global), then the built-in quiet
/// default. Pure + unit-tested so script-facing defaults stay pinned.
// trace:TASK-1159 | ai:codex
pub(crate) fn resolve_burndown_verbose(
    verbose_flag: bool,
    quiet_flag: bool,
    project_config: Option<bool>,
    global_config: Option<bool>,
) -> bool {
    if verbose_flag {
        true
    } else if quiet_flag {
        false
    } else {
        project_config.or(global_config).unwrap_or(false)
    }
}

// trace:TASK-1159 | ai:codex
pub(crate) fn read_burndown_verbose_from_config_path(path: &std::path::Path) -> Option<bool> {
    let content = std::fs::read_to_string(path).ok()?;
    let value = toml::from_str::<toml::Value>(&content).ok()?;
    value
        .get("burndown")
        .and_then(|section| section.get("verbose"))
        .and_then(|v| v.as_bool())
}

// trace:TASK-1159 | ai:codex
pub(crate) fn read_burndown_verbose_config(
    project_root: &std::path::Path,
) -> (Option<bool>, Option<bool>) {
    let project =
        read_burndown_verbose_from_config_path(&project_root.join(".aida").join("config.toml"));
    let global = aida_home_dir()
        .and_then(|home| read_burndown_verbose_from_config_path(&home.join(".aida/config.toml")));
    (project, global)
}

/// TASK-1172: read `[burndown] order` from one config path. An unreadable file,
/// a missing key, or an unrecognized value all yield `None` so the caller falls
/// through to the next source (and ultimately the default).
// trace:TASK-1172 | ai:claude
pub(crate) fn read_burndown_order_from_config_path(
    path: &std::path::Path,
) -> Option<burndown::ReadyOrder> {
    let content = std::fs::read_to_string(path).ok()?;
    let value = toml::from_str::<toml::Value>(&content).ok()?;
    value
        .get("burndown")
        .and_then(|section| section.get("order"))
        .and_then(|v| v.as_str())
        .and_then(burndown::parse_ready_order)
}

/// TASK-1172 / TASK-1175: how the drain orders its ready set. `AIDA_BURNDOWN_ORDER`
/// env (what the `--order` flag exports, so a headless child drain inherits the
/// operator's one-run choice) → project config → machine-global config → the
/// `priority` default. Never fails: a missing or malformed value at any tier
/// simply falls through to the next one.
// trace:TASK-1172 | ai:claude
// trace:TASK-1175 | ai:claude
pub(crate) fn resolved_burndown_ready_order(
    project_root: &std::path::Path,
) -> burndown::ReadyOrder {
    std::env::var("AIDA_BURNDOWN_ORDER")
        .ok()
        .and_then(|raw| burndown::parse_ready_order(&raw))
        .or_else(|| {
            read_burndown_order_from_config_path(&project_root.join(".aida").join("config.toml"))
        })
        .or_else(|| {
            aida_home_dir().and_then(|home| {
                read_burndown_order_from_config_path(&home.join(".aida/config.toml"))
            })
        })
        .unwrap_or_default()
}

/// TASK-1169 / ADR-22: read `[burndown] bg_wait_ceiling_ms` from one config
/// path. Project config wins over the machine-global one; both are optional.
// trace:TASK-1169 | ai:claude
pub(crate) fn read_bg_wait_ceiling_from_config_path(path: &std::path::Path) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    let value = toml::from_str::<toml::Value>(&content).ok()?;
    value
        .get("burndown")
        .and_then(|section| section.get("bg_wait_ceiling_ms"))
        .and_then(|v| v.as_integer())
        .and_then(|n| u64::try_from(n).ok())
}

/// TASK-1169 / ADR-22: the bounded background-wait ceiling (ms) AIDA sets on
/// every headless child it spawns. Env override → project config → global
/// config → default, clamped to a non-zero floor so wait-forever (the retired
/// `=0` stopgap) is unreachable. Never fails: an unreadable config or a
/// unresolvable project root falls through to the default.
// trace:TASK-1169 | ai:claude
pub(crate) fn resolved_bg_wait_ceiling_ms(project_root: Option<&std::path::Path>) -> u64 {
    let configured = project_root
        .and_then(|root| {
            read_bg_wait_ceiling_from_config_path(&root.join(".aida").join("config.toml"))
        })
        .or_else(|| {
            aida_home_dir().and_then(|home| {
                read_bg_wait_ceiling_from_config_path(&home.join(".aida/config.toml"))
            })
        });
    let override_env = std::env::var(burndown::BG_WAIT_CEILING_OVERRIDE_ENV).ok();
    burndown::resolve_bg_wait_ceiling_ms(override_env.as_deref(), configured)
}

/// TASK-1169 / ADR-22: the `(key, value)` a headless spawn site sets so the
/// child's turn-end background-wait ceiling is AIDA's bounded value rather than
/// whatever the ambient shell happened to carry. Applied CENTRALLY (every
/// headless spawn in `session.rs`) and at both burndown launch sites, so a
/// hand-typed `aida burndown run` and a daemonized one are identical — the
/// divergence that produced the 2026-07-18 limp incident.
// trace:TASK-1169 | ai:claude
pub(crate) fn bg_wait_ceiling_env(
    project_root: Option<&std::path::Path>,
) -> (&'static str, String) {
    (
        burndown::BG_WAIT_CEILING_ENV,
        resolved_bg_wait_ceiling_ms(project_root).to_string(),
    )
}

#[cfg(test)]
#[path = "tests/burndown_run_tests.rs"]
mod burndown_run_tests;

#[cfg(test)]
#[path = "tests/task1169_integration_wait_tests.rs"]
mod task1169_integration_wait_tests;

/// STORY-545: `aida burndown run` — kick off and walk away. Preflights the
/// advisor-blessed ready set (the same gate `plan` shows), then launches a
/// headless `claude -p "/aida-burndown <selector>"` that fans out
/// worktree-isolated implementers, integrates PRs, and loops until drained.
/// Strategy B (SPIKE-51): reuse the proven skill fan-out, don't reimplement it.
/// trace:STORY-545 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_burndown_run(
    status: &str,
    tag: Option<&str>,
    batch: Option<&str>,
    max: Option<usize>,
    concurrency: Option<usize>,
    permission_mode: Option<&str>,
    dry_run: bool,
    // TASK-804 (facet a of STORY-604): stream live per-event progress. When set,
    // the headless drain is launched with stream-json flags and its events are
    // teed to `.aida/burndown/<drain-id>.jsonl` + rendered live. Control flow is
    // identical to the quiet launch — visibility only. trace:TASK-804 | ai:claude
    verbose: bool,
    // TASK-1159: explicit negation for a configured verbose default.
    quiet: bool,
    // STORY-647/STORY-1353: bypass the dispatch-authority drain-start guardrail.
    force: bool,
    // TASK-1116: per-invocation `--vendor`/`--agent`. burndown's implementer
    // fan-out is the Claude harness's native subagent primitive (Claude-only),
    // so this exports `AIDA_HEADLESS_VENDOR` for the drain — any vendor-agnostic
    // per-spec orchestration inherits it — but does NOT reroute the native
    // fan-out. trace:TASK-1116 | ai:claude
    vendor: Option<&str>,
    // TASK-1120: opt-in pane hosting exported to the drain — any
    // `aida queue work --auto-complete` it spawns then hosts its implementer in
    // a titled tmux window. Faithful-launcher: `None` leaves the env untouched.
    panes: Option<&str>,
    // STORY-1414: refuse (not just warn) when the dev binary is stale.
    require_head: bool,
) -> Result<()> {
    // TASK-1120: export the requested pane host so the headless drain (and any
    // per-spec auto-complete orchestration under it) inherits it. Absent → env
    // untouched → byte-identical background spawn.
    if let Some(host) = panes {
        std::env::set_var(pane_host::HOST_ENV, host);
    }
    // STORY-1133: a companion is a NON-AUTHORITATIVE instance of a driver role —
    // it may read / converse / draft / advise, but must never drive a drain.
    // Refuse BEFORE the team gate (and before any sync/preflight), mirroring the
    // `aida queue work --auto-complete` companion gate, so even a companion that
    // otherwise carries advisor authority cannot start (or gate-probe) a drain
    // via `burndown run`. `--force` does NOT bypass this — the driver seat does.
    // trace:STORY-1133 | ai:claude
    if current_role_instance_is_companion() {
        anyhow::bail!(
            "companion sessions cannot drive drains. Start the authoritative driver \
             seat for this role, or run `aida burndown run` from the driver."
        );
    }
    // Starting a wave is dispatch, not disposition. `--force` remains the
    // audited guardrail escape hatch. trace:STORY-647 trace:STORY-1353 | ai:codex
    if !force && !has_dispatch_authority() {
        anyhow::bail!(
            "starting an autonomous drain needs dispatch authority (product, advisor, or \
             integrator role, or a live orchestrator)"
        );
    }

    // TASK-1116: install the per-invocation headless-vendor override (top
    // precedence) and export `AIDA_HEADLESS_VENDOR` so the drain inherits it.
    // burndown's fan-out is Claude-harness-only, so a non-Claude vendor is
    // called out loudly and pointed at the vendor-agnostic per-spec path — no
    // silent misroute. An unrecognized token is a hard error. trace:TASK-1116
    if let Some(raw) = vendor {
        let resolved = session::install_headless_vendor_override(raw).ok_or_else(|| {
            anyhow::anyhow!("unknown --vendor/--agent value `{raw}` (expected: claude, codex, agy)")
        })?;
        if resolved != session::HeadlessVendor::Claude {
            eprintln!(
                "  {} burndown's implementer fan-out is Claude-harness-only; `--vendor {}` \
                 sets AIDA_HEADLESS_VENDOR for the drain but the native subagent fan-out stays \
                 on Claude. For a fully {}-driven drain run `aida queue work <SPEC> \
                 --auto-complete --vendor {}` per spec.",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                resolved.as_str(),
                resolved.as_str(),
                resolved.as_str(),
            );
        }
    }

    // BUG-530: sync the orphan store before computing the blessed set. The
    // preflight reads the local `.aida-store/` worktree (via
    // resolve_burndown_sets → load_store_for_lookup → GitBackend), which can
    // LAG origin when a prior drain pushed Done→Completed bumps that this clone
    // hasn't pulled — so the preflight would otherwise announce stale specs
    // (e.g. already-Completed/unqueued ones). maybe_sync_pull is the proven
    // `--sync` primitive: fetch + ff/rebase, and it NEVER fails the caller (every
    // error → a warning), so a degraded/offline sync just falls back to the
    // local view. Runs on dry-run too, so the previewed set matches the real
    // drain. trace:BUG-530 | ai:claude
    if let Ok(project_root) = find_project_root() {
        if let Some(store_path) = detect_distributed_store_from(&project_root) {
            let _ = maybe_sync_pull(&store_path);
        }
    }

    // trace:TASK-149 — serialize-held specs are queued + blessed and drain in a
    // later wave; the runner never acts on them THIS wave, so they are not part
    // of the "nothing blessed" report below.
    let (ready, awaiting_signoff, _serialize_held, parked, _supervised, _titles) =
        resolve_burndown_sets(status, tag, batch, None)?;

    println!(
        "{} burndown run",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    println!(
        "  {}",
        burndown::selector_summary(status, tag, batch).dimmed()
    );

    // Nothing blessed → don't launch a session for no work.
    if ready.is_empty() {
        println!(
            "  {} 0 blessed specs to drain ({} pickable awaiting sign-off, {} parked).",
            "→".green(),
            awaiting_signoff.len(),
            parked.len()
        );
        if !awaiting_signoff.is_empty() {
            println!(
                "\n{} Pickable but not queued — bless with `aida queue add <id>`, then re-run:",
                "·".dimmed()
            );
            for id in &awaiting_signoff {
                println!("  {} {}", "+".yellow(), id);
            }
        }
        return Ok(());
    }

    println!(
        "  {} {} blessed spec(s) will drain: {}",
        "→".green(),
        ready.len(),
        ready.join(", ").cyan()
    );

    // TASK-1298: an unattended run must prove the host can sustain the work,
    // not merely that the spec substrate is healthy. Keep dry-run observational;
    // the real launch refuses on hard machine-readiness failures.
    // trace:TASK-1298 | ai:codex
    if !dry_run {
        let lanes = concurrency.unwrap_or(4).max(1);
        let specs = max.unwrap_or(ready.len()).min(ready.len()).max(1);
        let readiness_root =
            find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        let report = machine_readiness::probe(&readiness_root, 20, lanes, specs);
        machine_readiness::print_human(&report);
        if !report.ready() {
            anyhow::bail!(
                "machine readiness failed — apply the remedies above, then re-run `aida burndown readiness --hours 20 --lanes {lanes} --specs {specs}`"
            );
        }
    }

    // The skill re-resolves the ready set authoritatively (same gate), so the
    // preflight list is what WILL drain. Build the headless invocation.
    let prompt = burndown_skill_prompt(status, tag, batch, max, concurrency);
    let mode = permission_mode.unwrap_or("bypassPermissions");
    let (project_verbose, global_verbose) = find_project_root()
        .ok()
        .map(|root| read_burndown_verbose_config(&root))
        .unwrap_or((None, None));
    let stream_verbose = resolve_burndown_verbose(verbose, quiet, project_verbose, global_verbose);

    if dry_run {
        println!("\n{} dry run — not launching. Would run:", "·".dimmed());
        if stream_verbose {
            // Mirror the quiet path's `{:?}` quoting of the prompt positional so
            // the previewed command is copy-paste-safe.
            println!(
                "  claude -p {:?} --permission-mode {} {}",
                prompt,
                mode.cyan(),
                "--output-format stream-json --verbose --include-partial-messages".dimmed()
            );
            println!(
                "  {} live progress would stream to {}",
                "→".dimmed(),
                ".aida/burndown/<drain-id>.jsonl".cyan()
            );
        } else {
            println!("  claude -p {:?} --permission-mode {}", prompt, mode.cyan());
        }
        return Ok(());
    }

    // BUG-538: take the global drain lock before launching. A second drain
    // (another `burndown run`, or a `queue work --auto-complete`) against the
    // same tree would double-drive it — two integrators racing on main. The
    // guard's Drop frees the lock on the Ok path below; the non-success
    // `std::process::exit` path drops it explicitly. trace:BUG-538 | ai:claude
    let drain_lock_command = format!(
        "burndown run ({})",
        burndown::selector_summary(status, tag, batch)
    );
    let project_root = find_main_worktree_root()?;
    // STORY-1414: a wave pins its launching binary — warn about a stale dev
    // build (refuse under --require-head) before the lock is taken.
    // trace:STORY-1414 | ai:claude
    crate::freshness_gate::enforce_wave_launch_gate(&project_root, require_head)?;
    // BUG-759: record the blessed spec set in the lock so `aida drain status`
    // can name what this launcher-held drain is working (pid + started +
    // specs) for its entire wall-clock — the launcher writes no per-phase
    // drain-state file. The guard is held across the whole resume loop below,
    // so lock lifetime == `claude -p` child lifetime. trace:BUG-759 | ai:claude
    let _drain_guard =
        drain_lock::acquire_drain_lock_with_specs(&project_root, &drain_lock_command, &ready)?;

    // BUG-660: keep the host awake for the unattended headless drain. Best-effort
    // + pid-scoped (auto-releases when this process exits). trace:BUG-660
    let _sleep_inhibitor = {
        let inhibitor = drive_robustness::SleepInhibitor::for_drive("aida burndown run");
        if let Some(tool) = inhibitor.tool() {
            eprintln!(
                "  {} sleep-prevention active for this drain (via {})",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                tool
            );
        }
        inhibitor
    };

    // BUG-607: a drain-state.json left behind by a KILLED or crashed
    // `queue work --auto-complete` drain (its orchestrator pid is dead) must not
    // make the spawned `/aida-burndown` agent falsely believe a live drain
    // already owns these specs and refuse to fan out. The lock we just acquired
    // already guarantees exclusivity (BUG-538), so a dead-pid drain-state is a
    // pure stale tombstone — clear it before launching. trace:BUG-607 | ai:claude
    if let status @ (crate::drain_state::DrainStatus::Stale(_)
    | crate::drain_state::DrainStatus::Stopped(_)) = crate::drain_state::probe(&project_root)
    {
        let _ = crate::drain_state::DrainState::clear(&project_root);
        let stopped = matches!(status, crate::drain_state::DrainStatus::Stopped(_));
        println!(
            "  {} cleared a {} drain-state (its orchestrator is no longer running)",
            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
            if stopped { "stopped" } else { "stale" }
        );
    }

    // The default posture is faithful to the operator's walk-away intent: a
    // headless drain that pushes/merges/fans-out can't stall on prompts, so
    // default to bypassed permissions — but say so loudly and let `--permission-mode`
    // override. trace:STORY-545
    if permission_mode.is_none() {
        println!(
            "  {} launching headless `claude -p` with {} permissions (override: --permission-mode <mode>)",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            "BYPASSED".yellow().bold()
        );
    } else {
        println!(
            "  {} launching headless `claude -p` with permission mode `{}`",
            "→".green(),
            mode
        );
    }
    println!(
        "  {} {}",
        "→".green(),
        format!("claude -p {prompt:?}").dimmed()
    );
    println!();

    // BUG-755: the launcher — the long-lived process holding the drain lock —
    // must not exit while wave PRs are unmerged or blessed specs are unstarted.
    // A headless `claude -p` terminates at turn end, so any "background merge
    // watch" the session promises dies with it (observed 2026-07-18: three PRs
    // stranded "awaiting CI", four blessed specs never started). After each
    // session exit we probe the residual work; a clean exit that left residual
    // is a premature turn end → relaunch a bounded continuation turn. A
    // deliberate stop (the shelve/cap non-zero convention) or a spent resume
    // budget exits WITH an accurate handoff — never a claim of live watches.
    // trace:BUG-755 | ai:claude
    const MAX_RESUME_ROUNDS: usize = 3;
    let base_prompt = prompt.clone();
    let mut current_prompt = prompt.clone();
    let mut rounds_used = 0usize;
    // TASK-1169 / ADR-22: resolved ONCE for the whole drain so every relaunch
    // round carries the identical ceiling. trace:TASK-1169 | ai:claude
    let (ceiling_key, ceiling_value) = bg_wait_ceiling_env(Some(&project_root));
    println!(
        "  {} background-wait ceiling for this drain: {}m (override: {})",
        "→".green(),
        ceiling_value.parse::<u64>().unwrap_or_default() / 60_000,
        burndown::BG_WAIT_CEILING_OVERRIDE_ENV.dimmed(),
    );
    loop {
        // Propagate the drain's exit code either way so scripts can branch on
        // success/parked (the skill exits non-zero when it shelves work,
        // mirroring the orchestrator drain's exit-2 convention). `claude` is
        // resolved off PATH (matching every other launch site); a missing
        // binary surfaces as a guided error, not a raw ENOENT.
        let status_code = if stream_verbose {
            // TASK-804: stream-json + tee path. Redirect the drain's JSONL to a
            // discoverable log and render a live human progress line per event.
            run_burndown_verbose(&current_prompt, mode)?
        } else {
            // Inherit stdio so the drain streams live (the quiet default:
            // `claude -p` without stream-json buffers until completion, so the
            // operator sees the final summary).
            std::process::Command::new("claude")
                .arg("-p")
                .arg(&current_prompt)
                .arg("--permission-mode")
                .arg(mode)
                // BUG-607: the launcher holds the exclusive drain lock (BUG-538),
                // so the agent must NOT re-check for a "competing" drain — that
                // check detects its own launcher and self-deadlocks. This flag
                // tells the `/aida-burndown` skill to trust the lock and fan out.
                .env("AIDA_BURNDOWN_LOCK_HELD", "1")
                // TASK-1169 / ADR-22: the LAUNCHER sets the bounded background-wait
                // ceiling, so a hand-typed launch behaves identically to a
                // daemonized one (the 2026-07-18 limp came from exactly that
                // divergence) and the retired `=0` stopgap can't leak in from the
                // ambient shell. trace:TASK-1169 | ai:claude
                .env(ceiling_key, &ceiling_value)
                .status()
                .map_err(|e| {
                    anyhow::anyhow!(
                        "failed to launch `claude -p` ({e}) — the headless drain needs the Claude \
                         Code CLI on PATH. Install it, or use `aida queue work --auto-complete` \
                         (the orchestrator drain) instead."
                    )
                })?
        };

        let code = status_code.code().unwrap_or(1);
        // A `--max` cap makes "blessed specs remain unstarted" an EXPECTED stop,
        // so the unstarted probe is skipped; open wave PRs must still drain.
        let mut residual =
            probe_burndown_residual(&project_root, status, tag, batch, &ready, max.is_some());
        // TASK-1169 / ADR-21: the launcher integrates the open wave PRs ITSELF
        // — a Rust CI wait with no harness ceiling and no 10-minute tool cap —
        // BEFORE deciding whether another agent turn is even needed. Run on a
        // deliberate non-zero stop too: one spec shelving is no reason to
        // strand another spec's green PR. The residual is then RE-PROBED so the
        // follow-up decision sees the post-integration world (merged PRs gone,
        // parked specs excluded). trace:TASK-1169 | ai:claude
        if !residual.unmerged_prs.is_empty() {
            let rows = integrate_wave_prs(&project_root, &residual);
            if !rows.is_empty() {
                residual = probe_burndown_residual(
                    &project_root,
                    status,
                    tag,
                    batch,
                    &ready,
                    max.is_some(),
                );
            }
        }
        match burndown::drain_followup(code, residual.is_empty(), rounds_used, MAX_RESUME_ROUNDS) {
            burndown::DrainFollowup::Complete => {
                if code == 0 {
                    return Ok(());
                }
                // `std::process::exit` skips destructors — free the drain lock
                // explicitly so a non-zero (e.g. shelved-work, exit-2) drain
                // still leaves a clean lock for the next launch. trace:BUG-538
                drop(_drain_guard);
                std::process::exit(code);
            }
            burndown::DrainFollowup::Resume => {
                rounds_used += 1;
                println!(
                    "  {} the drain session ended its turn with unfinished work \
                     ({} open wave PR(s), {} unstarted blessed spec(s)) — relaunching a \
                     continuation turn ({rounds_used}/{MAX_RESUME_ROUNDS})",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    residual.unmerged_prs.len(),
                    residual.unstarted.len(),
                );
                current_prompt = burndown::continuation_prompt(&base_prompt, &residual);
            }
            burndown::DrainFollowup::Handoff => {
                print!("{}", burndown::render_drain_handoff(&residual));
                drop(_drain_guard);
                // Preserve a deliberate non-zero stop's code; a spent resume
                // budget surfaces as the shelved-work exit-2 convention so
                // scripts triage rather than read success.
                std::process::exit(if code != 0 { code } else { 2 });
            }
        }
    }
}

/// BUG-755: probe the residual work a drain session left behind when its
/// headless process exited — the impure half feeding the pure
/// [`burndown::drain_followup`] decision. Re-syncs the store (so the drain's
/// Done→Completed merge bumps are visible), re-resolves the blessed ready set
/// (the unstarted remainder — skipped when `--max` capped the run, where a
/// remainder is an expected stop), and matches the forge's open PRs against
/// the drain's scope (initial blessed set ∪ still-ready set). Every leg is
/// best-effort: a failed probe degrades to "no residual" rather than wedging
/// the launcher in a relaunch loop on bad data.
// trace:BUG-755 | ai:claude
pub(crate) fn probe_burndown_residual(
    project_root: &std::path::Path,
    status: &str,
    tag: Option<&str>,
    batch: Option<&str>,
    blessed: &[String],
    spec_capped: bool,
) -> burndown::ResidualWork {
    // Refresh the store view first — the drain's merges push Done→Completed
    // bumps this clone may not have pulled (same reason as the BUG-530 preflight
    // sync; never fails the caller).
    if let Some(store_path) = detect_distributed_store_from(project_root) {
        let _ = maybe_sync_pull(&store_path);
    }
    let unstarted: Vec<String> = if spec_capped {
        Vec::new()
    } else {
        resolve_burndown_sets(status, tag, batch, None)
            .map(|(ready, ..)| ready)
            .unwrap_or_default()
    };
    // The drain's scope: what it set out to drain plus what is still blessed
    // (a blocker clearing mid-drain can add specs).
    let mut scope: Vec<String> = blessed.to_vec();
    for id in &unstarted {
        if !scope.contains(id) {
            scope.push(id.clone());
        }
    }
    // Per-spec store facts so a deliberately-held (`review:draft-only`) or
    // shelved (NeedsAttention) spec's open PR is NOT treated as stranded.
    let mut facts: std::collections::HashMap<String, burndown::WaveSpecFacts> =
        std::collections::HashMap::new();
    if let Some(store) = load_store_for_lookup(project_root) {
        let norm = |s: &str| -> String {
            s.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase()
        };
        for req in &store.requirements {
            let disp = req
                .agreed_id
                .clone()
                .or_else(|| req.spec_id.clone())
                .unwrap_or_else(|| req.id.to_string());
            if scope.contains(&disp) {
                facts.insert(
                    disp,
                    burndown::WaveSpecFacts {
                        status_norm: norm(&req.status.to_string()),
                        tags: req.tags.iter().cloned().collect(),
                        // TASK-1169: a non-`drain` execution_mode holds the
                        // merge for a human (BUG-727) — deliberate, not
                        // stranded. trace:TASK-1169 | ai:claude
                        supervised: pr_ship::merge_requires_supervision(req.execution_mode),
                    },
                );
            }
        }
    }
    // Fresh (uncached) forge snapshot — the memoized `collect_open_prs` would
    // replay the pre-drain state and mask just-merged PRs.
    let open: Vec<(u64, String, String)> = collect_open_prs_uncached(project_root)
        .by_branch
        .into_values()
        .map(|pr| (pr.number, pr.title, pr.head_branch))
        .collect();
    burndown::ResidualWork {
        unmerged_prs: burndown::match_wave_prs(&scope, &open, &facts),
        unstarted,
    }
}

/// TASK-1169 / ADR-21: the launcher's OWN integration leg — the impure half of
/// the ratified design's primary item.
///
/// For every open wave PR the drain session left behind, this blocks IN RUST on
/// [`wait_for_ci_terminal`] (TASK-968's re-arming idle timer + absolute
/// ceiling), then decides via the pure [`burndown::wave_pr_action`] gate:
/// squash-merge the clean ones (+ `aida pull` for the Done→Completed bump),
/// HOLD the supervised ones (BUG-727), and PARK the rest `NeedsAttention` with
/// a recorded reason + a finding — never terminate, never close a PR.
///
/// Why the launcher and not the agent: a headless turn reaps its background
/// tasks at turn end and its foreground tool calls are capped at ~10 minutes,
/// so neither agent-side shape can hold a 30-minute cross-platform CI run. This
/// process has no such bound, costs no tokens while it waits, and already holds
/// the drain lock.
///
/// Every leg is best-effort per PR: one PR's probe/merge failure parks that PR
/// and moves to the next (EPIC-28 punt-and-continue), never aborting the drain.
// trace:TASK-1169 | ai:claude
pub(crate) fn integrate_wave_prs(
    project_root: &std::path::Path,
    residual: &burndown::ResidualWork,
) -> Vec<burndown::IntegrationOutcome> {
    let mut rows: Vec<burndown::IntegrationOutcome> = Vec::new();
    if residual.unmerged_prs.is_empty() {
        return rows;
    }
    println!(
        "\n  {} integrating {} open wave PR(s) in the launcher — CI waits run here, \
         not in an agent turn",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
        residual.unmerged_prs.len(),
    );

    for pr in &residual.unmerged_prs {
        println!(
            "  {} PR-{} ({}) — waiting for CI to reach a terminal state",
            crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
            pr.number,
            pr.spec.cyan(),
        );

        // BUG-727: a spec whose execution_mode is anything but `drain` (or is
        // unset — fail safe) must never be auto-merged. Checked FIRST so a
        // supervised PR doesn't even consume a CI wait.
        let supervision_label = wave_pr_supervision_label(project_root, &pr.spec);

        // The bounded foreground wait. `NoSignal` carrying a bound message is
        // the "wait expired" case ADR-22 requires us to PARK, not abort.
        let (ci, wait_expired) = if supervision_label.is_some() {
            (integrate::CiState::None, None)
        } else {
            match wait_for_ci_terminal(Some(project_root), &pr.branch) {
                CiProbe::Green { .. } => (integrate::CiState::Passing, None),
                CiProbe::Red { failed_summary, .. } => {
                    // A red verdict is not an expired wait — it belongs in the
                    // gate as `Failing`, with the detail surfaced in the log.
                    eprintln!("    {} {}", "·".dimmed(), failed_summary.dimmed());
                    (integrate::CiState::Failing, None)
                }
                CiProbe::PrNoChecks { .. } => (integrate::CiState::None, None),
                CiProbe::InProgress { .. } => (
                    integrate::CiState::Running,
                    Some("CI was still in progress when the wait returned".to_string()),
                ),
                CiProbe::NoSignal(reason) => {
                    // A bound (idle-stall / absolute ceiling) is a park; any
                    // other no-signal (no gh, no PR, network blip) is not — it
                    // degrades to "no CI signal" and the merge gate decides.
                    if reason.contains("ceiling") || reason.contains("idle") {
                        (integrate::CiState::Running, Some(reason))
                    } else {
                        eprintln!("    {} no CI signal: {}", "·".dimmed(), reason.dimmed());
                        (integrate::CiState::None, None)
                    }
                }
            }
        };

        // Review + mergeability, from the forge and the local verdict file —
        // either RequestChanges is a hard stop (never merge over a reviewer).
        let (forge_request_changes, mergeable, forge_head) =
            wave_pr_review_facts(project_root, pr.number);
        let local_request_changes =
            local_verdict_blocks_merge(project_root, pr.number, &pr.spec, forge_head.as_deref());

        let action = burndown::wave_pr_action(&burndown::WavePrFacts {
            supervision_label,
            wait_expired,
            ci,
            request_changes: forge_request_changes || local_request_changes,
            mergeable,
        });

        let succeeded = match &action {
            burndown::WavePrAction::Merge => merge_wave_pr(project_root, pr),
            burndown::WavePrAction::Hold(why) => {
                println!(
                    "  {} PR-{} held — {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    pr.number,
                    why
                );
                true
            }
            burndown::WavePrAction::Park(why) => {
                park_wave_pr(project_root, pr, why);
                true
            }
        };
        // A merge that was gated clean but failed at the forge is itself a
        // park — the work must never be left un-recorded.
        if matches!(action, burndown::WavePrAction::Merge) && !succeeded {
            park_wave_pr(
                project_root,
                pr,
                "the squash-merge call failed at the forge — PR left open for triage",
            );
        }
        rows.push(burndown::IntegrationOutcome {
            pr: pr.number,
            spec: pr.spec.clone(),
            action,
            succeeded,
        });
    }
    print!("{}", burndown::render_integration_report(&rows));
    rows
}

/// TASK-1169: the BUG-727 supervised-merge gate for one wave spec — `Some(label)`
/// when its `execution_mode` makes the merge supervised (anything but `drain`,
/// or unset).
///
/// BUG-1163: this gate FAILS CLOSED. When the spec's mode cannot be resolved —
/// the store is unreadable (e.g. a concurrent de-risk/groom store write, the
/// failure mode that leaked BUG-1156/STORY-1130/TASK-1233 to merge) or the spec
/// is absent — we return a hold label, NOT `None`. A supervision fence that
/// treats "can't tell" as "not supervised" would merge a drive/guided/operator
/// PR unreviewed; the safe direction is to HOLD (the PR is left open for a human
/// and merges on a later readable pass — never wedged, only conservatively
/// parked). The dangerous direction — merging supervised work unreviewed — is
/// exactly what this bug was.
// trace:BUG-1163 | ai:claude (supersedes the TASK-1169 fail-open tolerance)
pub(crate) fn wave_pr_supervision_label(
    project_root: &std::path::Path,
    spec: &str,
) -> Option<String> {
    let want = spec.to_ascii_uppercase();
    // `Some(mode)` ONLY when the spec's row was actually read from the store;
    // `None` means unresolvable — store unreadable (a concurrent write) OR spec
    // absent — and the fail-closed helper turns that into a hold.
    let resolved = load_store_for_lookup(project_root).and_then(|store| {
        store
            .requirements
            .iter()
            .find(|r| {
                r.spec_id
                    .as_deref()
                    .map(|s| s.eq_ignore_ascii_case(&want))
                    .unwrap_or(false)
                    || r.agreed_id
                        .as_deref()
                        .map(|s| s.eq_ignore_ascii_case(&want))
                        .unwrap_or(false)
            })
            .map(|r| r.execution_mode)
    });
    supervision_hold_label(spec, resolved)
}

/// BUG-1163: the fail-closed supervision decision, split from store I/O so the
/// safety property is unit-testable. `resolved` is `Some(mode)` only when the
/// spec's row was read from the store (`mode` may itself be `None` = unset);
/// `None` means the mode was unresolvable (store unreadable OR spec absent) and
/// MUST hold — a supervision fence never merges on "can't tell", because the
/// dangerous direction is merging drive/guided/operator work unreviewed, while a
/// false hold only leaves a PR open for a human.
// trace:BUG-1163 | ai:claude
pub(crate) fn supervision_hold_label(
    spec: &str,
    resolved: Option<Option<aida_core::ExecutionMode>>,
) -> Option<String> {
    match resolved {
        None => Some(format!(
            "merge supervision unresolved for {spec} — held for safety"
        )),
        Some(mode) if pr_ship::merge_requires_supervision(mode) => {
            Some(pr_ship::supervision_mode_label(mode))
        }
        Some(_) => None,
    }
}

#[cfg(test)]
mod bug_1163_supervision_fail_closed_tests {
    use super::*;
    use aida_core::ExecutionMode;

    // BUG-1163: an unresolvable mode — store unreadable (a concurrent write) OR
    // the spec absent — MUST hold. This is the exact leak: three drive specs
    // merged because the resolver failed open on a concurrent-write read.
    #[test]
    fn unresolved_mode_holds_fail_closed() {
        assert!(
            supervision_hold_label("BUG-9999", None).is_some(),
            "unresolvable mode must HOLD (fail closed), never merge"
        );
    }

    // Every non-drain mode (incl. unset) holds; only drain merges.
    #[test]
    fn resolved_modes_gate_correctly() {
        for m in [
            ExecutionMode::Drive,
            ExecutionMode::Guided,
            ExecutionMode::Operator,
            ExecutionMode::Decide,
        ] {
            assert!(
                supervision_hold_label("X", Some(Some(m))).is_some(),
                "{m:?} must hold"
            );
        }
        assert!(
            supervision_hold_label("X", Some(None)).is_some(),
            "unset mode is supervised — must hold"
        );
        assert!(
            supervision_hold_label("X", Some(Some(ExecutionMode::Drain))).is_none(),
            "drain is the only mode that auto-merges"
        );
    }
}

/// TASK-1169: probe one PR's review decision + mergeability from the forge.
/// Degrades to `(false, Unknown)` on any probe failure — "couldn't tell" must
/// not manufacture a RequestChanges, and `Unknown` mergeability is handled
/// safely by the gate (the forge refuses an unmergeable merge; it never
/// corrupts).
// trace:TASK-1169 | ai:claude
pub(crate) fn wave_pr_review_facts(
    project_root: &std::path::Path,
    pr_number: u64,
) -> (bool, integrate::MergeableState, Option<String>) {
    let change_ref = forge::ChangeRef {
        id: pr_number,
        url: String::new(),
        branch: String::new(),
        base: String::new(),
        title: None,
    };
    match forge::forge_for(project_root).change_status(&change_ref) {
        Ok(status) => (
            status.review == forge::ReviewDecision::ChangesRequested,
            if status.mergeable {
                integrate::MergeableState::Mergeable
            } else {
                // `gh` reports non-mergeable for both a real conflict and a
                // not-yet-computed state; the gate treats Unknown optimistically
                // and the forge still refuses a dirty merge, so this degrade is
                // safe in both directions.
                integrate::MergeableState::Unknown
            },
            (!status.head_sha.trim().is_empty()).then_some(status.head_sha),
        ),
        Err(_) => (false, integrate::MergeableState::Unknown, None),
    }
}

/// TASK-1169: does a local `.aida/review-verdicts/PR-N.json` block the merge?
/// True for a RequestChanges/Rejected verdict, a reviewer escalation, or an
/// existing artifact that cannot be reconciled. The launcher must never merge
/// over a reviewer any more than the orchestrator does. A genuinely missing
/// file is not a block (the wave may simply not have used delegated review),
/// but an existing unreadable file is evidence whose meaning cannot be proven
/// and therefore fails closed.
// trace:TASK-1169 | ai:claude
pub(crate) fn local_verdict_blocks_merge(
    project_root: &std::path::Path,
    pr_number: u64,
    spec: &str,
    current_sha: Option<&str>,
) -> bool {
    let paths = [
        review_verdict::verdict_path(project_root, &format!("PR-{pr_number}")),
        review_verdict::verdict_path(project_root, spec),
    ];
    let existing: Vec<_> = paths.iter().filter(|path| path.exists()).collect();
    if existing.is_empty() {
        return false;
    }
    let bodies: Vec<String> = match existing
        .iter()
        .map(|path| std::fs::read_to_string(path))
        .collect::<Result<_, _>>()
    {
        Ok(bodies) => bodies,
        Err(_) => return true,
    };
    let Some(current_sha) = current_sha else {
        return true;
    };
    !matches!(
        review_verdict::reconcile_artifacts_for_sha(bodies.iter().map(String::as_str), current_sha),
        Ok(Some(review_verdict::VerdictKind::Approved))
    )
}

#[cfg(test)]
mod bug_1581_merge_block_tests {
    use super::*;

    #[test]
    fn local_merge_chokepoint_blocks_an_approved_top_level_with_opposition() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".aida/review-verdicts/PR-2066.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            r#"{
              "verdict":"Approved",
              "reviewed_sha":"ac772eaca9d389fa762a232156df996023bfdf7a",
              "recorded_by":"reviewer-a",
              "rounds":[{
                "verdict":"RequestChanges",
                "reviewed_sha":"ac772eaca9",
                "recorded_by":"reviewer-b"
              }]
            }"#,
        )
        .unwrap();
        assert!(local_verdict_blocks_merge(
            root.path(),
            2066,
            "BUG-1581",
            Some("ac772eaca9d389fa762a232156df996023bfdf7a")
        ));
    }

    #[test]
    fn local_merge_chokepoint_reconciles_pr_and_spec_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("PR-2090.json"),
            r#"{"verdict":"Approved","reviewed_sha":"ac772eaca9d389fa762a232156df996023bfdf7a","recorded_by":"reviewer-pr"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("STORY-1448.json"),
            r#"{"verdict":"RequestChanges","reviewed_sha":"ac772eaca9d389fa762a232156df996023bfdf7a","recorded_by":"reviewer-spec"}"#,
        )
        .unwrap();
        assert!(local_verdict_blocks_merge(
            root.path(),
            2090,
            "STORY-1448",
            Some("ac772eaca9d389fa762a232156df996023bfdf7a")
        ));
    }

    #[test]
    fn wave_gate_uses_forge_head_not_stale_pr_artifact_head() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("PR-2090.json"),
            r#"{"verdict":"Approved","reviewed_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","recorded_by":"reviewer-pr"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("STORY-1448.json"),
            r#"{"verdict":"RequestChanges","reviewed_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","recorded_by":"reviewer-spec"}"#,
        )
        .unwrap();
        assert!(local_verdict_blocks_merge(
            root.path(),
            2090,
            "STORY-1448",
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        ));
        assert!(local_verdict_blocks_merge(
            root.path(),
            2090,
            "STORY-1448",
            None
        ));
    }
}

/// TASK-1169: squash-merge one gated-clean wave PR, then run `aida pull` so the
/// merge-driven Done→Completed auto-bump fires. `--delete-branch` is
/// deliberately NOT set: the implementer's worktree still holds the branch, so
/// the forge-side delete's local cleanup grumbles and (in the agent's `&&`
/// chain) used to swallow the pull leg — worktree pruning owns branch deletion
/// (BUG-758). Returns whether the merge itself succeeded.
// trace:TASK-1169 | ai:claude
pub(crate) fn merge_wave_pr(project_root: &std::path::Path, pr: &burndown::ResidualPr) -> bool {
    // BUG-1163: the merge-EXECUTION chokepoint self-guards. Even though the wave
    // loop already gates on `wave_pr_supervision_label` via `wave_pr_action`,
    // this second check at the point the merge actually runs means no caller
    // (a future wave path, a refactor, a re-entry) can merge a supervised spec
    // by bypassing the upstream gate. It reuses the same fail-closed resolver,
    // so an unresolvable mode holds here too. trace:BUG-1163 | ai:claude
    if let Some(label) = wave_pr_supervision_label(project_root, &pr.spec) {
        // BUG-1167: persist the hold as a substrate marker so a CONCURRENT
        // merger (an integrate sweep, another agent session, a stale-binary
        // drain) also refuses at the merge_change chokepoint — not just this
        // in-process guard, which was the BUG-1167 gap. trace:BUG-1167 | ai:claude
        let _ = crate::merge_hold::write_typed_hold(
            project_root,
            &crate::merge_hold::typed_hold(
                pr.number,
                crate::merge_hold::HoldReasonKind::Supervision,
                &label,
                None,
            )
            .with_spec(&pr.spec),
        );
        // trace:BUG-1236 | ai:claude
        if let Err(err) = crate::merge_hold::sync_label(project_root, pr.number, true) {
            eprintln!(
                "  {} merge-hold label not applied on PR-{}: {err} — run `aida merge-hold list --fix`",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                pr.number
            );
        }
        eprintln!(
            "  {} refusing to auto-merge PR-{} ({}) — {}",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            pr.number,
            pr.spec,
            label,
        );
        return false;
    }
    // Re-probe the forge head and reconcile every local artifact at the
    // irreversible boundary. The earlier wave classification is only a
    // snapshot; evidence or the PR head may change before merge execution.
    // trace:BUG-1581 | ai:codex
    let (_, _, forge_head) = wave_pr_review_facts(project_root, pr.number);
    if local_verdict_blocks_merge(project_root, pr.number, &pr.spec, forge_head.as_deref()) {
        eprintln!(
            "  {} refusing to auto-merge PR-{} ({}) — current-head review evidence is conflicting, blocking, or unprovable",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            pr.number,
            pr.spec,
        );
        return false;
    }
    // TASK-1244 / ADR-41: the wave merge shares the per-branch merge-lease with
    // `aida pr ship` and the drain merge phase, so no AIDA merge path runs
    // unserialized. Contention here just skips the PR for this wave.
    // trace:TASK-1244 | ai:claude
    let lease_root = main_worktree_root_from(project_root);
    let lease_target = crate::pr_cmd::pr_ship_target_branch(pr.number);
    let _merge_lease = match crate::merge_lock::acquire(
        &lease_root,
        &lease_target,
        Some(pr.number),
        "aida burndown wave merge",
        crate::merge_lock::DEFAULT_WAIT,
    ) {
        Ok(lease) => lease,
        Err(e) => {
            eprintln!(
                "  {} PR-{} ({}) skipped this wave — {e} (`aida merge-lock` shows the holder)",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                pr.number,
                pr.spec,
            );
            return false;
        }
    };
    let mut sink = network_retry::StderrSink;
    let change_ref = forge::ChangeRef {
        id: pr.number,
        url: String::new(),
        branch: pr.branch.clone(),
        base: String::new(),
        title: None,
    };
    let opts = forge::MergeOptions {
        method: forge::MergeMethod::Squash,
        squash_subject: None,
        squash_body: None, // trace:TASK-1330 | ai:claude
        delete_branch: false,
        match_head: None,
    };
    match forge::forge_for(project_root).merge_change(&change_ref, &opts, &mut sink) {
        Ok(_) => {
            println!(
                "  {} merged PR-{} ({})",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                pr.number,
                pr.spec.cyan(),
            );
            // The auto-bump: `aida pull` promotes the spec Done → Completed
            // when the referencing commit lands on main. Its own step, never
            // chained, so a grumble in the merge cleanup can't drop it.
            let aida = aida_exe_path();
            let pulled = std::process::Command::new(aida)
                .current_dir(project_root)
                .arg("pull")
                .status_retrying_etxtbsy();
            if !matches!(&pulled, Ok(s) if s.success()) {
                eprintln!(
                    "    {} `aida pull` did not succeed after merging PR-{} — the spec may still \
                     read Done; recover with `aida db reconcile-status --spec {}`",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    pr.number,
                    pr.spec,
                );
            }
            true
        }
        Err(e) => {
            eprintln!(
                "  {} merging PR-{} failed: {e:#}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                pr.number,
            );
            false
        }
    }
}

/// TASK-1169 / ADR-22: park one wave spec `NeedsAttention` with a recorded
/// reason and file a finding — the punt-not-abort safety item. The PR is left
/// OPEN and untouched; nothing is terminated and no uncommitted work is
/// orphaned without a record. Both writes are best-effort: a store or finding
/// failure is reported, never fatal, so one bad park can't stop the drain.
// trace:TASK-1169 | ai:claude
pub(crate) fn park_wave_pr(project_root: &std::path::Path, pr: &burndown::ResidualPr, why: &str) {
    println!(
        "  {} PR-{} ({}) parked — {}",
        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
        pr.number,
        pr.spec.cyan(),
        why,
    );
    let detail = format!("PR-{} ({}): {}", pr.number, pr.title, why);
    match shelve_spec_on_failure(
        project_root,
        &pr.spec,
        "integrate",
        4,
        "integration-wait",
        &detail,
        "review the PR, then re-run `aida burndown run` (or merge it by hand once it is green)",
    ) {
        Ok(Some(_)) => {}
        Ok(None) => eprintln!(
            "    {} {} could not be parked (already terminal or not in the store) — it is \
             reported in the handoff instead",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            pr.spec,
        ),
        Err(e) => eprintln!(
            "    {} could not park {}: {e:#}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            pr.spec,
        ),
    }
    if let Err(e) = file_integration_wait_finding(project_root, pr, why) {
        eprintln!(
            "    {} could not file the integration finding for PR-{}: {e:#}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            pr.number,
        );
    }
}

/// TASK-1169 / ADR-22: file the parked-integration finding so an expired or
/// refused wave integration lands in the triage surface the operator already
/// sweeps (`aida findings list`) rather than only in a drain's scrollback.
/// Mirrors `file_reviewer_verdict_unavailable_finding`'s shape.
// trace:TASK-1169 | ai:claude
pub(crate) fn file_integration_wait_finding(
    project_root: &std::path::Path,
    pr: &burndown::ResidualPr,
    why: &str,
) -> anyhow::Result<()> {
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        anyhow::bail!("no distributed store found");
    };
    let dispenser = load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    let title = format!("Wave integration parked for {} (PR {})", pr.spec, pr.number);
    let note = format!(
        "The burndown launcher's integration wait did not merge this PR: {why}\n\n\
         The PR is still OPEN and nothing was terminated. Triage it, then merge by hand \
         or re-run `aida burndown run`."
    );
    let mut req = aida_core::Requirement::new(title, note);
    req.req_type = aida_core::RequirementType::Task;
    req.status = aida_core::RequirementStatus::Draft;
    req.owner = get_default_author();
    req.tags.insert(format!("from-review:PR-{}", pr.number));
    req.tags.insert("kind:IntegrationWaitParked".to_string());
    req.tags.insert("severity:major".to_string());
    req.tags.insert("aida:burndown".to_string());

    // CR-8: stamp filing provenance BEFORE the store write — the object is
    // rewritten below from the in-memory copy. trace:CR-8 | ai:claude
    aida_core::provenance::stamp_if_absent(&mut req);
    let store = backend.update_atomically(|store| {
        let type_prefix = store.get_type_prefix(&req.req_type);
        store.add_requirement_with_id(req.clone(), None, type_prefix.as_deref());
    })?;
    let written = store
        .requirements
        .last()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("add_requirement_with_id produced no requirement"))?;
    aida_core::object_store::write_object(&store_path.join("objects"), &written)?;
    record_role_activity(written.spec_id.as_deref().unwrap_or("?"), "findings-add");
    Ok(())
}
