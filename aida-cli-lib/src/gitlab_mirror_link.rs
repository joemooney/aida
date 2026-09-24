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
//! show` git-linkage forge (option 3, out of scope here).
//!
//! Strict-review follow-up (same task, second pass) tightened four things:
//! 1. **Informational only.** The drain's `parse_ci_probe` reads every
//!    commit status on the rollup, so a `failure`/`pending` mirror status
//!    could shelve or stall a real drain over advisory GitLab-mirror
//!    evidence. The posted GitHub `state` is therefore always `success` —
//!    the real GitLab result lives in the `description`/`target_url` only —
//!    and `parse_ci_probe` additionally ignores [`MIRROR_STATUS_CONTEXT`] by
//!    name as a second, independent safety net.
//! 2. **Exact sha, not "newest for the branch".** The pipeline is looked up
//!    filtered by the pushed commit's `sha` (GitLab's pipelines endpoint
//!    supports a `sha` query param independently of `ref`), not the newest
//!    pipeline for the branch — an older, unrelated pipeline must never be
//!    reported as this push's evidence. No pipeline for that exact sha yet →
//!    post nothing (never a stale result).
//! 3. **Pinned `gh` target.** Resolved from `origin` via the same
//!    `PinnedRepo`/`resolve_pinned_repo` TASK-1455 uses, instead of `gh`
//!    inferring the repo from argv placeholders / cwd. Unresolvable → skip.
//! 4. **Open-PR gated.** Posts only when `branch` has an open GitHub PR
//!    (checked live) and is not the default branch — a status on a branch
//!    nobody is reviewing has no reader.
//!
//! Wired into the one place in AIDA that already knows a mirror push just
//! happened: `fan_out_mirror_push`'s STORY-760 code-leg call from `aida push`
//! (see `lib.rs`). Best-effort and bounded throughout — every failure
//! (glab/gh missing, no pipeline yet, no open PR, API error, timeout) is
//! swallowed, and every subprocess is timeout-bounded, so this can never
//! fail or hang the push it rides on. It is not a poller: it reports
//! whatever GitLab pipeline state is known right now, once, without waiting
//! for a terminal result — a later push (or a manual re-run) refreshes it.
// trace:TASK-1424 | ai:claude

use crate::forge::{CiState, CiStatus, ForgeKind};
use crate::merge_hold::{resolve_pinned_repo, PinnedRepo};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// The GitHub commit-status `context` this module posts under. Shared with
/// `parse_ci_probe`'s exclusion filter so the two can never drift apart.
// trace:TASK-1424 | ai:claude
pub(crate) const MIRROR_STATUS_CONTEXT: &str = "ci/gitlab-mirror";

/// Bounded wall-clock ceiling for each `git`/`gh`/`glab` subprocess this
/// module spawns — mirrors `FORGE_CLI_CALL_TIMEOUT` (BUG-1288) so a stalled
/// mirror link can never hold up the caller.
// trace:TASK-1424 | ai:claude
const MIRROR_LINK_CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Entry point. Best-effort, single-shot, never panics and never returns an
/// error the caller must handle — every failure is a silent no-op by design
/// (see module docs). Looks up the exact sha's newest GitLab pipeline and, if
/// one is known, posts an informational `ci/gitlab-mirror` commit status on
/// that sha via `gh api`, pinned to the project's own GitHub repo.
// trace:TASK-1424 | ai:claude
pub(crate) fn sync_mirror_ci_link(project_root: &Path, branch: &str) {
    // Only meaningful when GitHub is the review surface being linked FROM —
    // on a GitLab-primary or pure-git project there is no second forge to
    // point back at.
    if crate::forge::resolve_forge_kind(project_root) != ForgeKind::GitHub {
        return;
    }
    // Never the default branch — nothing reviews a push straight to main.
    if is_default_branch(project_root, branch) {
        return;
    }
    let Some(sha) = head_sha(project_root, branch) else {
        return;
    };
    // Only post when `branch` actually has an open PR to carry the status.
    if !has_open_pr(project_root, branch) {
        return;
    }
    let Ok(Some(pin)) = resolve_pinned_repo(project_root, ForgeKind::GitHub) else {
        return;
    };
    let Some(status) = newest_pipeline_status_for_sha(project_root, &sha) else {
        return; // no GitLab pipeline for this exact sha yet
    };
    post_github_mirror_status(project_root, &pin, &sha, &status);
}

/// Whether `branch` is this repo's default branch (short name, e.g. `main`).
fn is_default_branch(project_root: &Path, branch: &str) -> bool {
    let Some(default_ref) = detect_default_branch_ref_short(project_root) else {
        return false; // can't tell — don't block on an unknown default
    };
    branch == default_ref
}

/// `detect_default_branch_ref` returns a ref that may carry a remote prefix
/// (`origin/main`); this takes just the short name for a plain `==` compare
/// against a local branch name — same normalization
/// `required_status_checks_uncached` uses.
fn detect_default_branch_ref_short(project_root: &Path) -> Option<String> {
    crate::detect_default_branch_ref(project_root)
        .and_then(|r| r.rsplit('/').next().map(str::to_string))
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

/// Live check: does `branch` have an open GitHub PR right now? Errors and
/// "no change" both degrade to `false` — this is a gate for whether posting
/// makes sense, not a definitive existence check, so failing closed (skip)
/// is the safe default.
fn has_open_pr(project_root: &Path, branch: &str) -> bool {
    use crate::forge::ChangeLookup;
    matches!(
        crate::forge::forge_for_kind(project_root, ForgeKind::GitHub).change_for_branch(branch),
        Ok(ChangeLookup::Found(_))
    )
}

/// Query the GitLab mirror for the newest pipeline matching the EXACT `sha`
/// (GitLab's pipelines endpoint supports a `sha` filter independently of
/// `ref`), via the already-resolved `glab` CLI (host-based remote detection —
/// works whether the mirror remote is named `gitlab`, `all`, or anything
/// else). `None` on any failure (glab missing, no pipeline for this sha yet,
/// timeout) — best-effort, never a hard error.
// trace:TASK-1424 | ai:claude
fn newest_pipeline_status_for_sha(project_root: &Path, sha: &str) -> Option<CiStatus> {
    let glab = crate::resolve_forge_cli(ForgeKind::GitLab)?;
    let mut cmd = Command::new(&glab);
    cmd.current_dir(project_root).args([
        "api",
        "-X",
        "GET",
        "projects/:id/pipelines",
        "-f",
        &format!("sha={sha}"),
    ]);
    let out = crate::command_output_with_timeout(cmd, MIRROR_LINK_CALL_TIMEOUT);
    let out = out.ok_or_else(|| anyhow::anyhow!("glab pipelines lookup timed out"));
    let status = crate::forge::ci_status_from_glab_pipelines(out);
    if status.state == CiState::None {
        None
    } else {
        Some(status)
    }
}

/// Map a GitLab pipeline's forge-neutral [`CiState`] to a short, human word
/// for the informational description — never fed to `github_status_state`
/// (below), which is fixed to `success` regardless of this value.
fn mirror_result_word(state: CiState) -> &'static str {
    match state {
        CiState::Success => "passed",
        CiState::Failed => "failed",
        CiState::Running | CiState::Pending => "running",
        CiState::None => "unknown", // unreachable — caller already returned
    }
}

/// The GitHub commit-status `state` this module always posts. Fixed to
/// `success` regardless of the GitLab result: this status is informational
/// evidence for a human reviewer, never a merge gate, so it must not be able
/// to shelve or stall an automated drain the way a `failure`/`pending`
/// context on the rollup would (`parse_ci_probe`, `lib.rs`). The REAL GitLab
/// result is carried in the description and target_url instead.
// trace:TASK-1424 | ai:claude
const GITHUB_STATUS_STATE: &str = "success";

/// A short (GitHub caps `description` at 140 chars), human-readable summary
/// of the mirror pipeline's REAL state — this is where the actual result
/// lives, since the posted `state` field is always `success` (informational
/// only; see [`GITHUB_STATUS_STATE`]).
fn mirror_status_description(status: &CiStatus) -> String {
    format!(
        "GitLab mirror: {} — see link",
        mirror_result_word(status.state)
    )
}

/// POST the non-blocking, informational commit status via `gh api`, pinned to
/// the project's own GitHub repo (TASK-1455 pattern: `gh api` has no `-R`
/// flag, so the pin is baked into the endpoint path + `--hostname` rather
/// than left to `gh`'s cwd/placeholder inference). Every failure — `gh`
/// missing, API error, timeout — is swallowed: this is advisory evidence,
/// never a gate.
// trace:TASK-1424 | ai:claude
fn post_github_mirror_status(project_root: &Path, pin: &PinnedRepo, sha: &str, status: &CiStatus) {
    let Some(gh) = crate::resolve_forge_cli(ForgeKind::GitHub) else {
        return;
    };
    let Some(pipeline_url) = status.url.clone() else {
        return;
    };
    let description = mirror_status_description(status);
    let mut args: Vec<String> = vec!["api".into()];
    // `--hostname` only for a real, non-default host — an ssh-config alias
    // (no dot) has no meaning as a `gh` hostname, and github.com is already
    // `gh`'s default. Mirrors `PinnedRepo::host_is_alias`'s `contains('.')`
    // test without needing that merge_hold-private method here.
    if pin.host.contains('.') && pin.host != "github.com" {
        args.push("--hostname".into());
        args.push(pin.host.clone());
    }
    args.push(format!("repos/{}/statuses/{sha}", pin.path));
    args.push("-f".into());
    args.push(format!("state={GITHUB_STATUS_STATE}"));
    args.push("-f".into());
    args.push(format!("target_url={pipeline_url}"));
    args.push("-f".into());
    args.push(format!("description={description}"));
    args.push("-f".into());
    args.push(format!("context={MIRROR_STATUS_CONTEXT}"));

    let mut cmd = Command::new(&gh);
    cmd.current_dir(project_root).args(&args);
    let _ = crate::command_output_with_timeout(cmd, MIRROR_LINK_CALL_TIMEOUT);
}

#[cfg(test)]
#[path = "tests/task_1424_gitlab_mirror_link_tests.rs"]
mod task_1424_gitlab_mirror_link_tests;
