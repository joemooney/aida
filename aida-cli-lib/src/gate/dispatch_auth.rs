//! In-process dispatch authority context (`DispatchExecutionContext`, STORY-1917,
//! TASK-2344 Q2).
//!
//! When `aida dispatch serve` runs a `flow` or `train` request in-process,
//! long-running CI waits, hold-gate waits, and multi-member train waves can
//! outlive the mandate or grants that were active when the worker started.
//! [`DispatchExecutionContext`] binds the worker's exact on-disk request
//! (`requests/<id>.json` + SHA-256 digest), active `Running` claim
//! (`(worker_pid, worker_start, claim_token)`), exact `mandate_id`,
//! `requester_grant_id`, and `matched_binding`, and re-verifies all of them
//! from disk at every post-wait and pre-side-effect checkpoint inside `flow`,
//! `pr ship`, and `pr train`.
// mandate-gate
// trace:STORY-1917 trace:TASK-2344 | ai:agy

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::dispatch::{self, MAX_BRIEF_BYTES};
use crate::gate::dispatch_authority::{
    check_serve_grant, verify_claim_locked, DispatchRequest, DispatchResult, DispatchStatus,
    WorkerClaim,
};
use crate::gate::dispatch_lock::acquire_dispatch_lock_for_gate;
use crate::gate::mandate::{self, Scope};
use crate::gate::mandate_gates::GateOp;
use crate::gate::seat_authority;

/// Typed error marker returned when [`DispatchExecutionContext::reverify`]
/// aborts an in-flight dispatch operation due to revoked/expired authority,
/// human-gate changes, or invalidated claim ownership.
// trace:STORY-1917 trace:TASK-2344 | ai:agy
#[derive(Debug)]
pub(crate) struct DispatchAuthorityAbortedError(pub String);

impl std::fmt::Display for DispatchAuthorityAbortedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DispatchAuthorityAbortedError {}

pub(crate) fn is_dispatch_authority_aborted(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.is::<DispatchAuthorityAbortedError>())
}

/// Bound in-process dispatch execution authority token.
///
/// Fields are private so callers outside `gate::dispatch_auth` can only
/// construct a context through [`DispatchExecutionContext::establish`], which
/// validates the canonical on-disk `requests/<id>.json` and `results/<id>.json`
/// under `tick.lock`.
// trace:STORY-1917 trace:TASK-2344 | ai:agy
#[derive(Debug, Clone)]
pub(crate) struct DispatchExecutionContext {
    main_worktree: PathBuf,
    mandate_dir: PathBuf,
    request_id: String,
    request_sha256: String,
    mandate_id: String,
    principal: String,
    requester_grant_id: String,
    matched_binding: Vec<String>,
    delegates_reviewer: bool,
    ships_pr: bool,
    claim: WorkerClaim,
}

impl DispatchExecutionContext {
    /// Establish a bound [`DispatchExecutionContext`] for the current worker
    /// process from canonical on-disk `requests/<id>.json` and `results/<id>.json`,
    /// and immediately run a full [`Self::reverify`] pass at `now`.
    // trace:STORY-1917 trace:TASK-2344 | ai:agy
    pub(crate) fn establish(
        main_worktree: &Path,
        mandate_dir: &Path,
        request_id: &str,
        claim_token: &str,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        if Uuid::parse_str(request_id).is_err() {
            bail!("invalid dispatch request id `{request_id}`");
        }
        if claim_token.trim().is_empty() {
            bail!("cannot establish DispatchExecutionContext for {request_id}: empty claim_token");
        }

        let self_pid = std::process::id();
        let self_start = crate::process_probe::process_start_identity(self_pid);
        let claim = WorkerClaim {
            worker_pid: self_pid,
            worker_start: self_start,
            claim_token: claim_token.to_string(),
        };

        let (req, req_sha256) = {
            let _lock = acquire_dispatch_lock_for_gate(main_worktree)?;
            verify_claim_locked(main_worktree, request_id, &claim)?;
            let req_bytes = dispatch::read_bounded_regular_file(
                &dispatch::request_path(main_worktree, request_id),
                MAX_BRIEF_BYTES,
                true,
            )
            .with_context(|| format!("cannot read request file for {request_id}"))?;
            let mut hasher = Sha256::new();
            hasher.update(&req_bytes);
            let digest = format!("{:x}", hasher.finalize());
            let req: DispatchRequest = serde_json::from_slice(&req_bytes)
                .with_context(|| format!("malformed request JSON for {request_id}"))?;
            if req.id != request_id {
                bail!(
                    "tampered request JSON for {request_id}: inner id `{}` does not match",
                    req.id
                );
            }
            (req, digest)
        };

        let ctx = Self {
            main_worktree: main_worktree.to_path_buf(),
            mandate_dir: mandate_dir.to_path_buf(),
            request_id: req.id.clone(),
            request_sha256: req_sha256,
            mandate_id: req.mandate_id.clone(),
            principal: req.principal.clone(),
            requester_grant_id: req.requester_grant_id.clone(),
            matched_binding: req.matched_binding.clone(),
            delegates_reviewer: req.action.delegates_reviewer(),
            ships_pr: req.action.ships_pr(),
            claim,
        };
        ctx.reverify(now, ctx.ships_pr)?;
        Ok(ctx)
    }

    pub(crate) fn request_id(&self) -> &str {
        &self.request_id
    }

    pub(crate) fn mandate_id(&self) -> &str {
        &self.mandate_id
    }

    pub(crate) fn principal(&self) -> &str {
        &self.principal
    }

    pub(crate) fn claim(&self) -> &WorkerClaim {
        &self.claim
    }

    /// Re-verify live dispatch authority at `Utc::now()`.
    pub(crate) fn reverify_now(&self, requires_ship_held: bool) -> Result<()> {
        self.reverify(Utc::now(), requires_ship_held)
    }

    /// Re-verify live dispatch authority from disk at `now`:
    /// 1. Current process still matches `self.claim` and owns the active `Running`
    ///    `results/<id>.json` record (with no `needs-recovery` fence in
    ///    `.aida/dispatch/recovery.json`).
    /// 2. `requests/<id>.json` is untampered (SHA-256 matches `self.request_sha256`).
    /// 3. Serving advisor grant passes [`check_serve_grant`].
    /// 4. Exact recorded `mandate_id` passes [`mandate::recheck`] (a replacement
    ///    mandate B never satisfies request A) and covers [`Scope::Dispatch`]
    ///    (plus [`Scope::ShipHeld`] when `requires_ship_held || self.ships_pr`).
    /// 5. [`HumanGates`] allows [`GateOp::Dispatch`] (and [`GateOp::Ship`] when
    ///    `requires_ship_held || self.ships_pr`).
    /// 6. Requester advisor grant `self.requester_grant_id` has `revoked_at.is_none()`,
    ///    passes [`seat_authority::grant_authorized_at`] for `self.matched_binding`,
    ///    and still delegates `"reviewer"` when `self.delegates_reviewer` is true.
    // trace:STORY-1917 trace:TASK-2344 | ai:agy
    pub(crate) fn reverify(&self, now: DateTime<Utc>, requires_ship_held: bool) -> Result<()> {
        self.reverify_inner(now, requires_ship_held)
            .map_err(|e| anyhow::Error::new(DispatchAuthorityAbortedError(format!("{e:#}"))))
    }

    fn reverify_inner(&self, now: DateTime<Utc>, requires_ship_held: bool) -> Result<()> {
        let self_pid = std::process::id();
        if self.claim.worker_pid != self_pid {
            bail!(
                "dispatch execution context for {} belongs to pid {}, not current pid {self_pid}",
                self.request_id,
                self.claim.worker_pid
            );
        }

        // 1 & 2. Re-verify `recovery.json`, `results/<id>.json` ownership, and
        // untampered `requests/<id>.json` under `tick.lock`.
        {
            let _lock = acquire_dispatch_lock_for_gate(&self.main_worktree)?;
            verify_claim_locked(&self.main_worktree, &self.request_id, &self.claim)?;
            let req_bytes = dispatch::read_bounded_regular_file(
                &dispatch::request_path(&self.main_worktree, &self.request_id),
                MAX_BRIEF_BYTES,
                true,
            )
            .with_context(|| {
                format!(
                    "dispatch request {} is missing or unreadable during authority recheck",
                    self.request_id
                )
            })?;
            let mut hasher = Sha256::new();
            hasher.update(&req_bytes);
            let digest = format!("{:x}", hasher.finalize());
            if digest != self.request_sha256 {
                bail!(
                    "dispatch request {} was modified on disk during execution (sha256 mismatch)",
                    self.request_id
                );
            }
            let res_bytes = dispatch::read_bounded_regular_file(
                &dispatch::result_path(&self.main_worktree, &self.request_id),
                MAX_BRIEF_BYTES,
                true,
            )?;
            let res: DispatchResult = serde_json::from_slice(&res_bytes)?;
            if res.status != DispatchStatus::Running {
                bail!(
                    "dispatch request {} is no longer Running (status is `{}`)",
                    self.request_id,
                    res.status.as_str()
                );
            }
        }

        // 3. Re-verify serving advisor seat grant.
        check_serve_grant(&self.main_worktree, now).with_context(|| {
            format!(
                "dispatch request {} aborted: serve advisor grant is no longer valid",
                self.request_id
            )
        })?;

        // 4. Re-verify exact `mandate_id` on disk (`mandate::recheck`).
        let active_mandate =
            mandate::recheck(&self.mandate_dir, &self.mandate_id, &self.principal, now).map_err(
                |refusal| {
                    anyhow::anyhow!(
                        "dispatch request {} aborted: mandate {} is no longer active ({refusal:?})",
                        self.request_id,
                        self.mandate_id
                    )
                },
            )?;
        if !active_mandate.covers(Scope::Dispatch) {
            bail!(
                "dispatch request {} aborted: mandate {} no longer covers `dispatch`",
                self.request_id,
                active_mandate.id
            );
        }
        let need_ship_scope = requires_ship_held || self.ships_pr;
        if need_ship_scope && !active_mandate.covers(Scope::ShipHeld) {
            bail!(
                "dispatch request {} aborted: mandate {} does not cover `ship-held`",
                self.request_id,
                active_mandate.id
            );
        }

        // 5. Re-verify HumanGates from committed + operator config layers.
        let gates = crate::gate::mandate_gates::load_human_gates(&self.main_worktree)
            .map_err(|e| format!("{e:#}"));
        if let Some(why) = crate::gate::mandate_gates::gate_reason(&gates, GateOp::Dispatch) {
            bail!(
                "dispatch request {} aborted: dispatch is gated by human gates ({why})",
                self.request_id
            );
        }
        if need_ship_scope {
            if let Some(why) = crate::gate::mandate_gates::gate_reason(&gates, GateOp::Ship) {
                bail!(
                    "dispatch request {} aborted: shipping is gated by human gates ({why})",
                    self.request_id
                );
            }
        }

        // 6. Re-verify requester's advisor grant on disk (`revoked_at.is_none()`
        //    + `grant_authorized_at` + reviewer delegation).
        let grant = seat_authority::read_grant(&self.requester_grant_id).map_err(|e| {
            anyhow::anyhow!(
                "dispatch request {} aborted: requester grant {} is unreadable ({e:#})",
                self.request_id,
                self.requester_grant_id
            )
        })?;
        if grant.revoked_at.is_some()
            || !seat_authority::grant_authorized_at(
                &self.main_worktree,
                &self.requester_grant_id,
                mandate::MANDATE_SEAT,
                now,
                &self.matched_binding,
            )
        {
            bail!(
                "dispatch request {} aborted: requester advisor grant {} is no longer valid (expired, revoked, or binding mismatch)",
                self.request_id,
                self.requester_grant_id
            );
        }
        if self.delegates_reviewer
            && !grant
                .delegable_seats
                .iter()
                .any(|s| crate::canonical_role_name(s) == "reviewer")
        {
            bail!(
                "dispatch request {} aborted: requester advisor grant {} no longer delegates `reviewer`",
                self.request_id,
                self.requester_grant_id
            );
        }

        Ok(())
    }
}
