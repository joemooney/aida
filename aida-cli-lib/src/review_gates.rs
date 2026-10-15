//! rework verdicts and auto-queued PR review gates (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// TASK-54: return Some((behind, sample)) when `branch` lags `main`
/// (or origin/main) by 1+ commits. `sample` is a list of one-line
/// `<short-sha> <subject>` strings for the commits the user would
/// rebase onto. Returns None when:
///   - we're already on main (no point comparing to ourselves)
///   - main / origin/main don't exist locally
///   - branch is equal-to or strictly-ahead-of main (nothing to pull)
///   - any git invocation fails (treat as "no signal" rather than
///     pretending we know the answer).
///     Prefers `origin/main` when present so users with stale local
///     `main` still see the right delta. trace:TASK-54 | ai:claude
///     STORY-106: count commits this branch is ahead of `origin/main` (falls
///     back to `main` when origin isn't available). Best-effort — returns
///     `None` on any git failure so callers can degrade silently.
///     trace:STORY-106 | ai:claude
pub(crate) fn branch_commits_ahead_main(repo: &std::path::Path, branch: &str) -> Option<u32> {
    match branch_commits_ahead_default(repo, branch) {
        workflow_hints::CommitsAhead::Ahead(n) => Some(n),
        _ => None,
    }
}

pub(crate) fn branch_unshipped_patch_count_default(
    repo: &std::path::Path,
    branch: &str,
) -> Option<u32> {
    let Some(default_ref) = resolve_default_branch_ref(repo) else {
        return None;
    };
    branch_unshipped_patch_count_vs(repo, &default_ref, branch)
}

/// Body of [`branch_unshipped_patch_count_default`] with the default ref
/// already resolved. BUG-1756: the unshipped-work detector probes dozens of
/// candidate branches under a wall-clock budget; resolving the default ref
/// per branch (1–2 git subprocesses each) was pure overhead against that
/// budget, so the detector resolves once and passes it here.
// trace:BUG-1756 | ai:claude
pub(crate) fn branch_unshipped_patch_count_vs(
    repo: &std::path::Path,
    default_ref: &str,
    branch: &str,
) -> Option<u32> {
    match branch_unshipped_patch_count_probe(repo, default_ref, branch, None) {
        PatchCountProbe::Counted(n) => Some(n),
        PatchCountProbe::NoSignal | PatchCountProbe::TimedOut => None,
    }
}

/// Outcome of one candidate branch's bounded patch-count probe.
// trace:BUG-1756 | ai:claude
pub(crate) enum PatchCountProbe {
    Counted(u32),
    /// git failed — the pre-existing "no signal" shape: the candidate is
    /// dispositioned (skipped) and counts as scanned, exactly as
    /// [`branch_unshipped_patch_count_default`] returning `None` always has.
    NoSignal,
    /// The probe exceeded its per-candidate slice before answering. The
    /// candidate was NOT classified, so the caller must count it as
    /// unscanned and the scan as incomplete — never as "nothing to
    /// report" (PRIN-5).
    TimedOut,
}

/// [`branch_unshipped_patch_count_vs`] with an optional per-invocation time
/// slice. BUG-1756: `git cherry` patch-ids every default-branch commit since
/// the fork point, so ONE anciently-forked candidate (an old review
/// snapshot, a long-dead ref) can cost 20–30s — several times the entire
/// scan budget — and starve every candidate sorted after it. The slice caps
/// what a single candidate may spend; `None` (every unbounded caller, and
/// the exhaustive test paths) behaves exactly as before.
// trace:BUG-1756 | ai:claude
pub(crate) fn branch_unshipped_patch_count_probe(
    repo: &std::path::Path,
    default_ref: &str,
    branch: &str,
    probe_slice: Option<std::time::Duration>,
) -> PatchCountProbe {
    let run = |args: &[&str]| -> Result<std::process::Output, PatchCountProbe> {
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C").arg(repo).args(args);
        match probe_slice {
            Some(slice) => match command_output_with_timeout_detail(cmd, slice) {
                BoundedCommandOutput::Completed(out) => Ok(out),
                BoundedCommandOutput::SpawnFailed => Err(PatchCountProbe::NoSignal),
                BoundedCommandOutput::TimedOut => Err(PatchCountProbe::TimedOut),
            },
            None => cmd.output().map_err(|_| PatchCountProbe::NoSignal),
        }
    };

    // trace:BUG-853 | ai:codex
    // A squash-merged branch can still be ahead by commit id while its tree is
    // identical to the default branch. Treat that as shipped content.
    // The branch can come from a registered lease, so keep it from
    // reading as an option. trace:BUG-1622 | ai:claude
    let tree_diff = match run(&[
        "diff",
        "--quiet",
        git_arg_guard::END_OF_OPTIONS,
        default_ref,
        branch,
        "--",
    ]) {
        Ok(out) => out,
        Err(probe) => return probe,
    };
    match tree_diff.status.code() {
        Some(0) => return PatchCountProbe::Counted(0),
        Some(1) => {}
        _ => return PatchCountProbe::NoSignal,
    }

    // trace:BUG-1622 | ai:claude
    let out = match run(&["cherry", git_arg_guard::END_OF_OPTIONS, default_ref, branch]) {
        Ok(out) => out,
        Err(probe) => return probe,
    };
    if !out.status.success() {
        return PatchCountProbe::NoSignal;
    }
    PatchCountProbe::Counted(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|line| line.starts_with("+ "))
            .count() as u32,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReworkHeadChange {
    Unchanged,
    RebaseOnly,
    ContentChanged,
}

/// Classify the branch's contribution between two rework heads.
///
/// A changed SHA alone is not evidence of rework: amend and rebase rewrite
/// commit identities. Excluding the current default branch keeps a pure rebase
/// onto a newer main from looking like newly-authored work.
// trace:TASK-1265 trace:BUG-1445 | ai:codex
pub(crate) fn classify_rework_head_change(
    repo: &std::path::Path,
    before: &str,
    after: &str,
) -> Option<ReworkHeadChange> {
    let before = before.trim();
    let after = after.trim();
    // `before` is a stored verdict's reviewed sha: only a commit ID may
    // reach git. A non-ID is unclassifiable (the caller refuses).
    // trace:BUG-1622 | ai:claude
    if !git_arg_guard::is_hex_sha(before) || !git_arg_guard::is_hex_sha(after) {
        return None;
    }
    if before.eq_ignore_ascii_case(after) {
        return Some(ReworkHeadChange::Unchanged);
    }

    let tree_diff = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "diff",
            "--quiet",
            git_arg_guard::END_OF_OPTIONS,
            before,
            after,
            "--",
        ])
        .status()
        .ok()?;
    match tree_diff.code() {
        Some(0) => return Some(ReworkHeadChange::Unchanged),
        Some(1) => {}
        _ => return None,
    }

    let default_ref = resolve_default_branch_ref(repo)?;
    let exclude_default = format!("^{default_ref}");
    let range = format!("{before}...{after}");
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "rev-list",
            "--cherry-pick",
            "--right-only",
            git_arg_guard::END_OF_OPTIONS,
            &range,
            &exclude_default,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(if String::from_utf8_lossy(&out.stdout).trim().is_empty() {
        ReworkHeadChange::RebaseOnly
    } else {
        ReworkHeadChange::ContentChanged
    })
}

// trace:TASK-1265 trace:BUG-1445 | ai:codex
pub(crate) fn rework_no_op_message(
    pr: u32,
    before: &str,
    after: &str,
    reason: &str,
    round: usize,
    change: ReworkHeadChange,
) -> String {
    let classification = match change {
        ReworkHeadChange::Unchanged => "the PR head did not move",
        ReworkHeadChange::RebaseOnly => {
            "the PR head moved, but its patch-ids are unchanged (rebase-only)"
        }
        ReworkHeadChange::ContentChanged => "the branch contribution changed",
    };
    format!(
        "ROUND {round} rework added no patch-unique work to PR-{pr}: {classification} (head `{}` → `{}`). The previous round's commits do not count. Authoritative open items:\n{reason}",
        before.trim(),
        after.trim()
    )
}

// Reviewers normally persist the verdict under PR-N; older/manual handoffs
// may use the requirement id. Keep both readable so the no-op guard is armed
// by the same artifact that the reviewer actually wrote.
// trace:BUG-1445 | ai:codex
pub(crate) fn blocking_rework_verdict(
    project_root: &std::path::Path,
    spec: &str,
    pr: u32,
) -> Option<review_verdict::RecordedVerdict> {
    if let Some(verdict) = review_verdict::read_recorded_verdict(project_root, spec)
        .filter(|verdict| verdict.kind.blocks_done())
    {
        return Some(verdict);
    }
    let pr_id = format!("PR-{pr}");
    review_verdict::read_recorded_verdict(project_root, &pr_id)
        .filter(|verdict| verdict.kind.blocks_done())
}

/// Count how many commits `branch` is ahead of the repo's DEFAULT branch, and
/// say WHY when the answer can't be produced.
///
/// The `Option<u32>` wrapper above collapses "on the default branch",
/// "no default branch ref anywhere", and "rev-list failed" into one `None`,
/// which is fine for a hint but fatal for a gate: a gate that can't tell
/// "nothing to compare" from "comparison impossible" ends up waving work
/// through. `aida queue done` uses this richer form.
///
/// Two fixes over the old `origin/main`-or-`main` probe:
///   - the default branch is resolved with the same order the rest of the CLI
///     uses (`origin/HEAD` → `init.defaultBranch` → `main`/`master`, remote
///     ref preferred, LOCAL ref accepted) — so a repo with NO origin remote,
///     or one whose default branch is `master`, still gets a real answer.
///     Local-merge workflows live in exactly those repos.
///   - being *on* the default branch is a distinct, named outcome rather than
///     an unresolvable one.
// trace:BUG-775 | ai:claude
pub(crate) fn branch_commits_ahead_default(
    repo: &std::path::Path,
    branch: &str,
) -> workflow_hints::CommitsAhead {
    let Some(default_ref) = resolve_default_branch_ref(repo) else {
        return workflow_hints::CommitsAhead::Unknown(
            "no default branch could be resolved in this repository — \
             neither origin/HEAD, init.defaultBranch, main, nor master exists"
                .to_string(),
        );
    };
    // `origin/main` → `main`: compare NAMES so a branch checked out locally
    // is recognised as the default branch even when the resolved ref is the
    // remote-tracking form.
    let default_name = default_ref
        .strip_prefix("origin/")
        .unwrap_or(&default_ref)
        .to_string();
    if branch == "HEAD" || branch.eq_ignore_ascii_case(&default_name) || branch == default_ref {
        return workflow_hints::CommitsAhead::OnDefaultBranch;
    }
    let range = format!("{}..{}", default_ref, branch);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-list", "--count", &range])
        .output();
    let out = match out {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            return workflow_hints::CommitsAhead::Unknown(format!(
                "`git rev-list --count {range}` failed{}",
                if err.is_empty() {
                    String::new()
                } else {
                    format!(": {err}")
                }
            ));
        }
        Err(e) => {
            return workflow_hints::CommitsAhead::Unknown(format!("could not run git: {e}"));
        }
    };
    match String::from_utf8_lossy(&out.stdout).trim().parse::<u32>() {
        Ok(n) => workflow_hints::CommitsAhead::Ahead(n),
        Err(_) => workflow_hints::CommitsAhead::Unknown(format!(
            "`git rev-list --count {range}` returned no count"
        )),
    }
}

/// Full object name for `rev` in `repo`, or `None` when it doesn't resolve to
/// a commit here (unknown sha, sha from another clone, garbage input).
/// Expanding both sides is what lets a short sha recorded in a review verdict
/// be compared with a branch tip.
// trace:BUG-775 | ai:claude
pub(crate) fn resolve_commit_sha(repo: &std::path::Path, rev: &str) -> Option<String> {
    let rev = rev.trim();
    if rev.is_empty() {
        return None;
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            git_arg_guard::END_OF_OPTIONS, // trace:BUG-1622 | ai:claude
            &format!("{rev}^{{commit}}"),
        ])
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

/// TASK-1459: the head `aida review <SPEC>` marks its review-in-progress
/// marker at. Prefers the forge's live PR head — the local `origin/<branch>`
/// tracking ref used before this can be stale (nothing here fetches it), so a
/// marker written against it could under-cover a head that already moved.
/// Falls back to that local ref when the forge can't be reached (pure-git,
/// offline, forge fault) rather than leaving the marker head unrecorded,
/// which is the same best-effort shape as [`pr_head_sha_best_effort`].
// trace:TASK-1459 | ai:claude
pub(crate) fn review_marker_head_best_effort(
    project_root: &std::path::Path,
    pr: u64,
    branch: &str,
) -> Option<String> {
    pr_cmd::fetch_change_info_via_resolved_forge(
        project_root,
        pr,
        crate::forge::resolve_open_change_forge_kind(project_root),
    )
    .ok()
    .map(|info| info.head_oid)
    .filter(|s| !s.trim().is_empty())
    .or_else(|| resolve_commit_sha(project_root, &format!("origin/{branch}")))
}

/// `git merge-base --is-ancestor <ancestor> <descendant>` as a tri-state:
/// `Some(true)` / `Some(false)` for the two clean answers, `None` when the
/// probe itself could not run.
// trace:BUG-775 | ai:claude
pub(crate) fn is_ancestor_commit(
    repo: &std::path::Path,
    ancestor: &str,
    descendant: &str,
) -> Option<bool> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "merge-base",
            "--is-ancestor",
            git_arg_guard::END_OF_OPTIONS,
            ancestor,
            descendant,
        ]) // trace:BUG-1622 | ai:claude
        .output()
        .ok()?;
    match out.status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// Where the tip of `branch` (or HEAD when `branch` is `None`) sits relative
/// to the commit a review verdict named. All the git I/O for the review gate
/// lives here; the decision itself is the pure
/// `review_verdict::classify_tip_relation`.
// trace:BUG-775 | ai:claude
pub(crate) fn verdict_tip_relation(
    repo: &std::path::Path,
    branch: Option<&str>,
    reviewed_sha: Option<&str>,
) -> review_verdict::TipRelation {
    let Some(reviewed_raw) = reviewed_sha else {
        return review_verdict::TipRelation::Unknown;
    };
    let reviewed = resolve_commit_sha(repo, reviewed_raw);
    let tip = resolve_commit_sha(repo, branch.unwrap_or("HEAD"))
        .or_else(|| resolve_commit_sha(repo, "HEAD"));
    let ancestry = match (reviewed.as_deref(), tip.as_deref()) {
        (Some(a), Some(b)) => is_ancestor_commit(repo, a, b),
        _ => None,
    };
    review_verdict::classify_tip_relation(reviewed.as_deref(), tip.as_deref(), ancestry)
}

/// Resolve a branch's head LOCALLY, no forge call. `refs/remotes/origin/`
/// first (the common case: the branch was pushed but this checkout never
/// took it as a local branch), falling back to `refs/heads/` (a local-only
/// branch, e.g. the checkout this process is running in). Neither resolving
/// is "unknown", not an error -- the caller folds that into
/// `TipRelation::Unknown`, which `review_actionability` already treats as
/// indeterminate-therefore-absent (PRIN-5).
// trace:BUG-1508 | ai:claude
pub(crate) fn resolve_local_branch_head(
    project_root: &std::path::Path,
    branch: &str,
) -> Option<String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return None;
    }
    resolve_commit_sha(project_root, &format!("refs/remotes/origin/{branch}"))
        .or_else(|| resolve_commit_sha(project_root, &format!("refs/heads/{branch}")))
}

/// The spec id(s) a routed reviewer-queue entry's verdict must be read
/// against. A `Review PR-N: ...` auto-queue story (BUG-102/BUG-776) doesn't
/// carry a verdict itself -- it `implements` the real spec(s) the PR covers
/// (the same relationship `aida_subcmd_add_review_story` writes), so walk
/// that edge. A direct routing (the row's own spec_id, no review-story
/// wrapper) covers only itself.
///
/// `resolve` looks a relationship target's uuid up to its display id.
/// Generic over the lookup so a caller holding a full `RequirementsStore`
/// (an index) and a caller holding only a cache/backend (one targeted read
/// per uuid) share this walk.
// trace:BUG-1508 | ai:claude
pub(crate) fn covered_spec_ids_for_reviewer_row(
    req: &aida_core::Requirement,
    mut resolve: impl FnMut(uuid::Uuid) -> Option<String>,
) -> Vec<String> {
    if parse_review_story_pr_number(&req.title).is_some() {
        let ids: Vec<String> = req
            .relationships
            .iter()
            .filter(|rel| {
                matches!(&rel.rel_type, aida_core::RelationshipType::Custom(n) if n.eq_ignore_ascii_case("implements"))
            })
            .filter_map(|rel| resolve(rel.target_id))
            .collect();
        if !ids.is_empty() {
            return ids;
        }
    }
    req.agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .into_iter()
        .collect()
}

/// A routed reviewer-queue row's actionability (BUG-1508 AC1/AC2/AC3/AC8),
/// resolved entirely from local state: the verdict file(s) for the spec(s)
/// the row covers, and each covered spec's branch head resolved via
/// `resolve_local_branch_head` -- no forge/network call, so this is safe on
/// the fast `aida queue list` / `aida awaiting` paths.
///
/// The branch a covered spec's current head is read from, in order: the
/// verdict's own `reviewed_branch` (the reviewer recorded it, so it's the
/// most specific signal), then a live lease scoped to that spec, then a live
/// lease scoped to the ROW's own id (the review-story's lease, when a
/// reviewer has taken it via `aida worktree enter`). No resolvable branch
/// means an unknown head, which folds into `TipRelation::Unknown` ->
/// `NeedsReview` -- indeterminate is never read as covered (AC8).
///
/// A row covering several specs (one PR, several `(REQ-ID)` trailers) is
/// `NeedsReview` if ANY covered spec still needs one, else `AwaitingRework`
/// if any blocks, else `Resolved`.
///
/// `resolve` is the same uuid -> display-id lookup `covered_spec_ids_for_reviewer_row`
/// takes -- see that function for why it's generic.
// trace:BUG-1508 | ai:claude
pub(crate) fn reviewer_row_actionability(
    project_root: &std::path::Path,
    req: &aida_core::Requirement,
    leases: &[SessionLease],
    resolve: impl FnMut(uuid::Uuid) -> Option<String>,
) -> review_verdict::ReviewActionability {
    let story_id = req
        .agreed_id
        .as_deref()
        .or(req.spec_id.as_deref())
        .unwrap_or("");
    let covered = covered_spec_ids_for_reviewer_row(req, resolve);
    if covered.is_empty() {
        return review_verdict::ReviewActionability::NeedsReview;
    }
    let mut saw_rework = false;
    for spec_id in &covered {
        let verdict = review_verdict::read_recorded_verdict_any(project_root, &[spec_id.as_str()]);
        let branch = verdict
            .as_ref()
            .and_then(|v| v.reviewed_branch.clone())
            .or_else(|| {
                leases
                    .iter()
                    .find(|l| l.scope.eq_ignore_ascii_case(spec_id))
                    .map(|l| l.branch.clone())
            })
            .or_else(|| {
                leases
                    .iter()
                    .find(|l| l.scope.eq_ignore_ascii_case(story_id))
                    .map(|l| l.branch.clone())
            });
        let head = branch.and_then(|b| resolve_local_branch_head(project_root, &b));
        let reviewed_sha = verdict.as_ref().and_then(|v| v.reviewed_sha.as_deref());
        let ancestry = match (reviewed_sha, head.as_deref()) {
            (Some(a), Some(b)) => is_ancestor_commit(project_root, a, b),
            _ => None,
        };
        let relation =
            review_verdict::classify_tip_relation(reviewed_sha, head.as_deref(), ancestry);
        match review_verdict::review_actionability(verdict.as_ref(), relation) {
            review_verdict::ReviewActionability::NeedsReview => {
                return review_verdict::ReviewActionability::NeedsReview;
            }
            review_verdict::ReviewActionability::AwaitingRework => saw_rework = true,
            review_verdict::ReviewActionability::Resolved => {}
        }
    }
    if saw_rework {
        review_verdict::ReviewActionability::AwaitingRework
    } else {
        review_verdict::ReviewActionability::Resolved
    }
}

#[cfg(test)]
#[path = "tests/bug_1186_reviewer_seat_tests.rs"]
mod bug_1186_reviewer_seat_tests;

#[cfg(test)]
#[path = "tests/bug_1508_reviewer_row_tests.rs"]
mod bug_1508_reviewer_row_tests;

#[cfg(test)]
#[path = "tests/bug_775_commits_ahead_tests.rs"]
mod bug_775_commits_ahead_tests;

/// TASK-99: count how many commits the local `base_ref` is BEHIND
/// `origin/main` — i.e. commits on origin that the base hasn't yet
/// picked up. Returns `None` when `origin/main` (or the base ref)
/// doesn't resolve — fresh clone, offline, detached, etc. — so the
/// caller stays silent rather than warn on missing data. Best-effort:
/// any git failure → `None`. trace:TASK-99 | ai:claude
pub(crate) fn commits_behind_origin_main(repo: &std::path::Path, base_ref: &str) -> Option<u32> {
    let rev_parse = |refname: &str| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                git_arg_guard::END_OF_OPTIONS,
                refname,
            ]) // trace:BUG-1622 | ai:claude
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    // Only meaningful against the remote-tracking ref. If `origin/main`
    // isn't present we have nothing authoritative to compare against.
    rev_parse("origin/main")?;
    rev_parse(base_ref)?;
    let range = format!("{}..origin/main", base_ref);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-list", "--count", git_arg_guard::END_OF_OPTIONS, &range]) // trace:BUG-1622 | ai:claude
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// TASK-99: pure decision — given how many commits the new worktree's
/// base is behind `origin/<branch>`, produce the warning line (or
/// `None` when the base is current). Side-effect-free so the
/// behind-count → message mapping is unit-testable in isolation, no
/// git/CWD required. trace:TASK-99 | ai:claude
pub(crate) fn behind_origin_warning(behind: u32, branch: &str) -> Option<String> {
    if behind == 0 {
        return None;
    }
    Some(format!(
        "worktree base is {} commit{} behind origin/{} — run `aida rebase` to refresh, or your PR may land stale",
        behind,
        if behind == 1 { "" } else { "s" },
        branch
    ))
}

/// Default warn threshold (commits behind `origin/main`) at or above which
/// the statusline surfaces the `base behind by N` staleness indicator for an
/// active lease. Kept a touch loud so a one-commit drift doesn't nag every
/// prompt; the queue-list surface uses a lower (any-non-zero) floor.
// trace:TASK-101 | ai:claude
pub(crate) const BASE_BEHIND_STATUSLINE_THRESHOLD: u32 = 5;

/// Pure formatting for the "base behind by N" staleness indicator.
/// Given how many commits an active lease's branch is BEHIND `origin/main`
/// (from [`commits_behind_origin_main`]) and a noise-floor `threshold`,
/// produce the compact `base behind by N` label — or `None` when the base is
/// current (`behind == 0`) or the drift is below the caller's threshold. The
/// statusline passes a higher warn threshold; `aida queue list` passes `1` so
/// any non-zero drift surfaces. Side-effect-free so the behind-count → label
/// mapping is unit-testable without git/CWD.
// trace:TASK-101 | ai:claude
pub(crate) fn base_behind_indicator(behind: u32, threshold: u32) -> Option<String> {
    if behind == 0 || behind < threshold {
        return None;
    }
    Some(format!("base behind by {}", behind))
}

#[cfg(test)]
#[path = "tests/task_101_base_behind_tests.rs"]
mod task_101_base_behind_tests;

#[cfg(test)]
#[path = "tests/task_99_behind_origin_tests.rs"]
mod task_99_behind_origin_tests;

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

// BUG-1944: callers use `live_work::try_auto_queue_pr_review`, which wraps
// this and moves the PR's specs to In Progress. trace:BUG-1944 | ai:claude
pub(crate) fn try_auto_queue_pr_review_unbumped(
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
pub(crate) fn require_review_handoff(
    outcome: &AutoQueueOutcome,
) -> Result<(), auto_complete::PhaseFailure> {
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
