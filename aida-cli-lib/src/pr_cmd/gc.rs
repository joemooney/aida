use super::*;

// ---------------------------------------------------------------------------
// TASK-1312: `aida pr gc` — sweep local review-snapshot branches whose PR/MR
// has reached a terminal state.
//
// Every `pr-N` (GitHub) / `mr-N` (GitLab) local branch this project creates
// comes from fetching a forge change's head ref for headless review
// (`aida session start --owns PR-N`, `aida pr rebase`; see BUG-229). Nothing
// deletes it afterward, so a long-lived project's local branch namespace
// accumulates one per review, without bound. `aida pr gc` is the opt-in
// sweep: it never runs on its own, and it only ever touches branches whose
// name is exactly that shape.
// ---------------------------------------------------------------------------

/// Disposition for one candidate review-snapshot branch. Kept separate from
/// the git/forge I/O so the decision itself is a pure, table-testable
/// function.
// trace:TASK-1312 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrGcAction {
    /// Safe to delete: the change is merged/closed, the branch isn't
    /// checked out anywhere, and its tip still matches the change's last
    /// known head SHA.
    Delete,
    /// The change is still open — a review could still need this snapshot.
    SkipOpen,
    /// The branch is checked out in some worktree right now.
    SkipCheckedOut,
    /// The branch's tip no longer matches the change's head SHA (local
    /// commits, or the change moved since the fetch) — not provably a pure
    /// snapshot anymore, so it's left alone rather than guessed at.
    SkipDiverged {
        local_tip: String,
        remote_head: String,
    },
    /// Couldn't resolve the change's state (no forge CLI, auth failure,
    /// network error, deleted PR, …). Fails closed — nothing is deleted
    /// when the terminal-state check itself failed.
    SkipUnknownState(String),
}

/// Pure classifier: given what's already been observed about one candidate
/// branch, decide whether it's safe to delete. No I/O.
// trace:TASK-1312 | ai:claude
pub(crate) fn classify_pr_gc_branch(
    checked_out: bool,
    state: Result<crate::forge::ChangeState, String>,
    local_tip: &str,
    remote_head_sha: &str,
) -> PrGcAction {
    if checked_out {
        return PrGcAction::SkipCheckedOut;
    }
    match state {
        Err(reason) => PrGcAction::SkipUnknownState(reason),
        Ok(crate::forge::ChangeState::Open) => PrGcAction::SkipOpen,
        Ok(crate::forge::ChangeState::Merged) | Ok(crate::forge::ChangeState::Closed) => {
            if !remote_head_sha.is_empty() && local_tip == remote_head_sha {
                PrGcAction::Delete
            } else {
                PrGcAction::SkipDiverged {
                    local_tip: local_tip.to_string(),
                    remote_head: remote_head_sha.to_string(),
                }
            }
        }
    }
}

/// List local branches matching the review-snapshot shape.
// trace:TASK-1312 | ai:claude
fn pr_gc_candidate_branches(project_root: &std::path::Path) -> Result<Vec<String>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .output()
        .context("running git for-each-ref")?;
    anyhow::ensure!(
        out.status.success(),
        "git for-each-ref failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|b| parse_review_snapshot_branch(b).is_some())
        .collect())
}

/// Is `branch` checked out in any worktree of this repo right now?
// trace:TASK-1312 | ai:claude
fn pr_gc_branch_checked_out(project_root: &std::path::Path, branch: &str) -> Result<bool> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .context("running git worktree list")?;
    anyhow::ensure!(
        out.status.success(),
        "git worktree list failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let needle = format!("branch refs/heads/{branch}");
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|l| l.trim() == needle))
}

/// Resolve a local branch's tip SHA.
// trace:TASK-1312 | ai:claude
fn pr_gc_branch_tip(project_root: &std::path::Path, branch: &str) -> Result<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", &format!("refs/heads/{branch}")])
        .output()
        .context("running git rev-parse")?;
    anyhow::ensure!(
        out.status.success(),
        "git rev-parse {branch} failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn pr_gc_delete_branch(project_root: &std::path::Path, branch: &str) -> Result<()> {
    // trace:BUG-2211 | ai:claude
    let out = aida_core::git_ops::output_healing_config_lock(
        project_root,
        || {
            let mut cmd = std::process::Command::new("git");
            cmd.arg("-C").arg(project_root);
            cmd
        },
        &["branch", "-D", branch],
    )
    .context("running git branch -D")?;
    anyhow::ensure!(
        out.status.success(),
        "git branch -D {branch} failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(())
}

/// Resolve a candidate's terminal state + head SHA via the matching forge
/// CLI in one call. `Err` (stringified — `PrGcAction` doesn't need the full
/// error chain) on any failure to resolve, so the caller fails closed.
// trace:TASK-1312 | ai:claude
fn pr_gc_resolve_metadata(
    project_root: &std::path::Path,
    kind: crate::forge::ForgeKind,
    n: u64,
) -> Result<crate::forge::ChangeMetadata, String> {
    crate::forge::forge_for_kind(project_root, kind)
        .change_metadata(n, &mut network_retry::NoopSink)
        .map_err(|e| e.to_string())
}

/// `aida pr gc` handler: sweep local `pr-N` / `mr-N` branches whose change
/// has reached a terminal state.
// trace:TASK-1312 | ai:claude
pub(crate) fn pr_gc_handler(dry_run: bool) -> Result<()> {
    let project_root = find_main_worktree_root()?;
    let candidates = pr_gc_candidate_branches(&project_root)?;
    if candidates.is_empty() {
        println!("no local `pr-N` / `mr-N` review-snapshot branches found — nothing to sweep.");
        return Ok(());
    }

    let mut deleted = 0usize;
    let mut skipped = 0usize;
    for branch in &candidates {
        let Some((kind, n)) = parse_review_snapshot_branch(branch) else {
            continue;
        };
        let checked_out = pr_gc_branch_checked_out(&project_root, branch)?;
        let local_tip = pr_gc_branch_tip(&project_root, branch)?;
        let (state, remote_head) = if checked_out {
            // Skip the network round-trip — checked-out always skips.
            (Err(String::new()), String::new())
        } else {
            match pr_gc_resolve_metadata(&project_root, kind, n) {
                Ok(m) => (Ok(m.state), m.head_sha),
                Err(e) => (Err(e), String::new()),
            }
        };
        let action = classify_pr_gc_branch(checked_out, state, &local_tip, &remote_head);
        match action {
            PrGcAction::Delete => {
                if dry_run {
                    println!(
                        "  would delete `{branch}` — {} merged/closed, matches fetched head",
                        kind.change_noun()
                    );
                } else {
                    pr_gc_delete_branch(&project_root, branch)?;
                    println!("  {} deleted `{branch}`", "✓".green());
                }
                deleted += 1;
            }
            PrGcAction::SkipOpen => {
                println!("  skip `{branch}` — {} still open", kind.change_noun());
                skipped += 1;
            }
            PrGcAction::SkipCheckedOut => {
                println!("  skip `{branch}` — checked out in a worktree");
                skipped += 1;
            }
            PrGcAction::SkipDiverged {
                local_tip,
                remote_head,
            } => {
                println!(
                    "  skip `{branch}` — has commits not on its remote head (local {}, remote {})",
                    &local_tip[..local_tip.len().min(12)],
                    if remote_head.is_empty() {
                        "unknown".to_string()
                    } else {
                        remote_head[..remote_head.len().min(12)].to_string()
                    }
                );
                skipped += 1;
            }
            PrGcAction::SkipUnknownState(reason) => {
                println!(
                    "  skip `{branch}` — could not resolve {} state ({reason})",
                    kind.change_noun()
                );
                skipped += 1;
            }
        }
    }

    println!();
    if dry_run {
        println!(
            "{deleted} branch(es) would be deleted, {skipped} skipped (dry run — nothing changed)."
        );
    } else {
        println!("{deleted} branch(es) deleted, {skipped} skipped.");
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/task1312_pr_gc_tests.rs"]
mod task1312_pr_gc_tests;
