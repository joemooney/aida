//! Dispatch authority (STORY-1889, moved verbatim from `dispatch.rs` and
//! `dispatch_flow.rs`): what an admitted request record means, the target and
//! spec restrictions, request-time mandate / human-gate / advisor-grant
//! admission, execution-time re-admission, worker claim ownership and the
//! concurrency caps, and which review verdict and CI state let `flow` ship.
//! `dispatch.rs` keeps the orchestration (paths, bounded file I/O, brief
//! copying, prompt rendering, publication, spawning, runners, CLI) and calls
//! in here. The `tick.lock` and the recovery ledger are
//! [`crate::gate::dispatch_lock`] and [`crate::gate::dispatch_ledger`].
// mandate-gate
// trace:STORY-1889 trace:STORY-1917 trace:TASK-2344 | ai:claude

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;
use uuid::Uuid;

use crate::dispatch::{DispatchRequestVerb, DispatchStatusFilter, DispatchVendor};
use crate::gate::dispatch_deps::{ActionRow, Outcome};
use crate::gate::dispatch_io::{
    brief_path, read_bounded_regular_file, result_path, sha256_hex, BRIEFS_SUBDIR, MAX_BRIEF_BYTES,
};
use crate::gate::dispatch_ledger::{
    reconcile_inventory_locked, DispatchInventory, RECOVERY_GUIDANCE,
};
use crate::gate::mandate::{Mandate, MandateState, Scope};
use crate::gate::mandate_gates::GateOp;
use crate::gate::review_verdict::{self, VerdictKind};
use crate::gate::seat_authority::{GrantCheck, GrantRejection};

pub(crate) const DISPATCH_SCHEMA_VERSION: u32 = 1;

#[cfg(test)]
std::thread_local! {
    pub(crate) static TEST_FORCE_WORKER_PROBE: std::cell::RefCell<Option<aida_core::process_probe::Probe>> = const { std::cell::RefCell::new(None) };
}

pub(crate) const MAX_TRAIN_PRS: usize = 16;
pub(crate) const MAX_PR_NUMBER: u64 = 999_999;

/// On-disk request record (`requests/<id>.json`, mode 0600).
// trace:STORY-1917 | ai:agy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DispatchRequest {
    // Gate-private (STORY-1889): only `admitted_request` builds a record, and
    // the orchestration reads it through the accessors below.
    pub(in crate::gate) schema_version: u32,
    pub(in crate::gate) id: String,
    pub(in crate::gate) requested_at: DateTime<Utc>,
    pub(in crate::gate) principal: String,
    pub(in crate::gate) requester_seat: String,
    pub(in crate::gate) requester_grant_id: String,
    pub(in crate::gate) matched_binding: Vec<String>,
    pub(in crate::gate) mandate_id: String,
    pub(in crate::gate) dedupe_key: String,
    pub(in crate::gate) target_key: String,
    pub(in crate::gate) action: DispatchVerb,
}

// trace:STORY-1889 | ai:claude
impl DispatchRequest {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn requested_at(&self) -> DateTime<Utc> {
        self.requested_at
    }

    pub(crate) fn principal(&self) -> &str {
        &self.principal
    }

    pub(crate) fn requester_seat(&self) -> &str {
        &self.requester_seat
    }

    pub(crate) fn requester_grant_id(&self) -> &str {
        &self.requester_grant_id
    }

    pub(crate) fn matched_binding(&self) -> &[String] {
        &self.matched_binding
    }

    pub(crate) fn mandate_id(&self) -> &str {
        &self.mandate_id
    }

    pub(crate) fn dedupe_key(&self) -> &str {
        &self.dedupe_key
    }

    pub(crate) fn target_key(&self) -> &str {
        &self.target_key
    }

    pub(crate) fn action(&self) -> &DispatchVerb {
        &self.action
    }
}

/// Closed verbs persisted in [`DispatchRequest`].
// trace:STORY-1917 | ai:agy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum DispatchVerb {
    #[serde(alias = "pr-flow")]
    Flow {
        pr: u64,
        #[serde(default)]
        no_ship: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vendor: Option<DispatchVendor>,
    },
    Review {
        pr: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vendor: Option<DispatchVendor>,
    },
    #[serde(alias = "takeover")]
    Take {
        pr: u64,
        branch: String,
        spec: String,
        brief_relpath: String,
        brief_sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vendor: Option<DispatchVendor>,
    },
    #[serde(alias = "lane")]
    Impl {
        name: String,
        branch: String,
        spec: String,
        brief_relpath: String,
        brief_sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vendor: Option<DispatchVendor>,
    },
    Train {
        prs: Vec<u64>,
    },
}

impl DispatchVerb {
    pub(crate) fn verb_str(&self) -> &'static str {
        match self {
            Self::Flow { .. } => "flow",
            Self::Review { .. } => "review",
            Self::Take { .. } => "take",
            Self::Impl { .. } => "impl",
            Self::Train { .. } => "train",
        }
    }

    /// True when this verb requires the advisor seat grant at execution time
    /// and therefore runs only under `aida dispatch serve`, never under the
    /// cron/schedule `dispatch tick`.
    // trace:STORY-1917 | ai:agy
    pub(crate) fn requires_serve(&self) -> bool {
        matches!(
            self,
            Self::Flow { .. } | Self::Review { .. } | Self::Train { .. }
        )
    }

    /// True when this verb ships held PRs (`flow` without `--no-ship`, or `train`).
    pub(crate) fn ships_pr(&self) -> bool {
        matches!(self, Self::Flow { no_ship: false, .. } | Self::Train { .. })
    }

    /// True when this verb delegates a reviewer seat (`review`, or `flow`).
    pub(crate) fn delegates_reviewer(&self) -> bool {
        matches!(self, Self::Flow { .. } | Self::Review { .. })
    }

    pub(crate) fn target_key(&self) -> String {
        match self {
            Self::Flow { pr, .. } | Self::Review { pr, .. } | Self::Take { pr, .. } => {
                format!("pr:{pr}")
            }
            Self::Impl { name, spec, .. } => {
                format!("lane:{}:spec:{}", name, spec.to_ascii_uppercase())
            }
            Self::Train { .. } => "train".to_string(),
        }
    }

    pub(crate) fn dedupe_key(&self) -> String {
        match self {
            Self::Flow { pr, no_ship, .. } => format!("flow:pr:{pr}:noship:{no_ship}"),
            // Request queueing runs offline/store-less inside a sandboxed
            // advisor seat where forge/network queries for the current PR head
            // OID may be unavailable, so review deduplicates on active
            // (`Pending`/`Running`) requests for `pr:<pr>`; once a review
            // finishes (`Done`/`Failed`/`Refused`/`Cancelled`), a new review
            // request for a newer head queues immediately.
            Self::Review { pr, .. } => format!("review:pr:{pr}"),
            Self::Take {
                pr,
                branch,
                spec,
                brief_sha256,
                ..
            } => format!(
                "take:pr:{pr}:branch:{branch}:spec:{}:brief:{brief_sha256}",
                spec.to_ascii_uppercase()
            ),
            Self::Impl {
                name, branch, spec, ..
            } => format!(
                "impl:name:{name}:branch:{branch}:spec:{}",
                spec.to_ascii_uppercase()
            ),
            Self::Train { prs } => {
                let mut sorted = prs.clone();
                sorted.sort_unstable();
                sorted.dedup();
                let csv = sorted
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                format!("train:prs:{csv}")
            }
        }
    }

    pub(crate) fn subject_label(&self) -> String {
        match self {
            Self::Flow { pr, .. } | Self::Review { pr, .. } | Self::Take { pr, .. } => {
                format!("PR #{pr}")
            }
            Self::Impl { name, spec, .. } => format!("{name} ({})", spec.to_ascii_uppercase()),
            Self::Train { prs } => {
                let list = prs
                    .iter()
                    .map(|n| format!("#{n}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("PRs {list}")
            }
        }
    }
}

/// Execution status of a dispatch request.
// trace:STORY-1917 | ai:agy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DispatchStatus {
    Pending,
    Running,
    Done,
    Refused,
    Failed,
    Cancelled,
}

impl DispatchStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Refused => "refused",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }

    pub(crate) fn matches_filter(self, filter: DispatchStatusFilter) -> bool {
        matches!(
            (self, filter),
            (Self::Pending, DispatchStatusFilter::Pending)
                | (Self::Running, DispatchStatusFilter::Running)
                | (Self::Done, DispatchStatusFilter::Done)
                | (Self::Refused, DispatchStatusFilter::Refused)
                | (Self::Failed, DispatchStatusFilter::Failed)
                | (Self::Cancelled, DispatchStatusFilter::Cancelled)
        )
    }
}

/// On-disk result record (`results/<id>.json`, mode 0600).
// trace:STORY-1917 | ai:agy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DispatchResult {
    // The record identity, its mandate and requester grant, the worker claim
    // and the status with its timestamps and summary are gate-private
    // (STORY-1889): `pending_result` binds the first from the admission, and
    // only the transition methods below change the claim or the status.
    pub(in crate::gate) id: String,
    pub verb: String,
    pub target: String,
    pub dedupe_key: String,
    pub(in crate::gate) status: DispatchStatus,
    pub(in crate::gate) requested_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::gate) started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::gate) finished_at: Option<DateTime<Utc>>,
    pub(in crate::gate) requester_grant_prefix: String,
    pub(in crate::gate) mandate_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::gate) worker_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::gate) worker_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::gate) claim_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub(in crate::gate) summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal_reason: Option<String>,
    pub log_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_at: Option<DateTime<Utc>>,
}

// trace:STORY-1889 | ai:claude
impl DispatchResult {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn status(&self) -> DispatchStatus {
        self.status
    }

    pub(crate) fn started_at(&self) -> Option<DateTime<Utc>> {
        self.started_at
    }

    pub(crate) fn finished_at(&self) -> Option<DateTime<Utc>> {
        self.finished_at
    }

    pub(crate) fn summary(&self) -> &str {
        &self.summary
    }

    pub(crate) fn requested_at(&self) -> DateTime<Utc> {
        self.requested_at
    }

    pub(crate) fn requester_grant_prefix(&self) -> &str {
        &self.requester_grant_prefix
    }

    pub(crate) fn mandate_id(&self) -> &str {
        &self.mandate_id
    }

    pub(crate) fn worker_pid(&self) -> Option<u32> {
        self.worker_pid
    }

    pub(crate) fn worker_start(&self) -> Option<&str> {
        self.worker_start.as_deref()
    }

    pub(crate) fn claim_token(&self) -> Option<&str> {
        self.claim_token.as_deref()
    }

    /// Whether the record carries no worker and no claim token.
    pub(crate) fn is_unclaimed(&self) -> bool {
        self.worker_pid.is_none() && self.claim_token.is_none()
    }

    /// Whether `(pid, start, token)` is exactly the recorded claim.
    pub(crate) fn claim_is(&self, pid: u32, start: Option<&str>, token: Option<&str>) -> bool {
        self.worker_pid == Some(pid)
            && self.worker_start.as_deref() == start
            && self.claim_token.as_deref() == token
    }

    /// Record a fresh claim `token`, held by `worker` once it is known.
    fn set_claim(&mut self, worker: Option<(u32, Option<String>)>, token: String) {
        let (pid, start) = worker.map_or((None, None), |(p, s)| (Some(p), s));
        self.worker_pid = pid;
        self.worker_start = start;
        self.claim_token = Some(token);
    }

    /// Drop the worker and its claim token.
    fn clear_claim(&mut self) {
        self.worker_pid = None;
        self.worker_start = None;
        self.claim_token = None;
    }

    // ── Record-status transitions ───────────────────────────────────────────

    /// `Pending` -> `Cancelled`; only a `Pending` request can be cancelled.
    pub(crate) fn cancel(&mut self, req: &DispatchRequest, now: DateTime<Utc>) -> Result<()> {
        if self.status != DispatchStatus::Pending {
            bail!(
                "cannot cancel dispatch request {}: status is `{}` (only `pending` requests can be cancelled)",
                req.id,
                self.status.as_str()
            );
        }
        self.status = DispatchStatus::Cancelled;
        self.finished_at = Some(now);
        self.summary = format!(
            "cancelled {} {}",
            req.action.verb_str(),
            req.action.subject_label()
        );
        Ok(())
    }

    /// `Pending` -> `Running` under a fresh claim `token`, held by `worker`
    /// once it is known. Only a [`ClaimAdmission`] from [`pass_admission`] or
    /// [`run_admission`] admits the claim. `req`, when already read, names
    /// the run in the summary.
    pub(crate) fn begin_running(
        &mut self,
        _admission: ClaimAdmission,
        req: Option<&DispatchRequest>,
        worker: Option<(u32, Option<String>)>,
        token: String,
        now: DateTime<Utc>,
    ) -> Result<()> {
        if self.status != DispatchStatus::Pending {
            bail!(
                "cannot claim dispatch request {}: status is `{}`, not `pending`",
                self.id,
                self.status.as_str()
            );
        }
        self.status = DispatchStatus::Running;
        self.started_at = Some(now);
        self.set_claim(worker, token);
        if let Some(req) = req {
            self.summary = format!(
                "running {} {}",
                req.action.verb_str(),
                req.action.subject_label()
            );
        }
        Ok(())
    }

    /// Publish the spawned worker behind this record's `Running` claim.
    pub(crate) fn publish_worker(
        &mut self,
        req: &DispatchRequest,
        pid: u32,
        start: Option<String>,
    ) {
        self.worker_pid = Some(pid);
        self.worker_start = start;
        self.summary = format!(
            "running {} {} (pid {pid})",
            req.action.verb_str(),
            req.action.subject_label()
        );
    }

    /// `Running` -> `Failed` with `summary` and `reason`. `release_claim`
    /// drops the worker and claim token (a worker that never started).
    pub(crate) fn fail_running(
        &mut self,
        summary: String,
        reason: String,
        release_claim: bool,
        now: DateTime<Utc>,
    ) -> Result<()> {
        if self.status != DispatchStatus::Running {
            bail!(
                "cannot fail dispatch request {}: status is `{}`, not `running`",
                self.id,
                self.status.as_str()
            );
        }
        self.status = DispatchStatus::Failed;
        self.finished_at = Some(now);
        if release_claim {
            self.clear_claim();
        }
        self.summary = summary;
        self.refusal_reason = Some(reason);
        Ok(())
    }

    /// `Running` -> `Pending` when the serve grant expired before this
    /// process started the claimed request: only a claim exactly
    /// `(pid, start, token)` reverts. Returns whether it did.
    pub(crate) fn revert_unstarted_claim(
        &mut self,
        pid: u32,
        start: Option<&str>,
        token: Option<&str>,
    ) -> bool {
        if self.status != DispatchStatus::Running || !self.claim_is(pid, start, token) {
            return false;
        }
        self.status = DispatchStatus::Pending;
        self.started_at = None;
        self.clear_claim();
        true
    }

    /// Which execution path a record admits; a terminal record refuses.
    pub(crate) fn execution_entry(&self, id: &str) -> Result<ExecutionEntry> {
        match self.status {
            DispatchStatus::Running => Ok(ExecutionEntry::Running),
            DispatchStatus::Pending => Ok(ExecutionEntry::Pending),
            DispatchStatus::Done
            | DispatchStatus::Refused
            | DispatchStatus::Failed
            | DispatchStatus::Cancelled => bail!(
                "dispatch request {id} is already in terminal state `{}` (refusing execution)",
                self.status.as_str()
            ),
        }
    }

    /// `Running` -> the terminal `status` the claimed execution ended in.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn finish(
        &mut self,
        status: DispatchStatus,
        summary: String,
        refusal_reason: Option<String>,
        spawned_session: Option<String>,
        exit_code: Option<i32>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        if status.is_active() {
            bail!(
                "dispatch request {} cannot finish in active status `{}`",
                self.id,
                status.as_str()
            );
        }
        if self.status != DispatchStatus::Running {
            bail!(
                "cannot finish dispatch request {}: status is `{}`, not `running`",
                self.id,
                self.status.as_str()
            );
        }
        self.status = status;
        self.finished_at = Some(now);
        self.spawned_session = spawned_session;
        self.exit_code = exit_code;
        self.summary = summary;
        self.refusal_reason = refusal_reason;
        Ok(())
    }
}

/// The execution path [`DispatchResult::execution_entry`] admits.
// trace:STORY-1889 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExecutionEntry {
    /// Verify the existing claim belongs to this process.
    Running,
    /// Claim it now, through [`run_admission`].
    Pending,
}

/// An active request already holding a new submission's dedupe key or
/// target.
// trace:STORY-1889 trace:STORY-1917 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SubmissionCollision {
    /// The same request is already active: return it, deduplicated.
    Duplicate {
        request: DispatchRequest,
        result: DispatchResult,
    },
    /// Another active request owns the target: refuse with this reason.
    TargetBusy(String),
}

/// Submission admission against the active requests in `inv`: the first
/// active pair with the same dedupe key deduplicates, one with the same
/// target refuses.
// trace:STORY-1889 trace:STORY-1917 | ai:claude
pub(crate) fn submission_collision(
    inv: &DispatchInventory,
    dedupe_key: &str,
    target_key: &str,
) -> Option<SubmissionCollision> {
    for (existing_req, existing_res) in &inv.pairs {
        if !existing_res.status.is_active() {
            continue;
        }
        if existing_req.dedupe_key == dedupe_key {
            return Some(SubmissionCollision::Duplicate {
                request: existing_req.clone(),
                result: existing_res.clone(),
            });
        }
        if existing_req.target_key == target_key {
            return Some(SubmissionCollision::TargetBusy(format!(
                "target `{target_key}` already has an active dispatch request {} ({}, status: {}); cancel it first with `aida dispatch cancel {}`",
                existing_req.id,
                existing_req.action.verb_str(),
                existing_res.status.as_str(),
                existing_req.id
            )));
        }
    }
    None
}

pub(crate) const MAX_CONCURRENT_REVIEWS: usize = 2;
pub(crate) const MAX_CONCURRENT_FLOWS: usize = 4;
pub(crate) const MAX_CONCURRENT_TRAINS: usize = 1;

/// Fenced worker claim owning an in-flight `Running` dispatch record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkerClaim {
    pub(crate) worker_pid: u32,
    pub(crate) worker_start: Option<String>,
    pub(crate) claim_token: String,
}

// ── Dual-End Validation ─────────────────────────────────────────────────────

/// Validate a PR number (`1..=999_999`).
// trace:STORY-1917 | ai:agy
pub(crate) fn validate_pr(pr: u64) -> Result<u64> {
    if (1..=MAX_PR_NUMBER).contains(&pr) {
        Ok(pr)
    } else {
        bail!("invalid PR number `{pr}`: must be between 1 and {MAX_PR_NUMBER}")
    }
}

/// Validate a `train` PR list (`1..=16` items, each valid, deduplicated in order).
// trace:STORY-1917 | ai:agy
pub(crate) fn validate_prs(prs: &[u64]) -> Result<Vec<u64>> {
    if prs.is_empty() {
        bail!("train requires at least 1 PR number");
    }
    if prs.len() > MAX_TRAIN_PRS {
        bail!(
            "train accepts at most {MAX_TRAIN_PRS} PR numbers (got {})",
            prs.len()
        );
    }
    let mut out = Vec::with_capacity(prs.len());
    let mut seen = BTreeSet::new();
    for &pr in prs {
        validate_pr(pr)?;
        if seen.insert(pr) {
            out.push(pr);
        }
    }
    Ok(out)
}

/// Resolve and verify the repository's default branch strictly from local
/// `refs/remotes/origin/HEAD` without network calls, config writes, or
/// conventional-branch (`main`/`master`) fallback (TASK-2341, R2).
///
/// Fails closed if `refs/remotes/origin/HEAD` is missing, unreadable, points
/// outside `refs/remotes/origin/*`, or its target ref is dangling or does not
/// resolve to a commit object.
// trace:TASK-2341 | ai:agy
pub(crate) fn resolve_verified_local_default_branch(main_worktree: &Path) -> Result<String> {
    let out = aida_core::git_ops::host_git_command(main_worktree)
        .args(["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"])
        .output()
        .with_context(|| {
            format!(
                "cannot read local `refs/remotes/origin/HEAD` in {}",
                main_worktree.display()
            )
        })?;
    if !out.status.success() {
        bail!(
            "cannot verify repository default branch from local `refs/remotes/origin/HEAD` (symbolic-ref is missing or unreadable); set or refresh it locally with `git symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/<default>` or `git remote set-head origin -a`"
        );
    }
    let raw = std::str::from_utf8(&out.stdout).map_err(|_| {
        anyhow::anyhow!(
            "cannot verify repository default branch from local `refs/remotes/origin/HEAD` (non-UTF-8 symbolic-ref target)"
        )
    })?;
    let symref = raw.trim_end_matches(['\r', '\n']);
    let Some(default_branch) = symref.strip_prefix("refs/remotes/origin/") else {
        bail!(
            "cannot verify repository default branch from local `refs/remotes/origin/HEAD` (points to non-origin target `{symref}`); set it to `refs/remotes/origin/<default>`"
        );
    };
    if default_branch.is_empty()
        || default_branch == "HEAD"
        || default_branch.starts_with('/')
        || default_branch.ends_with('/')
        || default_branch.contains("..")
        || default_branch.contains("//")
        || default_branch
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        bail!(
            "cannot verify repository default branch from local `refs/remotes/origin/HEAD` (malformed default branch `{default_branch}`)"
        );
    }

    let target_commit_spec = format!("refs/remotes/origin/{default_branch}^{{commit}}");
    let rev_out = aida_core::git_ops::host_git_command(main_worktree)
        .args(["rev-parse", "--verify", "--quiet", &target_commit_spec])
        .output()
        .with_context(|| {
            format!(
                "cannot verify `{target_commit_spec}` in {}",
                main_worktree.display()
            )
        })?;
    let commit_sha = std::str::from_utf8(&rev_out.stdout)
        .ok()
        .map(str::trim)
        .unwrap_or("");
    if !rev_out.status.success()
        || commit_sha.is_empty()
        || !commit_sha.chars().all(|c| c.is_ascii_hexdigit())
    {
        bail!(
            "cannot verify repository default branch from local `refs/remotes/origin/HEAD` (`refs/remotes/origin/{default_branch}` is missing, dangling, or does not resolve to a commit); refresh local remote refs or run `git symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/<default>`"
        );
    }

    Ok(default_branch.to_string())
}

/// Validate a branch name (`^[A-Za-z0-9._/-]{1,80}$` plus git check-ref-format
/// rules, forbidding `main`, `master`, `HEAD`, and the repository's verified
/// local default branch from `refs/remotes/origin/HEAD`).
// trace:STORY-1917 trace:TASK-2341 | ai:agy
pub(crate) fn validate_branch(main_worktree: &Path, branch: &str) -> Result<String> {
    let b = branch.trim();
    if b.is_empty() || b.len() > 80 {
        bail!("invalid branch `{branch}`: length must be 1..=80 characters");
    }
    if matches!(b, "main" | "master" | "HEAD") {
        bail!("invalid branch `{b}`: cannot target `{b}` directly");
    }
    if b.starts_with('/')
        || b.ends_with('/')
        || b.starts_with('.')
        || b.ends_with('.')
        || b.starts_with('-')
    {
        bail!("invalid branch `{b}`: cannot start or end with `/`, `.`, or `-`");
    }
    if b.contains("..") || b.contains("//") || b.contains("@{") || b.ends_with(".lock") {
        bail!("invalid branch `{b}`: contains forbidden git ref sequence");
    }
    for segment in b.split('/') {
        if segment.is_empty()
            || segment.starts_with('.')
            || segment.ends_with('.')
            || segment.ends_with(".lock")
        {
            bail!("invalid branch `{b}`: invalid path component `{segment}`");
        }
    }
    if !b
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
    {
        bail!("invalid branch `{b}`: only `[A-Za-z0-9._/-]` characters are allowed");
    }
    let default_branch = resolve_verified_local_default_branch(main_worktree)?;
    if b == default_branch {
        bail!("invalid branch `{b}`: cannot target the repository default branch `{default_branch}` directly");
    }
    Ok(b.to_string())
}

/// Validate an `impl` lane / worktree name (`^[A-Za-z0-9._-]{1,60}$`, no `/`,
/// no leading `.` or `-`, no `..`).
// trace:STORY-1917 | ai:agy
pub(crate) fn validate_lane_name(name: &str) -> Result<String> {
    let n = name.trim();
    if n.is_empty() || n.len() > 60 {
        bail!("invalid lane name `{name}`: length must be 1..=60 characters");
    }
    if n.starts_with('.') || n.starts_with('-') || n.ends_with('.') || n.contains("..") {
        bail!("invalid lane name `{n}`: cannot start with `.`/`-` or contain `..`");
    }
    if !n
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        bail!("invalid lane name `{n}`: only `[A-Za-z0-9._-]` characters are allowed (no `/`)");
    }
    Ok(n.to_string())
}

/// Validate a spec ID syntax (`^[A-Z][A-Z0-9]*-(?:[0-9]+-)?[0-9]{1,6}$`),
/// confirm it exists in the project's store, and verify it is not
/// `operator-only` or `ExecutionMode::Decide` (in the current store or sticky
/// exclusion ledger). Also checks `include_supervised` when the active mandate
/// excludes supervised specs.
// trace:STORY-1917 | ai:agy
pub(crate) fn validate_spec_in_store(
    main_worktree: &Path,
    mandate_dir: &Path,
    raw_spec: &str,
    include_supervised: bool,
) -> Result<String> {
    let spec = raw_spec.trim().to_ascii_uppercase();
    let Some((prefix, rest)) = spec.split_once('-') else {
        bail!("invalid spec ID `{raw_spec}`: expected `<PREFIX>-<NUMBER>`");
    };
    if prefix.is_empty()
        || !prefix.chars().next().unwrap().is_ascii_uppercase()
        || !prefix
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        bail!("invalid spec ID `{raw_spec}`: invalid prefix `{prefix}`");
    }
    let valid_number = match rest.split_once('-') {
        None => !rest.is_empty() && rest.len() <= 6 && rest.chars().all(|c| c.is_ascii_digit()),
        Some((node, seq)) => {
            !node.is_empty()
                && node.len() <= 4
                && node.chars().all(|c| c.is_ascii_digit())
                && !seq.is_empty()
                && seq.len() <= 6
                && seq.chars().all(|c| c.is_ascii_digit())
        }
    };
    if !valid_number {
        bail!("invalid spec ID `{raw_spec}`: expected `<PREFIX>-<NUMBER>`");
    }

    let ledger = crate::gate::mandate::read_exclusions(mandate_dir)
        .map_err(|why| anyhow::anyhow!("mandate exclusion ledger is unreadable: {why}"))?;
    if let Some(entry) = ledger.get(&spec) {
        if entry.always_human {
            bail!(
                "spec `{spec}` is recorded as `{}` in the exclusion ledger and always requires a human",
                entry.class
            );
        }
        if !include_supervised {
            bail!(
                "spec `{spec}` is recorded as `{}` in the exclusion ledger; start the mandate with `--include-supervised` to cover it",
                entry.class
            );
        }
    }

    let store = crate::gate::dispatch_deps::load_store_for_lookup(main_worktree)
        .with_context(|| format!("cannot load requirement store to validate `{spec}`"))?;
    let Some(req) = store.requirements.iter().find(|r| {
        r.spec_id
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case(&spec))
            || r.agreed_id
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(&spec))
    }) else {
        bail!("spec `{spec}` was not found in the requirement store");
    };

    // Record any exclusions seen on this requirement into the sticky ledger.
    crate::gate::mandate::note_exclusions(mandate_dir, &spec, std::iter::once(req));

    if let Some(class) = crate::gate::mandate::always_human_class(req) {
        bail!("spec `{spec}` is `{class}` and always requires a human");
    }
    for alias in [req.spec_id.as_deref(), req.agreed_id.as_deref()]
        .into_iter()
        .flatten()
    {
        let key = alias.trim().to_ascii_uppercase();
        if let Some(entry) = ledger.get(&key) {
            if entry.always_human {
                bail!(
                    "spec `{spec}` (`{key}`) is recorded as `{}` in the exclusion ledger and always requires a human",
                    entry.class
                );
            }
            if !include_supervised {
                bail!(
                    "spec `{spec}` (`{key}`) is recorded as `{}` in the exclusion ledger; start the mandate with `--include-supervised` to cover it",
                    entry.class
                );
            }
        }
    }
    if !include_supervised {
        if let Some(why) = crate::gate::mandate::excluded_class(req) {
            bail!(
                "spec `{spec}` is in a supervised class ({why}); start the mandate with `--include-supervised` to cover it"
            );
        }
    }
    Ok(spec)
}

/// Execution-time re-validation of a frozen brief file on disk (`O_NOFOLLOW |
/// O_NONBLOCK`, mode 0600, owned by current uid, `<= 64 KiB`, non-empty UTF-8,
/// SHA-256 matches `expected_sha256`).
// trace:STORY-1917 | ai:agy
pub(crate) fn verify_frozen_brief(
    main_worktree: &Path,
    request_id: &str,
    brief_relpath: &str,
    expected_sha256: &str,
) -> Result<String> {
    let expected_rel = format!("{BRIEFS_SUBDIR}/{request_id}.md");
    if brief_relpath != expected_rel {
        bail!("invalid brief_relpath `{brief_relpath}` (expected `{expected_rel}`)");
    }
    if expected_sha256.len() != 64
        || !expected_sha256
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        bail!("invalid brief_sha256 `{expected_sha256}`");
    }
    let path = brief_path(main_worktree, request_id);
    let bytes = read_bounded_regular_file(&path, MAX_BRIEF_BYTES, true)?;
    if bytes.is_empty() || bytes.contains(&0) {
        bail!(
            "frozen brief {} is empty or contains NUL bytes",
            path.display()
        );
    }
    let actual_sha = sha256_hex(&bytes);
    if actual_sha != expected_sha256 {
        bail!(
            "frozen brief {} SHA-256 mismatch (expected {expected_sha256}, got {actual_sha})",
            path.display()
        );
    }
    let text = std::str::from_utf8(&bytes)
        .with_context(|| format!("frozen brief {} is not valid UTF-8", path.display()))?;
    Ok(text.to_string())
}

/// Execution-time re-validation of an on-disk [`DispatchRequest`].
// trace:STORY-1917 | ai:agy
pub(crate) fn revalidate_request(
    main_worktree: &Path,
    mandate_dir: &Path,
    req: &DispatchRequest,
    include_supervised: bool,
) -> Result<()> {
    if req.schema_version != DISPATCH_SCHEMA_VERSION {
        bail!(
            "unsupported dispatch request schema_version {} (expected {})",
            req.schema_version,
            DISPATCH_SCHEMA_VERSION
        );
    }
    if Uuid::parse_str(&req.id).is_err() {
        bail!("invalid dispatch request id `{}`", req.id);
    }
    if req.requester_seat != crate::gate::mandate::MANDATE_SEAT {
        bail!(
            "dispatch request `{}` has requester_seat `{}`, expected `{}`",
            req.id,
            req.requester_seat,
            crate::gate::mandate::MANDATE_SEAT
        );
    }
    if req.matched_binding.is_empty() {
        bail!("dispatch request `{}` has an empty matched_binding", req.id);
    }
    match &req.action {
        DispatchVerb::Flow { pr, .. } | DispatchVerb::Review { pr, .. } => {
            validate_pr(*pr)?;
        }
        DispatchVerb::Take {
            pr,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            ..
        } => {
            validate_pr(*pr)?;
            validate_branch(main_worktree, branch)?;
            validate_spec_in_store(main_worktree, mandate_dir, spec, include_supervised)?;
            verify_frozen_brief(main_worktree, &req.id, brief_relpath, brief_sha256)?;
        }
        DispatchVerb::Impl {
            name,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            ..
        } => {
            validate_lane_name(name)?;
            validate_branch(main_worktree, branch)?;
            validate_spec_in_store(main_worktree, mandate_dir, spec, include_supervised)?;
            verify_frozen_brief(main_worktree, &req.id, brief_relpath, brief_sha256)?;
        }
        DispatchVerb::Train { prs } => {
            validate_prs(prs)?;
        }
    }
    let expected_target = req.action.target_key();
    if req.target_key != expected_target {
        bail!(
            "dispatch request `{}` target_key `{}` does not match `{expected_target}`",
            req.id,
            req.target_key
        );
    }
    let expected_dedupe = req.action.dedupe_key();
    if req.dedupe_key != expected_dedupe {
        bail!(
            "dispatch request `{}` dedupe_key `{}` does not match `{expected_dedupe}`",
            req.id,
            req.dedupe_key
        );
    }
    Ok(())
}

// ── Authority Verification at Request Write Time ────────────────────────────

/// Validated caller & mandate context for `aida dispatch request`.
#[derive(Debug, Clone)]
pub(crate) struct AuthorizedDispatchCaller {
    mandate: Mandate,
    grant_id: String,
    matched_binding: Vec<String>,
}

impl AuthorizedDispatchCaller {
    pub(crate) fn mandate(&self) -> &Mandate {
        &self.mandate
    }

    pub(crate) fn grant_id(&self) -> &str {
        &self.grant_id
    }

    /// The request record this admission authorizes. Its requester seat,
    /// grant, presenter binding and mandate come from the checks that built
    /// `self`, never from the submitting code.
    // trace:STORY-1889 | ai:claude
    pub(crate) fn admitted_request(
        &self,
        id: String,
        requested_at: DateTime<Utc>,
        principal: &str,
        dedupe_key: String,
        target_key: String,
        action: DispatchVerb,
    ) -> DispatchRequest {
        DispatchRequest {
            schema_version: DISPATCH_SCHEMA_VERSION,
            id,
            requested_at,
            principal: principal.trim().to_string(),
            requester_seat: crate::gate::mandate::MANDATE_SEAT.to_string(),
            requester_grant_id: self.grant_id.clone(),
            matched_binding: self.matched_binding.clone(),
            mandate_id: self.mandate.id.clone(),
            dedupe_key,
            target_key,
            action,
        }
    }

    /// The `Pending` result record for `req`. Its identity, mandate and
    /// requester grant prefix come from `req` and this admission.
    // trace:STORY-1889 | ai:claude
    pub(crate) fn pending_result(
        &self,
        req: &DispatchRequest,
        summary: String,
        log_path: String,
    ) -> DispatchResult {
        DispatchResult {
            id: req.id.clone(),
            verb: req.action.verb_str().to_string(),
            target: req.target_key.clone(),
            dedupe_key: req.dedupe_key.clone(),
            status: DispatchStatus::Pending,
            requested_at: req.requested_at,
            started_at: None,
            finished_at: None,
            requester_grant_prefix: self.grant_id.chars().take(8).collect(),
            mandate_id: self.mandate.id.clone(),
            worker_pid: None,
            worker_start: None,
            claim_token: None,
            spawned_session: None,
            exit_code: None,
            summary,
            refusal_reason: None,
            log_path,
            notified_at: None,
        }
    }
}

/// Check Mandate (`Scope::Dispatch` + `Scope::ShipHeld` when shipping),
/// `HumanGates` (`GateOp::Dispatch` + `GateOp::Ship`), and `advisor` seat grant
/// (bound to the calling process or its inherited session keyring).
// trace:STORY-1917 | ai:agy
pub(crate) fn check_request_authority(
    main_worktree: &Path,
    mandate_dir: &Path,
    principal: &str,
    verb: &DispatchRequestVerb,
    now: DateTime<Utc>,
) -> Result<AuthorizedDispatchCaller> {
    let (valid, _) = crate::gate::mandate::load_valid(mandate_dir, principal, now);
    let mandate = match crate::gate::mandate::state_at(&valid, now) {
        MandateState::None => {
            bail!(
                "no active mandate; start one at a human terminal with `aida mandate start --ship-held --scope dispatch` (or `--drive`)"
            )
        }
        MandateState::Expired(m) => {
            bail!(
                "mandate {} expired at {}; start a new one with `aida mandate start --ship-held --scope dispatch`",
                m.id,
                m.expires_at.to_rfc3339()
            )
        }
        MandateState::Stopped(m) => {
            bail!(
                "mandate {} was stopped; start a new one with `aida mandate start --ship-held --scope dispatch`",
                m.id
            )
        }
        MandateState::Active(m) => m,
    };

    // trace:TASK-2340 | ai:agy
    if !mandate.covers(Scope::Dispatch) {
        let msg = format!(
            "active mandate {} does not cover `dispatch`; stop it and start one with `aida mandate start --ship-held --scope dispatch` (or `--drive`)",
            mandate.id
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    let ships_pr = matches!(
        verb,
        DispatchRequestVerb::Flow { no_ship: false, .. } | DispatchRequestVerb::Train { .. }
    );
    if ships_pr && !mandate.covers(Scope::ShipHeld) {
        let msg = format!(
            "active mandate {} does not cover `ship-held`, which `{}` requires",
            mandate.id,
            match verb {
                DispatchRequestVerb::Train { .. } => "train",
                _ => "flow",
            }
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    let gates =
        crate::gate::mandate_gates::load_human_gates(main_worktree).map_err(|e| format!("{e:#}"));
    if let Some(why) = crate::gate::mandate_gates::gate_reason(&gates, GateOp::Dispatch) {
        let msg = format!("dispatch is gated by human gates ({why})");
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Gated,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }
    if ships_pr {
        if let Some(why) = crate::gate::mandate_gates::gate_reason(&gates, GateOp::Ship) {
            let msg = format!("shipping is gated by human gates ({why})");
            emit_dispatch_event(
                main_worktree,
                &mandate.id,
                verb_name_of(verb),
                &verb_subject_of(verb),
                Outcome::Gated,
                Some(msg.clone()),
            );
            bail!("{msg}");
        }
    }

    let grant = match crate::gate::seat_authority::grant_check_at(main_worktree, now) {
        GrantCheck::Valid(grant) => grant,
        GrantCheck::Absent => {
            let msg = "dispatch requests require a bound `advisor` seat grant (no session grant is set); enter the role before starting the advisor session with `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"`".to_string();
            emit_dispatch_event(
                main_worktree,
                &mandate.id,
                verb_name_of(verb),
                &verb_subject_of(verb),
                Outcome::Refused,
                Some(msg.clone()),
            );
            bail!("{msg}");
        }
        GrantCheck::Rejected { id, seat, reason } => {
            let fix = match reason {
                GrantRejection::Unbound
                | GrantRejection::PresenterNotBound { .. }
                | GrantRejection::ForeignNamespace { .. }
                | GrantRejection::KeyringUnavailable { .. } => {
                    "A `--bind-pid` grant cannot present inside a bwrap tool call because the PID namespace hides the harness PID and no session key was inherited. Start the advisor session after running `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"` in the launching terminal so the kernel session keyring (`key:<sha256>`) is inherited into the sandbox."
                }
                _ => "Re-enter the advisor seat at your terminal with `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"`.",
            };
            let seat_str = seat.as_deref().unwrap_or("unknown");
            let msg = format!("advisor seat grant {id} ({seat_str}) was rejected: {reason}. {fix}");
            emit_dispatch_event(
                main_worktree,
                &mandate.id,
                verb_name_of(verb),
                &verb_subject_of(verb),
                Outcome::Refused,
                Some(msg.clone()),
            );
            bail!("{msg}");
        }
    };

    if crate::gate::dispatch_deps::canonical_role_name(&grant.seat)
        != crate::gate::mandate::MANDATE_SEAT
    {
        let msg = format!(
            "dispatch requests require the `advisor` seat grant, but current grant {} holds seat `{}`",
            grant.id, grant.seat
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    if grant.principal.trim() != mandate.principal.trim() {
        let msg = format!(
            "advisor grant principal `{}` does not match mandate principal `{}`",
            grant.principal, mandate.principal
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    let delegates_reviewer = matches!(
        verb,
        DispatchRequestVerb::Flow { .. } | DispatchRequestVerb::Review { .. }
    );
    if delegates_reviewer
        && !grant
            .delegable_seats
            .iter()
            .any(|s| crate::gate::dispatch_deps::canonical_role_name(s) == "reviewer")
    {
        let msg = format!(
            "advisor grant {} does not delegate the `reviewer` seat required by `{}`; re-enter with `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"`",
            grant.id,
            verb_name_of(verb)
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    let presenter = crate::gate::seat_authority::presenter_tokens_for(&grant);
    let matched_binding = crate::gate::seat_authority::matched_binding(&grant, &presenter);
    if matched_binding.is_empty() {
        let msg = format!(
            "advisor grant {} has no presenter binding matching this process; start the advisor session after `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"`",
            grant.id
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    Ok(AuthorizedDispatchCaller {
        mandate,
        grant_id: grant.id,
        matched_binding,
    })
}

pub(crate) fn verb_name_of(verb: &DispatchRequestVerb) -> &'static str {
    match verb {
        DispatchRequestVerb::Flow { .. } => "flow",
        DispatchRequestVerb::Review { .. } => "review",
        DispatchRequestVerb::Take { .. } => "take",
        DispatchRequestVerb::Impl { .. } => "impl",
        DispatchRequestVerb::Train { .. } => "train",
    }
}

pub(crate) fn verb_subject_of(verb: &DispatchRequestVerb) -> String {
    match verb {
        DispatchRequestVerb::Flow { pr, .. }
        | DispatchRequestVerb::Review { pr, .. }
        | DispatchRequestVerb::Take { pr, .. } => format!("PR #{pr}"),
        DispatchRequestVerb::Impl { name, spec, .. } => {
            format!("{name} ({})", spec.trim().to_ascii_uppercase())
        }
        DispatchRequestVerb::Train { prs } => {
            let list = prs
                .iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("PRs {list}")
        }
    }
}

pub(crate) fn emit_dispatch_event(
    main_worktree: &Path,
    mandate_id: &str,
    verb: &str,
    subject: &str,
    outcome: Outcome,
    why: Option<String>,
) {
    let row = ActionRow::new(&format!("dispatch:{verb}"), subject, verb, outcome, why);
    crate::gate::dispatch_deps::emit_action(main_worktree, mandate_id, "session", &row);
}

/// Verify that `aida dispatch serve` is running with a valid, bound `advisor`
/// seat grant (`GrantCheck::Valid` with `seat == "advisor"`).
///
/// Per ADVISOR SIGNOFF: `serve` never self-binds or mutates
/// `grant.binding`; it inherits the advisor terminal's `proc:`/`sid:`/`key:`
/// binding the ordinary way. If the grant is absent, rejected, expired, or
/// revoked, `serve` refuses to start (or stops mid-run and leaves unclaimed
/// requests `Pending`).
// trace:STORY-1917 | ai:agy
pub(crate) fn check_serve_grant(
    main_worktree: &Path,
    now: DateTime<Utc>,
) -> Result<crate::gate::seat_authority::SeatGrant> {
    let grant = match crate::gate::seat_authority::grant_check_at(main_worktree, now) {
        GrantCheck::Valid(g) => g,
        GrantCheck::Absent => {
            bail!(
                "`aida dispatch serve` requires a bound `advisor` seat grant (no session grant is set); enter the role at your terminal with `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"` before starting `aida dispatch serve`"
            )
        }
        GrantCheck::Rejected { id, seat, reason } => {
            let seat_str = seat.as_deref().unwrap_or("unknown");
            bail!(
                "`aida dispatch serve` advisor seat grant {id} ({seat_str}) is not valid: {reason}; re-enter with `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"`"
            )
        }
    };
    if crate::gate::dispatch_deps::canonical_role_name(&grant.seat)
        != crate::gate::mandate::MANDATE_SEAT
    {
        bail!(
            "`aida dispatch serve` requires the `advisor` seat grant, but current grant {} holds seat `{}`",
            grant.id,
            grant.seat
        );
    }
    let presenter = crate::gate::seat_authority::presenter_tokens_for(&grant);
    if crate::gate::seat_authority::matched_binding(&grant, &presenter).is_empty() {
        bail!(
            "`aida dispatch serve` grant {} has no presenter binding matching this process; run `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"` in this terminal first",
            grant.id
        );
    }
    Ok(grant)
}

/// Tri-state worker identity probe (`Alive` / `Dead` / `Unknown`, TASK-2344 R4).
///
/// Reaping a `Running` worker requires positive `Probe::Dead` evidence
/// (`/proc/<pid>/stat` is `NotFound`, process state is exited/zombie `Z`/`X`, or
/// field-22 `starttime` ticks differ from the recorded `linux-starttime:<ticks>`).
/// Missing `worker_pid`, missing/unrecognized `worker_start`, or `/proc` read
/// errors (`EACCES`, etc.) return `Probe::Unknown` so callers retain the claim
/// fail-closed rather than guessing.
// trace:STORY-1917 trace:TASK-2344 | ai:agy
pub(crate) fn probe_worker_identity(
    worker_pid: Option<u32>,
    worker_start: Option<&str>,
) -> aida_core::process_probe::Probe {
    use aida_core::process_probe::Probe;

    #[cfg(test)]
    if let Some(forced) = TEST_FORCE_WORKER_PROBE.with(|c| c.borrow().clone()) {
        return forced;
    }

    let Some(pid) = worker_pid.filter(|&p| p > 0) else {
        return Probe::Unknown("worker_pid is missing or zero".to_string());
    };
    let Some(raw_start) = worker_start.map(str::trim).filter(|s| !s.is_empty()) else {
        return Probe::Unknown(format!("worker_start is missing for pid {pid}"));
    };
    let Some(rec_ticks_str) = raw_start.strip_prefix("linux-starttime:") else {
        return Probe::Unknown(format!(
            "worker_start `{raw_start}` for pid {pid} does not use `linux-starttime:<ticks>`"
        ));
    };
    let Ok(recorded_ticks) = rec_ticks_str.parse::<u64>() else {
        return Probe::Unknown(format!(
            "worker_start `{raw_start}` for pid {pid} has invalid start ticks"
        ));
    };

    let stat_path = format!("/proc/{pid}/stat");
    let stat_str = match std::fs::read_to_string(&stat_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Probe::Dead(format!("process {pid} no longer exists"));
        }
        Err(e) => {
            return Probe::Unknown(format!("cannot read {stat_path}: {e}"));
        }
    };
    let Some(rparen) = stat_str.rfind(')') else {
        return Probe::Unknown(format!("malformed {stat_path} (missing `)`)"));
    };
    let mut fields = stat_str[rparen + 1..].split_whitespace();
    let Some(state_tok) = fields.next() else {
        return Probe::Unknown(format!("malformed {stat_path} (missing state field)"));
    };
    let state_char = state_tok.chars().next().unwrap_or('?');
    // Field 3 was consumed by `fields.next()`; field 22 (`starttime`) is 18 fields later (`nth(18)`).
    let Some(ticks_tok) = fields.nth(22 - 4) else {
        return Probe::Unknown(format!(
            "malformed {stat_path} (missing starttime field 22)"
        ));
    };
    let Ok(live_ticks) = ticks_tok.parse::<u64>() else {
        return Probe::Unknown(format!(
            "malformed {stat_path} (non-numeric starttime `{ticks_tok}`)"
        ));
    };
    if live_ticks != recorded_ticks {
        return Probe::Dead(format!(
            "pid {pid} start ticks changed ({recorded_ticks} -> {live_ticks}); PID was recycled"
        ));
    }
    if matches!(state_char, 'Z' | 'X' | 'x') {
        return Probe::Dead(format!(
            "process {pid} is in exited/zombie state `{state_char}`"
        ));
    }
    Probe::Alive
}

/// The `Running` claims in `inv` the reaper releases, with the reason: only
/// those whose recorded `(worker_pid, worker_start)` probes as positively
/// `Probe::Dead`. `Alive` and `Unknown` claims (and every recovery fence,
/// which is not in `inv.pairs`) keep their cap slot and target, so an
/// unreadable probe never frees a slot for a duplicate or replacement run.
// trace:STORY-1889 trace:STORY-1917 trace:TASK-2344 | ai:claude
pub(crate) fn dead_claims(inv: &DispatchInventory) -> Vec<(String, String)> {
    inv.pairs
        .iter()
        .filter(|(_, res)| res.status == DispatchStatus::Running)
        .filter_map(|(req, res)| {
            match probe_worker_identity(res.worker_pid, res.worker_start.as_deref()) {
                aida_core::process_probe::Probe::Dead(dead_reason) => Some((
                    req.id.clone(),
                    format!(
                        "worker (pid {:?}) exited unexpectedly without recording completion ({dead_reason})",
                        res.worker_pid
                    ),
                )),
                aida_core::process_probe::Probe::Alive
                | aida_core::process_probe::Probe::Unknown(_) => None,
            }
        })
        .collect()
}

/// Verify under `tick.lock` that the on-disk result for `id` is still `Running`,
/// not fenced in `.aida/dispatch/recovery.json`, and owned by `claim` before
/// allowing a terminal state write (F2, TASK-2344 R4).
pub(crate) fn verify_claim_locked(
    main_worktree: &Path,
    id: &str,
    claim: &WorkerClaim,
) -> Result<()> {
    let inv = reconcile_inventory_locked(main_worktree, Utc::now())?;
    if let Some(fence) = inv.find_fence(id) {
        bail!(
            "fenced result transition refused for {id}: record is in `needs-recovery` state ({})",
            fence.reason
        );
    }
    let on_disk_bytes =
        read_bounded_regular_file(&result_path(main_worktree, id), MAX_BRIEF_BYTES, true)?;
    let on_disk: DispatchResult = serde_json::from_slice(&on_disk_bytes)
        .with_context(|| format!("malformed result JSON for {id}"))?;
    if on_disk.status != DispatchStatus::Running
        || on_disk.worker_pid != Some(claim.worker_pid)
        || on_disk.worker_start != claim.worker_start
        || on_disk.claim_token.as_deref() != Some(claim.claim_token.as_str())
    {
        bail!(
            "fenced result transition refused for {id}: on-disk claim {:?}/{:?} (status {}) does not match worker claim {}/{:?}",
            on_disk.worker_pid,
            on_disk.claim_token,
            on_disk.status.as_str(),
            claim.worker_pid,
            claim.claim_token
        );
    }
    Ok(())
}

/// Inspect `.aida/review-verdicts/PR-<pr>.json` (through the pinned, no-follow
/// verdict reader) against `head_oid`.
// trace:STORY-1917 | ai:agy
pub(crate) fn verdict_at_head(
    main_worktree: &Path,
    pr: u64,
    head_oid: &str,
) -> Option<VerdictKind> {
    let key = format!("PR-{pr}");
    let current = review_verdict::current_verdict(main_worktree, &key)?;
    let v = review_verdict::parse_recorded_verdict(&current.body)?;
    if v.is_closed() {
        return None;
    }
    let rev_sha = v.reviewed_sha.as_deref()?;
    if !review_verdict::same_reviewed_sha(rev_sha, head_oid) {
        return None;
    }
    // Positive review credit needs durable independent authority. Refusals
    // remain protective even when recorded without an approval grant.
    // trace:BUG-2398 | ai:codex
    if v.kind.approves()
        && (review_verdict::reconcile_artifacts_for_sha([current.body.as_str()], head_oid).ok()
            != Some(Some(VerdictKind::Approved))
            || !crate::gate::review_authority::independent_approval_authorized(
                main_worktree,
                &[],
                v.reviewed_branch.as_deref(),
                &v,
            ))
    {
        return None;
    }
    Some(v.kind)
}

/// Accept only [`crate::gate::dispatch_deps::CiState::Success`] as green CI.
// trace:STORY-1917 | ai:claude
pub(crate) fn ci_state_is_green(pr: u64, state: crate::gate::dispatch_deps::CiState) -> Result<()> {
    match state {
        crate::gate::dispatch_deps::CiState::Success => Ok(()),
        crate::gate::dispatch_deps::CiState::Failed => bail!("CI failed for PR #{pr}"),
        crate::gate::dispatch_deps::CiState::None => {
            bail!("PR #{pr} has no CI checks; flow reports green only after CI passes")
        }
        other => bail!("CI for PR #{pr} did not pass (state {other:?})"),
    }
}

/// What `flow` does with the review verdict at the PR head.
// trace:STORY-1889 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlowVerdict {
    /// An APPROVED verdict pinned to the head: `flow` may hand the PR to ship.
    Ship,
    /// CHANGES_REQUESTED or REJECTED: stop with the hold intact.
    ChangesRequested,
    /// No usable verdict at the head: stop.
    NoVerdict,
}

/// Only an APPROVED verdict at the current head lets `flow` ship; anything
/// else stops the flow (the ship gate in `pr ship` still decides the merge).
// trace:STORY-1889 | ai:claude
pub(crate) fn flow_ship_decision(verdict: Option<VerdictKind>) -> FlowVerdict {
    match verdict {
        Some(VerdictKind::Approved) => FlowVerdict::Ship,
        Some(VerdictKind::RequestChanges | VerdictKind::Rejected) => FlowVerdict::ChangesRequested,
        _ => FlowVerdict::NoVerdict,
    }
}

// ── Claim Admission (`dispatch tick` / `serve` / `run`) ─────────────────────

/// The `(review, flow, train)` slots in use: `Running` records plus
/// known-verb `needs-recovery` fences, counted against the concurrency caps.
// trace:STORY-1889 trace:STORY-1917 trace:TASK-2344 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClaimCaps {
    reviews: usize,
    flows: usize,
    trains: usize,
}

impl ClaimCaps {
    /// The slots `inv` already uses, `exclude_id` aside. Refuses while the
    /// inventory, or a fence with an unknown verb, blocks every claim.
    pub(crate) fn from_inventory(
        inv: &DispatchInventory,
        exclude_id: Option<&str>,
    ) -> Result<Self> {
        let (reviews, flows, trains) = inv.active_and_fenced_cap_usage(exclude_id)?;
        Ok(Self {
            reviews,
            flows,
            trains,
        })
    }

    /// Take a slot for `verb` when its cap has room; `take` and `impl` are
    /// uncapped.
    pub(crate) fn try_take(&mut self, verb: &DispatchVerb) -> bool {
        match verb {
            DispatchVerb::Review { .. } => {
                if self.reviews >= MAX_CONCURRENT_REVIEWS {
                    return false;
                }
                self.reviews += 1;
            }
            DispatchVerb::Flow { .. } => {
                if self.flows >= MAX_CONCURRENT_FLOWS {
                    return false;
                }
                self.flows += 1;
            }
            DispatchVerb::Train { .. } => {
                if self.trains >= MAX_CONCURRENT_TRAINS {
                    return false;
                }
                self.trains += 1;
            }
            DispatchVerb::Take { .. } | DispatchVerb::Impl { .. } => {}
        }
        true
    }

    /// Give back the slot a claim took when its worker never started.
    pub(crate) fn release(&mut self, verb: &DispatchVerb) {
        match verb {
            DispatchVerb::Review { .. } => {
                self.reviews = self.reviews.saturating_sub(1);
            }
            DispatchVerb::Flow { .. } => {
                self.flows = self.flows.saturating_sub(1);
            }
            DispatchVerb::Train { .. } => {
                self.trains = self.trains.saturating_sub(1);
            }
            DispatchVerb::Take { .. } | DispatchVerb::Impl { .. } => {}
        }
    }
}

/// Proof that the gate admitted a claim on one `Pending` record. Only
/// [`pass_admission`] and [`run_admission`] build one, and
/// [`DispatchResult::begin_running`] consumes it, so no claim is written
/// without the admission.
// trace:STORY-1889 | ai:claude
#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub(crate) struct ClaimAdmission {
    _sealed: (),
}

/// Whether a tick or serve pass claims one `Pending` record.
// trace:STORY-1889 trace:STORY-1917 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PassAdmission {
    /// `review`, `flow` and `train` run only under `aida dispatch serve`.
    WaitingForServe,
    /// A recovery fence reserves the target, or the verb's cap is full.
    Skip,
    /// Claimed; `caps` now counts its slot.
    Claim(ClaimAdmission),
}

/// Claim admission for one `Pending` record on a tick or serve pass.
// trace:STORY-1889 trace:STORY-1917 trace:TASK-2344 | ai:claude
pub(crate) fn pass_admission(
    inv: &DispatchInventory,
    req: &DispatchRequest,
    via_serve: bool,
    caps: &mut ClaimCaps,
) -> PassAdmission {
    if req.action.requires_serve() && !via_serve {
        return PassAdmission::WaitingForServe;
    }
    if inv
        .execution_target_fence(&req.id, &req.target_key)
        .is_some()
    {
        return PassAdmission::Skip;
    }
    if !caps.try_take(&req.action) {
        return PassAdmission::Skip;
    }
    PassAdmission::Claim(ClaimAdmission { _sealed: () })
}

/// Claim admission for `aida dispatch run <id>` on a `Pending` record: the
/// recovery fences and concurrency caps a pass applies, refused by name.
// trace:STORY-1889 trace:STORY-1917 trace:TASK-2344 | ai:claude
pub(crate) fn run_admission(
    inv_after_reap: &DispatchInventory,
    id: &str,
    raw_req_res: &Result<DispatchRequest>,
) -> Result<ClaimAdmission> {
    let (running_reviews, running_flows, running_trains) =
        inv_after_reap.active_and_fenced_cap_usage(Some(id))?;
    // Enforce concurrency caps on direct `aida dispatch run <id>` as well.
    if let Ok(req) = raw_req_res {
        if let Some(fence) = inv_after_reap.execution_target_fence(id, &req.target_key) {
            bail!(
                "cannot run dispatch request {id}: target `{}` is reserved by dispatch record {} in `needs-recovery` state ({}). {RECOVERY_GUIDANCE}",
                req.target_key,
                fence.id,
                fence.reason
            );
        }
        match req.action {
            DispatchVerb::Review { .. } if running_reviews >= MAX_CONCURRENT_REVIEWS => {
                bail!(
                    "cannot run dispatch request {id}: review concurrency cap ({MAX_CONCURRENT_REVIEWS}) reached"
                );
            }
            DispatchVerb::Flow { .. } if running_flows >= MAX_CONCURRENT_FLOWS => {
                bail!(
                    "cannot run dispatch request {id}: flow concurrency cap ({MAX_CONCURRENT_FLOWS}) reached"
                );
            }
            DispatchVerb::Train { .. } if running_trains >= MAX_CONCURRENT_TRAINS => {
                bail!(
                    "cannot run dispatch request {id}: train concurrency cap ({MAX_CONCURRENT_TRAINS}) reached"
                );
            }
            _ => {}
        }
    }
    Ok(ClaimAdmission { _sealed: () })
}

/// A `Running` record's claim, checked against this process.
// trace:STORY-1889 trace:STORY-1917 trace:TASK-2344 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunningClaim {
    /// This process owns the claim.
    Owned(WorkerClaim),
    /// The pre-spawn claim was written but its worker pid never was: the
    /// record fails closed with `reason`, then the run refuses with `refusal`.
    Unpublished { reason: String, refusal: String },
    /// Another process holds the claim. `dead` is positive `Probe::Dead`
    /// evidence, so the caller reaps before refusing.
    Foreign { dead: bool, refusal: String },
}

/// Check a `Running` record's claim token and worker identity against the
/// presented token and this process (F2, TASK-2344 R4).
// trace:STORY-1889 trace:STORY-1917 trace:TASK-2344 | ai:claude
pub(crate) fn verify_running_claim(
    id: &str,
    res: &DispatchResult,
    presented_token: Option<&str>,
    self_pid: u32,
    self_start: &Option<String>,
) -> Result<RunningClaim> {
    let Some(expected_token) = res.claim_token.clone() else {
        bail!(
            "dispatch request {id} is already running without a claim token (refusing unowned execution)"
        );
    };
    if presented_token != Some(expected_token.as_str()) {
        bail!(
            "dispatch request {id} is already running under worker pid {:?} (claim token mismatch; refusing to overwrite live worker)",
            res.worker_pid
        );
    }
    let Some(recorded_pid) = res.worker_pid else {
        // Parent crashed or failed after pre-spawn claim write before
        // publishing the child PID: fail closed so an unowned child can
        // never perform work or leave a replaceable live claim.
        let reason =
            "worker claim publication did not complete (worker_pid is None); refusing unowned execution"
                .to_string();
        return Ok(RunningClaim::Unpublished {
            refusal: format!("dispatch request {id} has an incomplete Running claim without worker_pid: {reason}"),
            reason,
        });
    };
    let pid_matches = recorded_pid == self_pid && res.worker_start == *self_start;
    if !pid_matches {
        let dead = matches!(
            probe_worker_identity(Some(recorded_pid), res.worker_start.as_deref()),
            aida_core::process_probe::Probe::Dead(_)
        );
        return Ok(RunningClaim::Foreign {
            dead,
            refusal: format!(
                "dispatch request {id} is already running under worker pid {recorded_pid} (start {:?}), not this process {self_pid} (start {:?}); refusing duplicate or replacement execution",
                res.worker_start,
                self_start
            ),
        });
    }
    Ok(RunningClaim::Owned(WorkerClaim {
        worker_pid: self_pid,
        worker_start: self_start.clone(),
        claim_token: expected_token,
    }))
}

// ── Execution-Time Admission ────────────────────────────────────────────────

/// Why a claimed request is refused at execution time: the reason recorded
/// on its result and the action-log outcome (`Gated` for a human gate).
// trace:STORY-1889 trace:STORY-1917 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecutionRefusal {
    pub(crate) reason: String,
    pub(crate) outcome: Outcome,
}

/// Proof that [`admit_execution`] admitted a claimed request. Only the gate
/// builds one, and every verb runner takes it, so a runner cannot run
/// without the execution-time admission.
// trace:STORY-1889 | ai:claude
#[derive(Debug)]
#[must_use]
pub(crate) struct ExecutionAdmission {
    _sealed: (),
}

/// Execution-time admission of a claimed request, first refusal wins:
/// Boundary B, the serve grant, the exact recorded mandate and its scopes,
/// human gates, the requester grant and its `reviewer` delegation, then the
/// dual-end re-validation of the request and its frozen brief.
// trace:STORY-1889 trace:STORY-1917 | ai:claude
pub(crate) fn admit_execution(
    main_worktree: &Path,
    mandate_dir: &Path,
    req: &DispatchRequest,
    via_serve: bool,
    now: DateTime<Utc>,
) -> std::result::Result<ExecutionAdmission, ExecutionRefusal> {
    match execution_refusal(main_worktree, mandate_dir, req, via_serve, now) {
        Some(refusal) => Err(refusal),
        None => Ok(ExecutionAdmission { _sealed: () }),
    }
}

/// The first execution-time refusal for `req`, if any (see
/// [`admit_execution`]).
fn execution_refusal(
    main_worktree: &Path,
    mandate_dir: &Path,
    req: &DispatchRequest,
    via_serve: bool,
    now: DateTime<Utc>,
) -> Option<ExecutionRefusal> {
    // 1. Boundary B check: `review`, `flow`, and `train` may ONLY execute under
    // `aida dispatch serve` (never under `dispatch tick`).
    if req.action.requires_serve() && !via_serve {
        let reason = format!(
            "verb `{}` requires an advisor seat grant and can only run under `aida dispatch serve` (refused by `dispatch tick`)",
            req.action.verb_str()
        );
        return Some(ExecutionRefusal {
            reason,
            outcome: Outcome::Refused,
        });
    }

    if via_serve {
        if let Err(err) = check_serve_grant(main_worktree, now) {
            let reason = format!("serve advisor grant check failed at execution time: {err:#}");
            return Some(ExecutionRefusal {
                reason,
                outcome: Outcome::Refused,
            });
        }
    }

    // 2. Re-validate the exact recorded mandate ID on disk (`mandate::recheck`, F3).
    //    A replacement mandate B must never revive requests stopped or expired under A.
    let active_mandate =
        match crate::gate::mandate::recheck(mandate_dir, &req.mandate_id, &req.principal, now) {
            Ok(m) => m,
            Err(crate::gate::mandate::MandateRefusal::Expired { id: mid }) => {
                let reason = format!("mandate {mid} expired before execution");
                return Some(ExecutionRefusal {
                    reason,
                    outcome: Outcome::Refused,
                });
            }
            Err(crate::gate::mandate::MandateRefusal::Stopped { id: mid }) => {
                let reason = format!("mandate {mid} was stopped or superseded before execution");
                return Some(ExecutionRefusal {
                    reason,
                    outcome: Outcome::Refused,
                });
            }
            Err(other) => {
                let reason = format!(
                    "mandate {} recheck refused before execution: {other:?}",
                    req.mandate_id
                );
                return Some(ExecutionRefusal {
                    reason,
                    outcome: Outcome::Refused,
                });
            }
        };

    if !active_mandate.covers(Scope::Dispatch) {
        let reason = format!(
            "active mandate {} no longer covers `dispatch`",
            active_mandate.id
        );
        return Some(ExecutionRefusal {
            reason,
            outcome: Outcome::Refused,
        });
    }
    if req.action.ships_pr() && !active_mandate.covers(Scope::ShipHeld) {
        let reason = format!(
            "active mandate {} does not cover `ship-held`, which `{}` requires",
            active_mandate.id,
            req.action.verb_str()
        );
        return Some(ExecutionRefusal {
            reason,
            outcome: Outcome::Refused,
        });
    }

    // 3. Re-validate HumanGates at execution time.
    let gates =
        crate::gate::mandate_gates::load_human_gates(main_worktree).map_err(|e| format!("{e:#}"));
    if let Some(why) = crate::gate::mandate_gates::gate_reason(&gates, GateOp::Dispatch) {
        let reason = format!("dispatch is gated by human gates at execution time ({why})");
        return Some(ExecutionRefusal {
            reason,
            outcome: Outcome::Gated,
        });
    }
    if req.action.ships_pr() {
        if let Some(why) = crate::gate::mandate_gates::gate_reason(&gates, GateOp::Ship) {
            let reason = format!("shipping is gated by human gates at execution time ({why})");
            return Some(ExecutionRefusal {
                reason,
                outcome: Outcome::Gated,
            });
        }
    }

    // 4. Re-validate the requester's advisor grant on disk (`grant_authorized_at`),
    //    and treat any `revoked_at` stamp as immediate revocation regardless of
    //    clock skew.
    let grant_unrevoked = crate::gate::seat_authority::read_grant(&req.requester_grant_id)
        .map(|g| g.revoked_at.is_none())
        .unwrap_or(false);
    if !grant_unrevoked
        || !crate::gate::seat_authority::grant_authorized_at(
            main_worktree,
            &req.requester_grant_id,
            crate::gate::mandate::MANDATE_SEAT,
            now,
            &req.matched_binding,
        )
    {
        let reason = format!(
            "requester advisor grant {} is no longer valid (expired, revoked, or binding mismatch)",
            req.requester_grant_id
        );
        return Some(ExecutionRefusal {
            reason,
            outcome: Outcome::Refused,
        });
    }
    if req.action.delegates_reviewer() {
        let delegable_ok = crate::gate::seat_authority::read_grant(&req.requester_grant_id)
            .map(|g| {
                g.delegable_seats
                    .iter()
                    .any(|s| crate::gate::dispatch_deps::canonical_role_name(s) == "reviewer")
            })
            .unwrap_or(false);
        if !delegable_ok {
            let reason = format!(
                "requester advisor grant {} does not delegate `reviewer`",
                req.requester_grant_id
            );
            return Some(ExecutionRefusal {
                reason,
                outcome: Outcome::Refused,
            });
        }
    }

    // 5. Dual-end validation of request fields + frozen brief SHA-256 + spec store checks.
    if let Err(err) = revalidate_request(
        main_worktree,
        mandate_dir,
        req,
        active_mandate.include_supervised,
    ) {
        let reason = format!("execution-time validation refused request: {err:#}");
        return Some(ExecutionRefusal {
            reason,
            outcome: Outcome::Refused,
        });
    }
    None
}

#[cfg(test)]
#[path = "tests/story_1889_dispatch_authority_tests.rs"]
mod story_1889_dispatch_authority_tests;

// The STORY-1917 behavioural suites for the dispatch gate (request admission,
// execution-time refusals, claim fencing, reaping and caps) stay on the floor
// with the code they test.
#[cfg(test)]
#[path = "tests/story_1917_dispatch_request_tests.rs"]
mod story_1917_dispatch_request_tests;

#[cfg(test)]
#[path = "tests/story_1917_dispatch_exec_tests.rs"]
mod story_1917_dispatch_exec_tests;
