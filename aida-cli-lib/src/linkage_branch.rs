//! In-flight branch search for the git-linkage collector: which branch holds
//! an unshipped spec's referencing commits.
//!
//! [`contains_branch_scan`] is the original search (BUG-553/720/1528/1591/
//! 1594): one `git branch --all --contains <sha>` per referencing commit,
//! newest first, under [`crate::LINKAGE_BRANCH_SCAN_MAX_COMMITS`] and the
//! branch budget. Measured on the live repo (BUG-2221 SKETCH v2) each call
//! costs ~0.3-0.45 s idle, so an unshipped spec with several commits spent
//! seconds here under load.
//!
//! [`name_matched_branch_scan`] is the BUG-2221 P2 fast path in front of it.
//! The scan's answer, whenever any candidate's name matches a spec id, is
//! built only from those id-named refs. So the fast path lists branches once
//! (the same `git branch --all` listing, order and short names the scan
//! reads), keeps the id-named ones, and asks `merge-base --is-ancestor` per
//! (tip, commit) pair, which is ~14-44 ms each. It then rebuilds the scan's
//! first-seen order and local-before-remote ranking from those answers. A
//! fast-path HIT is the scan's answer for the same commits. A MISS (no
//! id-named ref, none containing a commit, any failed or timed-out call)
//! proves nothing and falls through to the unchanged scan, within the same
//! branch budget (advisor signoff 2026-10-25 23:55 MST).
// trace:BUG-2221 | ai:claude

use crate::*;
use std::time::{Duration, Instant};

/// Upper bound on the fast path's share of the branch budget, so a miss
/// leaves the unchanged scan most of its opportunity. Not a new budget: the
/// fast path and the scan run inside one [`crate::LINKAGE_BRANCH_SCAN_BUDGET`]
/// deadline.
// trace:BUG-2221 | ai:claude
const NAME_MATCH_FAST_PATH_SHARE: Duration = Duration::from_millis(500);

/// The in-flight branch search's result.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct BranchScan {
    pub(crate) branch: Option<String>,
    pub(crate) other_branches: Vec<String>,
    /// `(scanned, total)` referencing commits when the search stopped short.
    pub(crate) truncated: Option<(usize, usize)>,
}

/// The in-flight branch search: the name-matched fast path, then the
/// unchanged `--contains` scan on a miss. `bounded` is the show-deadline
/// mode (each scan call clipped to the branch deadline).
// trace:BUG-2221 | ai:claude
pub(crate) fn in_flight_branch_scan(
    project_root: &std::path::Path,
    ids: &[String],
    commits: &[(String, String, String)],
    branch_deadline: Instant,
    bounded: bool,
) -> BranchScan {
    let fast_deadline = branch_deadline.min(Instant::now() + NAME_MATCH_FAST_PATH_SHARE);
    name_matched_branch_scan(project_root, ids, commits, fast_deadline).unwrap_or_else(|| {
        contains_branch_scan(project_root, ids, commits, branch_deadline, bounded)
    })
}

/// BUG-1528: a branch name matches the spec when its normalized form equals
/// or contains a normalized spec id.
fn branch_matches_ids(b: &str, ids: &[String]) -> bool {
    let norm_id = |s: &str| s.to_ascii_lowercase().replace([' ', '_'], "-");
    let bl = b.to_ascii_lowercase();
    ids.iter().any(|id| {
        let nid = norm_id(id);
        !nid.is_empty() && (bl == nid || bl.contains(&nid))
    })
}

/// BUG-1591: whether a `%(refname:short)` listing line is a remote ref, and
/// the branch name with `remotes/` and `origin/` stripped.
fn split_branch_raw(raw: &str) -> (bool, &str) {
    let is_remote = raw.starts_with("origin/") || raw.starts_with("remotes/");
    let b = raw
        .trim_start_matches("remotes/")
        .trim_start_matches("origin/");
    (is_remote, b)
}

/// BUG-553/720: a branch that may be offered as the spec's own branch.
fn is_branch_candidate(b: &str) -> bool {
    !b.is_empty() && b != "HEAD" && b != "main" && b != "master" && !is_orphan_store_branch(b)
}

/// Push one containing ref into the scan's accumulators, exactly as the
/// original `--contains` loop does.
fn note_containing_ref(
    raw: &str,
    candidates: &mut Vec<String>,
    local_candidates: &mut Vec<String>,
) {
    let (is_remote, b) = split_branch_raw(raw);
    if !is_remote && !b.is_empty() && !local_candidates.iter().any(|c| c == b) {
        local_candidates.push(b.to_string());
    }
    if is_branch_candidate(b) && !candidates.iter().any(|c| c == b) {
        candidates.push(b.to_string());
    }
}

/// The selection over the accumulated candidates (BUG-1528/1591): id-named
/// candidates in first-seen order, local before remote-only; else the first
/// candidate.
fn select_branches(
    ids: &[String],
    candidates: Vec<String>,
    local_candidates: &[String],
) -> (Option<String>, Vec<String>) {
    let mut id_matches: Vec<String> = candidates
        .iter()
        .filter(|b| branch_matches_ids(b, ids))
        .cloned()
        .collect();
    id_matches.sort_by_key(|b| !local_candidates.iter().any(|c| c == b));
    if id_matches.is_empty() {
        (candidates.into_iter().next(), Vec::new())
    } else {
        let branch = Some(id_matches.remove(0));
        (branch, id_matches)
    }
}

/// The original in-flight branch search, unchanged in behaviour.
// trace:BUG-1528 | ai:claude
pub(crate) fn contains_branch_scan(
    project_root: &std::path::Path,
    ids: &[String],
    commits: &[(String, String, String)],
    branch_deadline: Instant,
    bounded: bool,
) -> BranchScan {
    // BUG-1528: don't anchor solely on the SINGLE newest referencing commit.
    // When a spec has multiple branches in flight (a branch-crossing — e.g.
    // `bug-1420-work` plus a later `bug-1420-round2`), the newest commit
    // across ALL of them can live on a branch that gets filtered out below
    // (main/master/orphan-store) or simply isn't the spec's own branch,
    // leaving the spec's actual local branch entirely unreachable via
    // `--contains <newest-sha>` even though `git branch --list` plainly shows
    // it. Union the `--contains` result over EVERY referencing commit (still
    // newest-first, so ties still prefer recency) so a branch holding an
    // older-but-still-relevant commit is found too.
    let mut candidates: Vec<String> = Vec::new();
    let mut local_candidates: Vec<String> = Vec::new();
    // BUG-1594: `git branch --contains` is one subprocess per referencing
    // commit, and a spec with a long commit trail (or commits old enough that
    // every agent branch contains them) made this loop dominate `aida show`.
    // Walk newest-first under a commit cap AND a wall-clock budget; if either
    // stops the walk, record it so the renderer says the branch search is
    // partial. trace:BUG-1594 | ai:claude
    let mut scanned = 0usize;
    for (commit_full, _, _) in commits {
        if scanned >= LINKAGE_BRANCH_SCAN_MAX_COMMITS {
            break;
        }
        let remaining = branch_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        // Clone-safe builder: host git refuses a seat clone (TASK-2143
        // ratchet). trace:BUG-2221 | ai:claude
        let mut cmd = aida_core::git_ops::host_git_command(project_root);
        cmd.args([
            "branch",
            "--all",
            "--contains",
            commit_full,
            "--format=%(refname:short)",
        ]);
        // BUG-1993: only `aida show` bounds each call; other callers keep the
        // unbounded call and the between-call budget check.
        let result = if bounded {
            command_output_with_timeout_detail(cmd, remaining)
        } else {
            match cmd.output() {
                Ok(o) => BoundedCommandOutput::Completed(o),
                Err(_) => BoundedCommandOutput::SpawnFailed,
            }
        };
        let contains = match result {
            BoundedCommandOutput::Completed(o) if o.status.success() => {
                scanned += 1;
                String::from_utf8_lossy(&o.stdout).trim_end().to_string()
            }
            BoundedCommandOutput::TimedOut => break,
            _ => {
                scanned += 1;
                continue;
            }
        };
        // BUG-553: a commit can be reachable from MULTIPLE branches when a
        // later spec's branch was stacked on this one's unmerged commit (the
        // BUG-554 anti-pattern). Picking the first arbitrarily then
        // mis-attributes the spec to a sibling's branch (e.g. TASK-806 shown
        // on `task-805`). Prefer the branch whose name matches one of the
        // spec ids being resolved; fall back to the first only when none
        // matches. trace:BUG-553 | ai:claude
        //
        // BUG-720: also exclude the orphan `aida-store` branch, which can be
        // the only ref holding a store bookkeeping commit that names the spec.
        //
        // BUG-1591: remember which candidates exist as a LOCAL branch —
        // stripping `origin/` erased that, so selection fell to commit recency
        // and a newer remote-only branch beat the spec's own local branch.
        // trace:BUG-1591 | ai:claude
        for raw in contains.lines() {
            note_containing_ref(raw.trim(), &mut candidates, &mut local_candidates);
        }
    }
    let truncated = (scanned < commits.len()).then_some((scanned, commits.len()));
    let (branch, other_branches) = select_branches(ids, candidates, &local_candidates);
    BranchScan {
        branch,
        other_branches,
        truncated,
    }
}

/// BUG-2221 P2: the scan's answer computed from id-named refs only, or
/// `None` (a miss: fall through to [`contains_branch_scan`]).
///
/// Exactness. The scan examines the newest `n = min(len, MAX_COMMITS)`
/// commits; a ref appears in its candidates at the first commit index it
/// contains, in listing order within one index. When any id-named candidate
/// exists the scan answers from id-named candidates alone (first-seen order,
/// then local-before-remote). This computes, for EVERY id-named ref in the
/// same listing, its first containing index among the same `n` commits, and
/// replays the scan's accumulation in that order — so a hit is the answer an
/// untruncated scan of those commits gives. Truncation by the commit cap is
/// reported exactly as the scan reports it.
// trace:BUG-2221 | ai:claude
pub(crate) fn name_matched_branch_scan(
    project_root: &std::path::Path,
    ids: &[String],
    commits: &[(String, String, String)],
    deadline: Instant,
) -> Option<BranchScan> {
    let n = commits.len().min(LINKAGE_BRANCH_SCAN_MAX_COMMITS);
    if n == 0 {
        return None;
    }
    let run = |args: &[&str]| -> Option<std::process::Output> {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        let mut cmd = aida_core::git_ops::host_git_command(project_root);
        cmd.args(args);
        match command_output_with_timeout_detail(cmd, left) {
            BoundedCommandOutput::Completed(o) => Some(o),
            _ => None,
        }
    };
    // The scan's own listing: same refs, order and short names.
    let listing = run(&[
        "branch",
        "--all",
        "--format=%(refname:short)%00%(objectname)",
    ])?;
    if !listing.status.success() {
        return None;
    }
    let listing = String::from_utf8_lossy(&listing.stdout).to_string();
    // (raw short name, tip OID) of every id-named ref, in listing order.
    let mut named: Vec<(String, String)> = Vec::new();
    for line in listing.lines() {
        let Some((raw, oid)) = line.trim().split_once('\0') else {
            continue;
        };
        let (_, b) = split_branch_raw(raw);
        if is_branch_candidate(b) && branch_matches_ids(b, ids) && git_arg_guard::is_hex_sha(oid) {
            named.push((raw.to_string(), oid.to_string()));
        }
    }
    if named.is_empty() {
        return None;
    }
    // First containing commit index per tip; refs sharing a tip share it.
    let mut first_index: std::collections::HashMap<&str, Option<usize>> =
        std::collections::HashMap::new();
    for (_, tip) in &named {
        if first_index.contains_key(tip.as_str()) {
            continue;
        }
        let mut found = None;
        for (i, (commit_full, _, _)) in commits.iter().take(n).enumerate() {
            if !git_arg_guard::is_hex_sha(commit_full) {
                return None;
            }
            let out = run(&[
                "merge-base",
                "--is-ancestor",
                commit_full.as_str(),
                tip.as_str(),
            ])?;
            match out.status.code() {
                Some(0) => {
                    found = Some(i);
                    break;
                }
                Some(1) => {}
                // An error is not a "no": the fast path proves nothing.
                _ => return None,
            }
        }
        first_index.insert(tip.as_str(), found);
    }
    // Replay the scan's accumulation: commit index ascending, listing order
    // within one index.
    let mut candidates: Vec<String> = Vec::new();
    let mut local_candidates: Vec<String> = Vec::new();
    for i in 0..n {
        for (raw, tip) in &named {
            if first_index.get(tip.as_str()).copied().flatten() == Some(i) {
                note_containing_ref(raw, &mut candidates, &mut local_candidates);
            }
        }
    }
    if candidates.is_empty() {
        return None;
    }
    let (branch, other_branches) = select_branches(ids, candidates, &local_candidates);
    Some(BranchScan {
        branch,
        other_branches,
        truncated: (n < commits.len()).then_some((n, commits.len())),
    })
}

#[cfg(test)]
#[path = "tests/bug_2221_branch_fast_path_tests.rs"]
mod bug_2221_branch_fast_path_tests;
