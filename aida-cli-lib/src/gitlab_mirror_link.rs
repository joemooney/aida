//! TASK-1424: link a GitHub PR back to the GitLab mirror pipeline for the
//! same branch/sha.
//!
//! Problem (from the spec): a GitLab pipeline can be the ONLY evidence a
//! claim in a GitHub PR is true (this repo's own `.gitlab-ci.yml` runs gates
//! GitHub's Linux-only required check knows nothing about), yet nothing in
//! the GitHub PR points a reviewer at the corresponding GitLab pipeline —
//! establishing it even exists requires already knowing to look on a second
//! forge with separate credentials.
//!
//! Scope (per the proxy triage comment on TASK-1424 — narrowed to option 1 or
//! 2 of the spec's three candidates): a non-blocking GitHub commit status
//! (option 2), not a PR-comment convention (option 1) and not a new `aida
//! show` git-linkage forge (option 3, out of scope here). The status's
//! `context` (`ci/gitlab-mirror`) makes re-posting naturally idempotent —
//! GitHub only ever shows the latest status per context, so there is nothing
//! to search-and-edit the way a comment would need.
//!
//! Reuses the existing GitLab pipeline read (`GitLabForge::ci_status`,
//! STORY-510/TASK-962) — this module adds no new GitLab-side call, only the
//! GitHub-side post. Wired into the one place in AIDA that already knows a
//! mirror push just happened: `fan_out_mirror_push`'s STORY-760 code-leg
//! call from `aida push` (see `lib.rs`). Best-effort and bounded throughout —
//! every failure (glab/gh missing, no pipeline yet, API error, timeout) is
//! swallowed, and every subprocess is timeout-bounded, so this can never
//! fail or hang the push it rides on. It is not a poller: it reports
//! whatever GitLab pipeline state is known right now, once, without waiting
//! for a terminal result — a later push (or a manual re-run) refreshes it.
// trace:TASK-1424 | ai:claude

use crate::forge::{CiState, CiStatus, CiTarget, Forge, ForgeKind, GitLabForge};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Bounded wall-clock ceiling for each `git`/`gh` subprocess this module
/// spawns — mirrors `FORGE_CLI_CALL_TIMEOUT` (BUG-1288) so a stalled mirror
/// link can never hold up the caller.
// trace:TASK-1424 | ai:claude
const MIRROR_LINK_CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Entry point. Best-effort, single-shot, never panics and never returns an
/// error the caller must handle — every failure is a silent no-op by design
/// (see module docs). Looks up `branch`'s newest GitLab pipeline and, if one
/// is known, posts a non-blocking `ci/gitlab-mirror` commit status on the
/// branch's current head sha via `gh api`.
// trace:TASK-1424 | ai:claude
pub(crate) fn sync_mirror_ci_link(project_root: &Path, branch: &str) {
    // Only meaningful when GitHub is the review surface being linked FROM —
    // on a GitLab-primary or pure-git project there is no second forge to
    // point back at.
    if crate::forge::resolve_forge_kind(project_root) != ForgeKind::GitHub {
        return;
    }
    let Ok(status) = GitLabForge::new(project_root).ci_status(CiTarget::Branch(branch.to_string()))
    else {
        return;
    };
    if status.state == CiState::None {
        return; // no GitLab pipeline for this branch yet — nothing to link
    }
    let Some(pipeline_url) = status.url.clone() else {
        return;
    };
    let Some(sha) = head_sha(project_root, branch) else {
        return;
    };
    post_github_mirror_status(project_root, &sha, &status, &pipeline_url);
}

/// Resolve `branch`'s current head commit, bounded. `None` on any failure
/// (detached worktree oddities, branch gone, timeout) — the caller then
/// skips posting rather than guessing a sha.
fn head_sha(project_root: &Path, branch: &str) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--verify", "--quiet", branch]);
    let out = crate::command_output_with_timeout(cmd, MIRROR_LINK_CALL_TIMEOUT)?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}

/// Map a GitLab pipeline's forge-neutral [`CiState`] to a GitHub commit-status
/// `state`. GitHub's statuses API has no separate "queued" state, so
/// `Pending`/`Running` both fold to `pending`. `None` is unreachable — the
/// caller already returned before constructing a status for it.
// trace:TASK-1424 | ai:claude
fn github_status_state(state: CiState) -> &'static str {
    match state {
        CiState::Success => "success",
        CiState::Failed => "failure",
        CiState::Running | CiState::Pending | CiState::None => "pending",
    }
}

/// A short (GitHub caps `description` at 140 chars), human-readable summary
/// of the mirror pipeline's state for the commit-status line.
fn mirror_status_description(status: &CiStatus) -> String {
    match status.state {
        CiState::Success => "GitLab mirror pipeline passed".to_string(),
        CiState::Failed => status
            .failing_checks
            .first()
            .map(|c| format!("GitLab mirror {c}"))
            .unwrap_or_else(|| "GitLab mirror pipeline failed".to_string()),
        _ => "GitLab mirror pipeline running".to_string(),
    }
}

/// POST the non-blocking commit status via `gh api`. `gh api` switches to
/// POST automatically once any `-f` field is present, and `{owner}`/`{repo}`
/// are `gh`'s own placeholders, resolved from the repo `gh` is invoked in
/// (same pattern as `required_status_checks_uncached`). Every failure —
/// `gh` missing, API error, timeout — is swallowed: this is advisory
/// evidence, never a gate.
// trace:TASK-1424 | ai:claude
fn post_github_mirror_status(
    project_root: &Path,
    sha: &str,
    status: &CiStatus,
    pipeline_url: &str,
) {
    let Some(gh) = crate::resolve_forge_cli(ForgeKind::GitHub) else {
        return;
    };
    let state = github_status_state(status.state);
    let description = mirror_status_description(status);
    let mut cmd = Command::new(&gh);
    cmd.current_dir(project_root).args([
        "api",
        &format!("repos/{{owner}}/{{repo}}/statuses/{sha}"),
        "-f",
        &format!("state={state}"),
        "-f",
        &format!("target_url={pipeline_url}"),
        "-f",
        &format!("description={description}"),
        "-f",
        "context=ci/gitlab-mirror",
    ]);
    let _ = crate::command_output_with_timeout(cmd, MIRROR_LINK_CALL_TIMEOUT);
}

#[cfg(test)]
#[path = "tests/task_1424_gitlab_mirror_link_tests.rs"]
mod task_1424_gitlab_mirror_link_tests;
