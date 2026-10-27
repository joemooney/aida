//! `aida dispatch flow` and `aida dispatch train` in-process runners
//! (STORY-1917 Slice 3, TASK-2344 Q2).
//!
//! Both runners run exclusively under `aida dispatch serve` (where the advisor
//! terminal's bound grant is inherited) and bind a [`DispatchExecutionContext`]
//! (`crate::gate::dispatch_auth::DispatchExecutionContext`) from the canonical
//! on-disk `requests/<id>.json` and `results/<id>.json` claim. Every post-wait
//! and pre-side-effect checkpoint inside `flow`, `pr ship`, and `pr train`
//! re-verifies that context from disk so a stopped/replaced mandate, revoked or
//! expired grant, tightened human gate, or invalidated worker claim aborts the
//! in-flight operation before any hold release or merge.
//!
//! Which review verdict and CI state let a flow ship is gate authority
//! ([`crate::gate::dispatch_authority::verdict_at_head`],
//! [`crate::gate::dispatch_authority::ci_state_is_green`], STORY-1889).
// trace:STORY-1917 trace:TASK-2344 | ai:agy

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use std::path::Path;
use std::time::Duration;

use crate::dispatch::{
    append_dispatch_log, open_pinned_dispatch_subdir, run_review_verb, DispatchVendor,
    RunnerOutcome,
};
use crate::gate::dispatch_auth::{is_dispatch_authority_aborted, DispatchExecutionContext};
use crate::gate::dispatch_authority::{
    ci_state_is_green, flow_ship_decision, verdict_at_head, DispatchRequest, ExecutionAdmission,
    FlowVerdict, WorkerClaim,
};

const MAX_FLOW_TRIES: usize = 4;
const FLOW_SHIP_WAIT_SECS: u64 = 3600;
const TRAIN_LOCK_FILE: &str = "train.lock";
/// Bound on one `gh` call the runners make (R6: bounded subprocesses).
const FLOW_GH_TIMEOUT: Duration = Duration::from_secs(120);
/// Bound on the `git fetch` after a branch update.
const FLOW_FETCH_TIMEOUT: Duration = Duration::from_secs(300);

/// Run `cmd` under `timeout`, naming `what` in the error when it cannot be
/// spawned or overruns, so a hung forge never holds a flow/train claim.
// trace:STORY-1917 | ai:claude
fn bounded_output(
    cmd: std::process::Command,
    timeout: Duration,
    what: &str,
) -> Result<std::process::Output> {
    match crate::command_output_with_timeout_detail(cmd, timeout) {
        crate::BoundedCommandOutput::Completed(out) => Ok(out),
        crate::BoundedCommandOutput::SpawnFailed => bail!("failed to run `{what}`"),
        crate::BoundedCommandOutput::TimedOut => {
            bail!(
                "`{what}` did not finish within {}s and was killed",
                timeout.as_secs()
            )
        }
    }
}

/// Exclusive flock over `.aida/dispatch/train.lock` (mode 0600, `O_NOFOLLOW`),
/// explicitly unlocked on Drop so only one `dispatch train` (or `flow` train
/// fallback) runs at a time.
// trace:STORY-1917 | ai:agy
pub(crate) struct TrainDispatchLock(std::fs::File);

impl Drop for TrainDispatchLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

pub(crate) fn acquire_train_dispatch_lock(main_worktree: &Path) -> Result<TrainDispatchLock> {
    use fs2::FileExt;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;

    let disp_fd = open_pinned_dispatch_subdir(main_worktree, None)?;
    let display_path = crate::gate::dispatch_io::dispatch_root(main_worktree).join(TRAIN_LOCK_FILE);
    let c_name = std::ffi::CString::new(TRAIN_LOCK_FILE)?;
    let fd = unsafe {
        libc::openat(
            disp_fd.as_raw_fd(),
            c_name.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ELOOP) {
            bail!("{} is a symlink (refused)", display_path.display());
        }
        return Err(err).with_context(|| format!("cannot open {}", display_path.display()));
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = file
        .metadata()
        .with_context(|| format!("cannot stat {}", display_path.display()))?;
    if !meta.is_file() {
        bail!("{} is not a regular file (refused)", display_path.display());
    }
    if meta.nlink() != 1 {
        bail!(
            "{} has {} hard links, expected 1 (refused)",
            display_path.display(),
            meta.nlink()
        );
    }
    let uid = unsafe { libc::geteuid() };
    if meta.uid() != uid {
        bail!(
            "{} is owned by uid {}, not current uid {uid}",
            display_path.display(),
            meta.uid()
        );
    }
    if (meta.mode() & 0o7777) != 0o600 {
        bail!(
            "{} has mode {:04o}, expected 0600 (refused)",
            display_path.display(),
            meta.mode() & 0o7777
        );
    }
    // Non-blocking: a second train refuses instead of parking a worker (and
    // its claim and cap slot) behind the first. trace:STORY-1917 | ai:claude
    if let Err(e) = file.try_lock_exclusive() {
        if e.kind() == std::io::ErrorKind::WouldBlock || e.raw_os_error() == Some(libc::EWOULDBLOCK)
        {
            bail!(
                "another dispatch train holds {}; this train refuses rather than wait",
                display_path.display()
            );
        }
        return Err(e).with_context(|| format!("cannot lock {}", display_path.display()));
    }
    Ok(TrainDispatchLock(file))
}

#[derive(Debug, Clone, serde::Deserialize)]
struct FlowPrView {
    #[serde(rename = "headRefOid")]
    head_ref_oid: String,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "baseRefName")]
    base_ref_name: String,
    state: String,
    #[serde(rename = "mergeStateStatus", default)]
    merge_state_status: String,
    #[serde(default)]
    title: String,
}

fn query_flow_pr_view(main_worktree: &Path, pr: u64) -> Result<FlowPrView> {
    let gh = crate::resolve_gh_binary()
        .ok_or_else(|| anyhow::anyhow!("cannot locate `gh` executable on PATH"))?;
    let pr_str = pr.to_string();
    let mut cmd = std::process::Command::new(gh);
    cmd.current_dir(main_worktree)
        .args([
            "pr",
            "view",
            &pr_str,
            "--json",
            "headRefOid,headRefName,baseRefName,state,mergeStateStatus,title",
        ])
        .stdin(std::process::Stdio::null());
    let out = bounded_output(cmd, FLOW_GH_TIMEOUT, &format!("gh pr view {pr}"))?;
    if !out.status.success() {
        bail!(
            "`gh pr view {pr}` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let view: FlowPrView = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("cannot parse `gh pr view {pr}` JSON"))?;
    if view.head_ref_oid.trim().is_empty() || view.head_ref_name.trim().is_empty() {
        bail!("`gh pr view {pr}` returned empty headRefOid or headRefName");
    }
    Ok(view)
}

/// Non-force `gh pr update-branch <pr>` followed by `git fetch origin`.
// trace:STORY-1917 | ai:agy
fn update_pr_branch_non_force(
    main_worktree: &Path,
    req_id: &str,
    pr: u64,
    base_ref: &str,
    head_ref: &str,
) -> Result<()> {
    let gh = crate::resolve_gh_binary()
        .ok_or_else(|| anyhow::anyhow!("cannot locate `gh` executable on PATH"))?;
    let pr_str = pr.to_string();
    let _ = append_dispatch_log(
        main_worktree,
        req_id,
        &format!("running `gh pr update-branch {pr}` (non-force)\n"),
    );
    let mut cmd = std::process::Command::new(gh);
    cmd.current_dir(main_worktree)
        .args(["pr", "update-branch", &pr_str])
        .stdin(std::process::Stdio::null());
    let out = bounded_output(cmd, FLOW_GH_TIMEOUT, &format!("gh pr update-branch {pr}"))?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = append_dispatch_log(main_worktree, req_id, &combined);
    if !out.status.success() {
        bail!(
            "`gh pr update-branch {pr}` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    // Refresh origin/<base> and origin/<head> in main_worktree if origin remote exists.
    let mut probe = aida_core::git_ops::host_git_command(main_worktree);
    probe.args(["remote", "get-url", "origin"]);
    let has_origin = bounded_output(probe, FLOW_GH_TIMEOUT, "git remote get-url origin")
        .map(|o| o.status.success())
        .unwrap_or(false);
    if has_origin {
        let mut fetch = aida_core::git_ops::host_remote_git_command(main_worktree, "origin");
        fetch.args([
            "fetch",
            "--quiet",
            "--no-tags",
            "origin",
            &format!("+refs/heads/{base_ref}:refs/remotes/origin/{base_ref}"),
            &format!("+refs/heads/{head_ref}:refs/remotes/origin/{head_ref}"),
        ]);
        // The fetch only refreshes local refs; every later step re-reads the
        // PR from the forge, so a failed or slow fetch is logged, not fatal.
        if let Err(e) = bounded_output(fetch, FLOW_FETCH_TIMEOUT, "git fetch origin") {
            let _ = append_dispatch_log(main_worktree, req_id, &format!("{e:#}\n"));
        }
    }
    Ok(())
}

#[cfg(test)]
type TestFlowShipHook = Box<dyn FnMut(u64, &DispatchExecutionContext) -> Result<()>>;
#[cfg(test)]
type TestFlowTrainHook = Box<dyn FnMut(&[u64], &DispatchExecutionContext) -> Result<()>>;
#[cfg(test)]
type TestFlowWaitCiHook = Box<dyn FnMut(&Path, u64, &DispatchExecutionContext, bool) -> Result<()>>;

#[cfg(test)]
std::thread_local! {
    pub(crate) static TEST_FLOW_SHIP_HOOK: std::cell::RefCell<Option<TestFlowShipHook>> =
        std::cell::RefCell::new(None);
    pub(crate) static TEST_FLOW_TRAIN_HOOK: std::cell::RefCell<Option<TestFlowTrainHook>> =
        std::cell::RefCell::new(None);
    pub(crate) static TEST_FLOW_WAIT_CI_HOOK: std::cell::RefCell<Option<TestFlowWaitCiHook>> =
        std::cell::RefCell::new(None);
}

fn invoke_pr_ship_with_ctx(pr: u64, ctx: &DispatchExecutionContext) -> Result<()> {
    #[cfg(test)]
    {
        let maybe_res =
            TEST_FLOW_SHIP_HOOK.with(|cell| cell.borrow_mut().as_mut().map(|hook| hook(pr, ctx)));
        if let Some(res) = maybe_res {
            return res;
        }
    }
    crate::pr_cmd::pr_ship_with_method_and_dispatch_ctx(
        Some(pr),
        false,
        false,
        false,
        false,
        None,
        None,
        false,
        false,
        false,
        Some(FLOW_SHIP_WAIT_SECS),
        false,
        crate::forge::MergeMethod::Squash,
        false,
        Some(ctx),
    )
}

fn invoke_pr_train_with_ctx(prs: &[u64], ctx: &DispatchExecutionContext) -> Result<()> {
    #[cfg(test)]
    {
        let maybe_res =
            TEST_FLOW_TRAIN_HOOK.with(|cell| cell.borrow_mut().as_mut().map(|hook| hook(prs, ctx)));
        if let Some(res) = maybe_res {
            return res;
        }
    }
    let opts = crate::pr_train::TrainOptions {
        prs,
        all_ready: false,
        dry_run: false,
        local_ci: None,
        max: prs.len(),
        no_pull: false,
    };
    crate::pr_train::pr_train_handler_with_dispatch_ctx(&opts, Some(ctx))
}

/// Wait for required CI checks on `pr` to settle, re-verifying `ctx` before and
/// after the wait.
// trace:STORY-1917 | ai:agy
fn wait_for_pr_ci_with_ctx(
    main_worktree: &Path,
    pr: u64,
    view: &FlowPrView,
    ctx: &DispatchExecutionContext,
    requires_ship_held: bool,
) -> Result<()> {
    ctx.reverify_now(requires_ship_held)?;
    #[cfg(test)]
    {
        let maybe_res = TEST_FLOW_WAIT_CI_HOOK.with(|cell| {
            cell.borrow_mut()
                .as_mut()
                .map(|hook| hook(main_worktree, pr, ctx, requires_ship_held))
        });
        if let Some(res) = maybe_res {
            ctx.reverify_now(requires_ship_held)?;
            return res;
        }
    }
    let forge = crate::forge::forge_for(main_worktree);
    let change = crate::forge::ChangeRef {
        id: pr,
        url: String::new(),
        branch: view.head_ref_name.clone(),
        base: view.base_ref_name.clone(),
        title: (!view.title.trim().is_empty()).then(|| view.title.clone()),
    };
    let hold_present = crate::gate::merge_hold_core::read_hold(main_worktree, pr).is_some();
    let label_only_hold =
        !hold_present && crate::merge_hold::label_present(main_worktree, pr).unwrap_or(false);
    let state = crate::ci_gate::wait_for_ship_ci(
        main_worktree,
        forge.as_ref(),
        &change,
        hold_present,
        label_only_hold,
        Some(Duration::from_secs(FLOW_SHIP_WAIT_SECS)),
        Duration::from_secs(10),
    )?;
    ctx.reverify_now(requires_ship_held)?;
    // R6: green means the forge reported success for the exact head. No CI,
    // a pending run or a failure is never reported as green.
    // trace:STORY-1917 | ai:claude
    ci_state_is_green(pr, state)
}

/// Verify `.aida-store` has no uncommitted changes before starting a train pass.
// trace:STORY-1917 | ai:agy
pub(crate) fn ensure_clean_aida_store(main_worktree: &Path) -> Result<()> {
    let store_dir = main_worktree.join(".aida-store");
    if !store_dir.exists() {
        return Ok(());
    }
    if store_dir.join(".git").exists() {
        let mut cmd = aida_core::git_ops::host_git_command(&store_dir);
        cmd.args(["status", "--porcelain"]);
        let out = bounded_output(cmd, FLOW_GH_TIMEOUT, "git status --porcelain (.aida-store)")?;
        if !out.status.success() {
            bail!(
                "cannot inspect .aida-store cleanliness: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let dirty = String::from_utf8_lossy(&out.stdout);
        if !dirty.trim().is_empty() {
            bail!(
                "refusing to run dispatch train with dirty .aida-store:\n{}",
                dirty.trim()
            );
        }
    }
    Ok(())
}

/// Ship a single approved PR in-process using `pr_ship_with_method_and_dispatch_ctx`.
/// If `pr ship` fails specifically because the PR is behind base:
/// - when held (or behind base), attempts `gh pr update-branch <pr>` + `git fetch`
///   and retries `pr ship` once;
/// - if still behind base, falls back to `pr_train_handler_with_dispatch_ctx`
///   under `.aida/dispatch/train.lock`.
// trace:STORY-1917 | ai:agy
fn ship_or_train_single_pr(
    main_worktree: &Path,
    req_id: &str,
    pr: u64,
    ctx: &DispatchExecutionContext,
    held_train_lock: bool,
) -> Result<String> {
    ctx.reverify_now(true)?;
    match invoke_pr_ship_with_ctx(pr, ctx) {
        Ok(()) => Ok(format!("shipped PR #{pr}")),
        Err(err) => {
            let msg = format!("{err:#}");
            let _ = append_dispatch_log(
                main_worktree,
                req_id,
                &format!("pr ship {pr} returned error: {msg}\n"),
            );
            if is_dispatch_authority_aborted(&err) {
                return Err(err);
            }
            // If authority was revoked or a hold/review gate refused, surface immediately.
            ctx.reverify_now(true)?;
            let Some(view) = query_flow_pr_view(main_worktree, pr).ok() else {
                return Err(err);
            };
            if !view.merge_state_status.eq_ignore_ascii_case("BEHIND") {
                return Err(err);
            }
            if update_pr_branch_non_force(
                main_worktree,
                req_id,
                pr,
                &view.base_ref_name,
                &view.head_ref_name,
            )
            .is_ok()
            {
                ctx.reverify_now(true)?;
                match invoke_pr_ship_with_ctx(pr, ctx) {
                    Ok(()) => return Ok(format!("updated branch and shipped PR #{pr}")),
                    Err(retry_err) => {
                        let retry_msg = format!("{retry_err:#}");
                        let _ = append_dispatch_log(
                            main_worktree,
                            req_id,
                            &format!("pr ship {pr} retry returned error: {retry_msg}\n"),
                        );
                        if is_dispatch_authority_aborted(&retry_err) {
                            return Err(retry_err);
                        }
                        ctx.reverify_now(true)?;
                        let still_behind = query_flow_pr_view(main_worktree, pr)
                            .map(|v| v.merge_state_status.eq_ignore_ascii_case("BEHIND"))
                            .unwrap_or(false);
                        if !still_behind {
                            return Err(retry_err);
                        }
                    }
                }
            }
            ctx.reverify_now(true)?;
            let _guard = if held_train_lock {
                None
            } else {
                Some(acquire_train_dispatch_lock(main_worktree)?)
            };
            ensure_clean_aida_store(main_worktree)?;
            let prs = [pr];
            invoke_pr_train_with_ctx(&prs, ctx)?;
            Ok(format!("landed PR #{pr} via merge train"))
        }
    }
}

/// Execute `aida dispatch request flow --pr <pr> [--no-ship] [--vendor <v>]`.
// trace:STORY-1917 | ai:agy
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_flow_verb(
    main_worktree: &Path,
    mandate_dir: &Path,
    req: &DispatchRequest,
    admission: &ExecutionAdmission,
    claim: &WorkerClaim,
    pr: u64,
    no_ship: bool,
    vendor: Option<DispatchVendor>,
    now: DateTime<Utc>,
) -> RunnerOutcome {
    let ctx = match DispatchExecutionContext::establish(
        main_worktree,
        mandate_dir,
        &req.id(),
        &claim.claim_token,
        now,
    ) {
        Ok(c) => c,
        Err(err) => {
            return RunnerOutcome::Refused {
                reason: format!("failed to establish dispatch execution context: {err:#}"),
            };
        }
    };

    let initial_view = match query_flow_pr_view(main_worktree, pr) {
        Ok(v) => v,
        Err(err) => {
            return RunnerOutcome::Failed {
                summary: format!("failed to inspect PR #{pr}: {err:#}"),
                exit_code: None,
            };
        }
    };
    if !initial_view.state.eq_ignore_ascii_case("OPEN") {
        return RunnerOutcome::Refused {
            reason: format!("PR #{pr} is not OPEN (state is `{}`)", initial_view.state),
        };
    }

    // Approved-at-head fast path: if `!no_ship` and PR already has a current-head
    // APPROVED verdict (and is not BEHIND), jump straight to the ship/train phase.
    if !no_ship
        && !initial_view
            .merge_state_status
            .eq_ignore_ascii_case("BEHIND")
        && flow_ship_decision(verdict_at_head(
            main_worktree,
            pr,
            &initial_view.head_ref_oid,
        )) == FlowVerdict::Ship
    {
        let _ = append_dispatch_log(
            main_worktree,
            &req.id(),
            &format!(
                "fast-path: PR #{pr} already has APPROVED verdict at head {}; shipping\n",
                initial_view.head_ref_oid
            ),
        );
        return match ship_or_train_single_pr(main_worktree, &req.id(), pr, &ctx, false) {
            Ok(summary) => RunnerOutcome::Done {
                summary,
                spawned_session: None,
                exit_code: Some(0),
            },
            Err(err) => {
                let msg = format!("{err:#}");
                if is_dispatch_authority_aborted(&err) || ctx.reverify_now(true).is_err() {
                    RunnerOutcome::Refused { reason: msg }
                } else {
                    RunnerOutcome::Failed {
                        summary: format!("flow ship failed for PR #{pr}: {msg}"),
                        exit_code: Some(1),
                    }
                }
            }
        };
    }

    for attempt in 1..=MAX_FLOW_TRIES {
        if let Err(err) = ctx.reverify_now(!no_ship) {
            return RunnerOutcome::Refused {
                reason: format!("{err:#}"),
            };
        }

        let mut view = match query_flow_pr_view(main_worktree, pr) {
            Ok(v) => v,
            Err(err) => {
                return RunnerOutcome::Failed {
                    summary: format!("failed to inspect PR #{pr} (attempt {attempt}): {err:#}"),
                    exit_code: None,
                };
            }
        };
        if !view.state.eq_ignore_ascii_case("OPEN") {
            return RunnerOutcome::Refused {
                reason: format!("PR #{pr} is no longer OPEN (state `{}`)", view.state),
            };
        }

        // Step 1: if BEHIND base, update-branch (non-force) and re-query head SHA.
        if view.merge_state_status.eq_ignore_ascii_case("BEHIND") {
            if let Err(err) = ctx.reverify_now(!no_ship) {
                return RunnerOutcome::Refused {
                    reason: format!("{err:#}"),
                };
            }
            if let Err(err) = update_pr_branch_non_force(
                main_worktree,
                &req.id(),
                pr,
                &view.base_ref_name,
                &view.head_ref_name,
            ) {
                return RunnerOutcome::Failed {
                    summary: format!("failed to update behind branch on PR #{pr}: {err:#}"),
                    exit_code: Some(1),
                };
            }
            view = match query_flow_pr_view(main_worktree, pr) {
                Ok(v) => v,
                Err(err) => {
                    return RunnerOutcome::Failed {
                        summary: format!("failed to re-read PR #{pr} after update-branch: {err:#}"),
                        exit_code: None,
                    };
                }
            };
        }

        // Step 2: wait for exact-head required CI to settle green.
        if let Err(err) = wait_for_pr_ci_with_ctx(main_worktree, pr, &view, &ctx, !no_ship) {
            let msg = format!("{err:#}");
            if is_dispatch_authority_aborted(&err) || ctx.reverify_now(!no_ship).is_err() {
                return RunnerOutcome::Refused { reason: msg };
            }
            return RunnerOutcome::Failed {
                summary: format!("CI did not pass for PR #{pr}: {msg}"),
                exit_code: Some(1),
            };
        }

        // Step 3: ensure an attested review verdict covers `view.head_ref_oid`.
        let current_verdict = match verdict_at_head(main_worktree, pr, &view.head_ref_oid) {
            Some(kind) => Some(kind),
            None => {
                if let Err(err) = ctx.reverify_now(!no_ship) {
                    return RunnerOutcome::Refused {
                        reason: format!("{err:#}"),
                    };
                }
                match run_review_verb(main_worktree, req, admission, pr, vendor) {
                    RunnerOutcome::Done { .. } => {}
                    other => return other,
                }
                if let Err(err) = ctx.reverify_now(!no_ship) {
                    return RunnerOutcome::Refused {
                        reason: format!("{err:#}"),
                    };
                }
                verdict_at_head(main_worktree, pr, &view.head_ref_oid)
            }
        };

        match flow_ship_decision(current_verdict) {
            FlowVerdict::Ship => {}
            FlowVerdict::ChangesRequested => {
                // Stop immediately on CHANGES_REQUESTED / REJECTED without clearing any hold!
                return RunnerOutcome::Failed {
                    summary: format!(
                        "PR #{pr} review verdict is CHANGES_REQUESTED at head {}; stopping with hold intact",
                        &view.head_ref_oid[..view.head_ref_oid.len().min(10)]
                    ),
                    exit_code: Some(1),
                };
            }
            FlowVerdict::NoVerdict => {
                return RunnerOutcome::Failed {
                    summary: format!(
                        "PR #{pr} review completed without a valid verdict pinned to head {}",
                        &view.head_ref_oid[..view.head_ref_oid.len().min(10)]
                    ),
                    exit_code: Some(1),
                };
            }
        }

        // Step 4: re-read PR metadata after review. If head moved or became BEHIND,
        // loop again (up to MAX_FLOW_TRIES) so CI and review cover the new head.
        let post_view = match query_flow_pr_view(main_worktree, pr) {
            Ok(v) => v,
            Err(err) => {
                return RunnerOutcome::Failed {
                    summary: format!("failed to re-read PR #{pr} after review: {err:#}"),
                    exit_code: None,
                };
            }
        };
        if post_view.head_ref_oid != view.head_ref_oid
            || post_view.merge_state_status.eq_ignore_ascii_case("BEHIND")
        {
            let _ = append_dispatch_log(
                main_worktree,
                &req.id(),
                &format!(
                    "PR #{pr} head/mergeState moved during attempt {attempt} ({} -> {}, mergeState={}); retrying\n",
                    view.head_ref_oid, post_view.head_ref_oid, post_view.merge_state_status
                ),
            );
            continue;
        }

        if no_ship {
            return RunnerOutcome::Done {
                summary: format!(
                    "PR #{pr} approved at head {} with green CI (--no-ship)",
                    &post_view.head_ref_oid[..post_view.head_ref_oid.len().min(10)]
                ),
                spawned_session: None,
                exit_code: Some(0),
            };
        }

        return match ship_or_train_single_pr(main_worktree, &req.id(), pr, &ctx, false) {
            Ok(summary) => RunnerOutcome::Done {
                summary,
                spawned_session: None,
                exit_code: Some(0),
            },
            Err(err) => {
                let msg = format!("{err:#}");
                if is_dispatch_authority_aborted(&err) || ctx.reverify_now(true).is_err() {
                    RunnerOutcome::Refused { reason: msg }
                } else {
                    RunnerOutcome::Failed {
                        summary: format!("flow ship failed for PR #{pr}: {msg}"),
                        exit_code: Some(1),
                    }
                }
            }
        };
    }

    RunnerOutcome::Failed {
        summary: format!(
            "PR #{pr} did not converge after {MAX_FLOW_TRIES} update/CI/review attempts"
        ),
        exit_code: Some(1),
    }
}

/// Execute `aida dispatch request train <prs...>`.
// trace:STORY-1917 | ai:agy
pub(crate) fn run_train_verb(
    main_worktree: &Path,
    mandate_dir: &Path,
    req: &DispatchRequest,
    admission: &ExecutionAdmission,
    claim: &WorkerClaim,
    prs: &[u64],
    now: DateTime<Utc>,
) -> RunnerOutcome {
    let _train_lock = match acquire_train_dispatch_lock(main_worktree) {
        Ok(l) => l,
        Err(err) => {
            return RunnerOutcome::Refused {
                reason: format!("cannot acquire dispatch train.lock: {err:#}"),
            };
        }
    };

    if let Err(err) = ensure_clean_aida_store(main_worktree) {
        return RunnerOutcome::Refused {
            reason: format!("{err:#}"),
        };
    }

    let ctx = match DispatchExecutionContext::establish(
        main_worktree,
        mandate_dir,
        &req.id(),
        &claim.claim_token,
        now,
    ) {
        Ok(c) => c,
        Err(err) => {
            return RunnerOutcome::Refused {
                reason: format!("failed to establish dispatch execution context: {err:#}"),
            };
        }
    };

    let mut landed_notes = Vec::new();
    for &pr in prs {
        if let Err(err) = ctx.reverify_now(true) {
            return RunnerOutcome::Refused {
                reason: format!("{err:#}"),
            };
        }
        if let Err(err) = ensure_clean_aida_store(main_worktree) {
            return RunnerOutcome::Refused {
                reason: format!("{err:#}"),
            };
        }
        match ship_or_train_single_pr(main_worktree, &req.id(), pr, &ctx, true) {
            Ok(note) => landed_notes.push(note),
            Err(err) => {
                let msg = format!("{err:#}");
                if is_dispatch_authority_aborted(&err) || ctx.reverify_now(true).is_err() {
                    return RunnerOutcome::Refused { reason: msg };
                }
                return RunnerOutcome::Failed {
                    summary: format!(
                        "train stopped on PR #{pr} (landed {} before failure): {msg}",
                        landed_notes.len()
                    ),
                    exit_code: Some(1),
                };
            }
        }
    }

    RunnerOutcome::Done {
        summary: format!(
            "completed train for {} PR(s): {}",
            prs.len(),
            landed_notes.join("; ")
        ),
        spawned_session: None,
        exit_code: Some(0),
    }
}
