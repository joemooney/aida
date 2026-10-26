//! Typed dispatch requests (`aida dispatch`).
//!
//! A sandboxed advisor seat queues closed, validated requests (`flow`,
//! `review`, `take`, `impl`, `train`) under `<main_worktree>/.aida/dispatch/`.
//! Request write time enforces an active Mandate covering [`Scope::Dispatch`]
//! (plus [`Scope::ShipHeld`] for shipping `flow` and `train`), `HumanGates`,
//! the `operator-only` / `ExecutionMode::Decide` always-human floor, and a
//! process/session-key-bound `advisor` seat grant.
//!
//! Per the signed SKETCH v1 and ADVISOR SIGNOFF:
//! - No self-binding grant guard: no code path ever appends `proc:` tokens to
//!   an advisor grant outside `aida role enter` / `--bind-pid`.
//! - The cron/schedule `dispatch tick` executes only `take` and `impl`;
//!   `review`, `flow`, and `train` require `aida dispatch serve` started by the
//!   human from an advisor terminal after `eval "$(aida role enter advisor ...)"`,
//!   and `aida dispatch list` surfaces them as `waiting for serve` while
//!   `Pending`.
//!
//! Those decisions live on the mandate's protected floor (STORY-1889):
//! [`crate::gate::dispatch_authority`] (request records, admission, claims,
//! caps), [`crate::gate::dispatch_lock`] (`tick.lock`) and
//! [`crate::gate::dispatch_ledger`] (recovery fences) and
//! [`crate::gate::dispatch_io`] (record paths, bounded no-follow reads, brief
//! hashing, pinned directories). This module is the orchestration that calls
//! them.
// trace:STORY-1917 | ai:agy
// trace:STORY-1889 | ai:claude

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[path = "dispatch_args.rs"]
pub(crate) mod dispatch_args;
pub(crate) use dispatch_args::*;

#[path = "dispatch_flow.rs"]
pub(crate) mod dispatch_flow;

use crate::gate::dispatch_authority::{
    admit_execution, check_request_authority, check_serve_grant, dead_claims, emit_dispatch_event,
    pass_admission, probe_worker_identity, resolve_verified_local_default_branch,
    revalidate_request, run_admission, submission_collision, validate_branch, validate_lane_name,
    validate_pr, validate_prs, validate_spec_in_store, verb_name_of, verb_subject_of,
    verify_claim_locked, verify_frozen_brief, verify_running_claim, ClaimCaps, DispatchRequest,
    DispatchResult, DispatchStatus, DispatchVerb, ExecutionAdmission, ExecutionEntry,
    PassAdmission, RunningClaim, SubmissionCollision, WorkerClaim,
};
use crate::gate::dispatch_ledger::{
    inspect_dispatch_inventory_readonly, read_json_half, reconcile_inventory_locked, HalfRead,
    RecoveryFenceEntry, RECOVERY_GUIDANCE,
};
use crate::gate::dispatch_lock::{acquire_dispatch_lock, activate_lock_as_human, TICK_LOCK_FILE};
use crate::gate::dispatch_reviewer_hold::remove_reviewer_worktree_preserving_hold;
use crate::gate::mandate;
use crate::gate::mandate::{MandateState, Scope};
use crate::gate::seat_authority;
use crate::mandate_drive::Outcome;

// Paths and private file I/O live on the protected floor with the gate code
// that trusts them (STORY-1889).
use crate::gate::dispatch_io::{
    brief_path, briefs_dir, dispatch_root, logs_dir, open_pinned_dispatch_subdir,
    read_bounded_regular_file, recovery_path, request_path, requests_dir, result_path, results_dir,
    sha256_hex, write_private_atomic, BRIEFS_SUBDIR, LOGS_SUBDIR, MAX_BRIEF_BYTES,
};

/// Open `.aida/dispatch/logs/<id>.log` relative to the pinned `logs` directory
/// descriptor using `O_NOFOLLOW | O_NONBLOCK`, verifying it is a singly-linked
/// (`nlink == 1`) regular file owned by the current uid with mode `0600` and
/// size `<= MAX_DISPATCH_LOG_BYTES`. Never follows symlinks and never replaces
/// a symlink or hardlink.
// trace:STORY-1917 | ai:agy
fn open_pinned_log_file(
    main_worktree: &Path,
    id: &str,
    append_mode: bool,
) -> Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;
    if Uuid::parse_str(id).is_err() {
        bail!("invalid dispatch request id `{id}` for log file");
    }
    let logs_fd = open_pinned_dispatch_subdir(main_worktree, Some(LOGS_SUBDIR))?;
    let file_name = format!("{id}.log");
    let display_path = logs_dir(main_worktree).join(&file_name);
    let c_name = std::ffi::CString::new(file_name.as_bytes())?;
    let mut flags =
        libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
    if append_mode {
        flags |= libc::O_APPEND;
    }
    let fd = unsafe { libc::openat(logs_fd.as_raw_fd(), c_name.as_ptr(), flags, 0o600) };
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
    let mode = meta.mode() & 0o7777;
    if mode != 0o600 {
        bail!(
            "{} has mode {:04o}, expected 0600 (refused)",
            display_path.display(),
            mode
        );
    }
    if meta.len() > MAX_DISPATCH_LOG_BYTES as u64 {
        bail!(
            "{} exceeds maximum log size ({} bytes > {} bytes)",
            display_path.display(),
            meta.len(),
            MAX_DISPATCH_LOG_BYTES
        );
    }
    // Clear O_NONBLOCK now that we have verified the descriptor is a regular file.
    unsafe {
        let cur = libc::fcntl(file.as_raw_fd(), libc::F_GETFL);
        if cur >= 0 {
            let _ = libc::fcntl(file.as_raw_fd(), libc::F_SETFL, cur & !libc::O_NONBLOCK);
        }
    }
    Ok(file)
}

/// Read, validate, and freeze `--brief <path>` into `.aida/dispatch/briefs/<id>.md`
/// (mode 0600). Returns `(brief_relpath, brief_sha256)`.
// trace:STORY-1917 | ai:agy
pub(crate) fn freeze_brief(
    main_worktree: &Path,
    request_id: &str,
    source_path: &Path,
) -> Result<(String, String)> {
    let bytes = read_bounded_regular_file(source_path, MAX_BRIEF_BYTES, false)?;
    if bytes.is_empty() {
        bail!("brief file {} is empty", source_path.display());
    }
    if bytes.contains(&0) {
        bail!("brief file {} contains NUL bytes", source_path.display());
    }
    let text = std::str::from_utf8(&bytes)
        .with_context(|| format!("brief file {} is not valid UTF-8", source_path.display()))?;
    if text.trim().is_empty() {
        bail!("brief file {} is blank", source_path.display());
    }
    let digest = sha256_hex(&bytes);
    let relpath = format!("{BRIEFS_SUBDIR}/{request_id}.md");
    let dest = brief_path(main_worktree, request_id);
    write_private_atomic(&dest, &bytes)?;
    Ok((relpath, digest))
}

/// Render the `take` or `impl` prompt wrapper from the embedded template in
/// `aida-core/templates/dispatch/`.
// trace:STORY-1917 | ai:agy
pub(crate) fn render_lane_prompt(
    main_worktree: &Path,
    worktree: &Path,
    action: &DispatchVerb,
    brief_body: &str,
    now: DateTime<Utc>,
) -> Result<String> {
    let ts = now.format("%Y-%m-%d %H:%M").to_string();
    match action {
        DispatchVerb::Take {
            pr,
            branch,
            spec,
            vendor,
            ..
        } => {
            let tpl = aida_core::templates::TemplateLoader::with_project_root(main_worktree)
                .load("dispatch/take-prompt.md")
                .context("missing embedded template dispatch/take-prompt.md")?;
            let vendor_str = vendor.unwrap_or(DispatchVendor::Claude).as_str();
            Ok(tpl
                .replace("{{WORKTREE}}", &worktree.display().to_string())
                .replace("{{MAIN_WORKTREE}}", &main_worktree.display().to_string())
                .replace("{{BRANCH}}", branch)
                .replace("{{PR}}", &pr.to_string())
                .replace("{{SPEC}}", spec)
                .replace("{{VENDOR}}", vendor_str)
                .replace("{{TIMESTAMP}}", &ts)
                .replace("{{BRIEF}}", brief_body.trim()))
        }
        DispatchVerb::Impl {
            branch,
            spec,
            vendor,
            ..
        } => {
            let tpl = aida_core::templates::TemplateLoader::with_project_root(main_worktree)
                .load("dispatch/impl-prompt.md")
                .context("missing embedded template dispatch/impl-prompt.md")?;
            let vendor_str = vendor.unwrap_or(DispatchVendor::Claude).as_str();
            Ok(tpl
                .replace("{{WORKTREE}}", &worktree.display().to_string())
                .replace("{{MAIN_WORKTREE}}", &main_worktree.display().to_string())
                .replace("{{BRANCH}}", branch)
                .replace("{{SPEC}}", spec)
                .replace("{{VENDOR}}", vendor_str)
                .replace("{{TIMESTAMP}}", &ts)
                .replace("{{BRIEF}}", brief_body.trim()))
        }
        other => bail!(
            "verb `{}` does not use a lane prompt template",
            other.verb_str()
        ),
    }
}

// ── Store Operations (Submit / List / Show / Cancel) ────────────────────────

/// True when an active Mandate record on disk covers [`Scope::Dispatch`]
/// (used by the `mandate.dispatch_active` schedule predicate, R5).
// trace:STORY-1917 | ai:agy
pub(crate) fn dispatch_record_active(now: DateTime<Utc>) -> bool {
    let Ok(dir) = mandate::mandate_dir() else {
        return false;
    };
    let principal = crate::current_user_id(None);
    let (valid, _) = mandate::load_valid(&dir, &principal, now);
    matches!(
        mandate::state_at(&valid, now),
        MandateState::Active(m) if m.covers(Scope::Dispatch)
    )
}

/// The `dispatch.pending` schedule predicate (R5): every validated `Pending`
/// request, plus one per `needs-recovery` record and one for a blocked or
/// unreadable inventory. Unknown state is never a false zero: it keeps the
/// tick firing so the tick's own refusal reaches the schedule log. The count
/// grants nothing; the tick re-checks authority itself.
// trace:STORY-1917 | ai:agy
// trace:STORY-1917 trace:TASK-2344 | ai:claude
pub(crate) fn pending_request_count(main_worktree: &Path) -> i64 {
    let inv = match inspect_dispatch_inventory_readonly(main_worktree) {
        Ok(inv) => inv,
        Err(e) => {
            eprintln!(
                "warning: dispatch.pending: the dispatch inventory is unreadable ({e:#}); counting it as pending so the dispatch tick reports why"
            );
            return 1;
        }
    };
    let pending = inv
        .pairs
        .iter()
        .filter(|(_, res)| res.status() == DispatchStatus::Pending)
        .count();
    (pending + inv.fences.len() + usize::from(inv.global_block_reason.is_some())) as i64
}

/// Clean up only the exact unpublished files created by the current
/// `submit_request` invocation while still holding `tick.lock` BEFORE
/// `requests/<id>.json` is published (TASK-2344, R4).
fn cleanup_unpublished_submission_locked(
    main_worktree: &Path,
    request_id: &str,
    wrote_brief: bool,
    wrote_result: bool,
) {
    if request_path(main_worktree, request_id).exists() {
        return;
    }
    if wrote_result {
        let res_p = result_path(main_worktree, request_id);
        if let HalfRead::Valid(res) = read_json_half::<DispatchResult>(&res_p, "result") {
            if res.id() == request_id
                && res.status() == DispatchStatus::Pending
                && res.is_unclaimed()
            {
                let _ = std::fs::remove_file(&res_p);
            }
        }
    }
    if wrote_brief {
        let _ = std::fs::remove_file(brief_path(main_worktree, request_id));
    }
}

/// Outcome of `submit_request`: either a newly created request or an existing
/// active (`Pending`/`Running`) request with the same `dedupe_key`.
#[derive(Debug, Clone)]
pub(crate) struct SubmittedDispatch {
    pub request: DispatchRequest,
    pub result: DispatchResult,
    pub deduplicated: bool,
}

/// Validate and persist a dispatch request under `.aida/dispatch/` using
/// result-first publication (`briefs/<id>.md` -> `results/<id>.json` ->
/// `requests/<id>.json`) under `tick.lock` (TASK-2344, R4).
// trace:STORY-1917 trace:TASK-2344 | ai:agy
pub(crate) fn submit_request(
    main_worktree: &Path,
    mandate_dir: &Path,
    principal: &str,
    verb: &DispatchRequestVerb,
    now: DateTime<Utc>,
) -> Result<SubmittedDispatch> {
    let caller = check_request_authority(main_worktree, mandate_dir, principal, verb, now)?;
    let _lock = acquire_dispatch_lock(main_worktree)?;
    let inv = reconcile_inventory_locked(main_worktree, now)?;

    let request_id = Uuid::new_v4().to_string();
    let mut wrote_brief = false;
    let action_res: Result<DispatchVerb> = (|| match verb {
        DispatchRequestVerb::Flow {
            pr,
            no_ship,
            vendor,
        } => {
            let pr = validate_pr(*pr)?;
            Ok(DispatchVerb::Flow {
                pr,
                no_ship: *no_ship,
                vendor: *vendor,
            })
        }
        DispatchRequestVerb::Review { pr, vendor } => {
            let pr = validate_pr(*pr)?;
            Ok(DispatchVerb::Review {
                pr,
                vendor: *vendor,
            })
        }
        DispatchRequestVerb::Take {
            pr,
            branch,
            spec,
            brief,
            vendor,
        } => {
            let pr = validate_pr(*pr)?;
            let branch = validate_branch(main_worktree, branch)?;
            let spec = validate_spec_in_store(
                main_worktree,
                mandate_dir,
                spec,
                caller.mandate().include_supervised,
            )?;
            let (brief_relpath, brief_sha256) = freeze_brief(main_worktree, &request_id, brief)?;
            wrote_brief = true;
            Ok(DispatchVerb::Take {
                pr,
                branch,
                spec,
                brief_relpath,
                brief_sha256,
                vendor: *vendor,
            })
        }
        DispatchRequestVerb::Impl {
            name,
            branch,
            spec,
            brief,
            vendor,
        } => {
            let name = validate_lane_name(name)?;
            let branch = validate_branch(main_worktree, branch)?;
            let spec = validate_spec_in_store(
                main_worktree,
                mandate_dir,
                spec,
                caller.mandate().include_supervised,
            )?;
            let (brief_relpath, brief_sha256) = freeze_brief(main_worktree, &request_id, brief)?;
            wrote_brief = true;
            Ok(DispatchVerb::Impl {
                name,
                branch,
                spec,
                brief_relpath,
                brief_sha256,
                vendor: *vendor,
            })
        }
        DispatchRequestVerb::Train { prs } => {
            let prs = validate_prs(prs)?;
            Ok(DispatchVerb::Train { prs })
        }
    })();

    let action = match action_res {
        Ok(action) => action,
        Err(err) => {
            cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, false);
            emit_dispatch_event(
                main_worktree,
                &caller.mandate().id,
                verb_name_of(verb),
                &verb_subject_of(verb),
                Outcome::Refused,
                Some(format!("{err:#}")),
            );
            return Err(err);
        }
    };

    let dedupe_key = action.dedupe_key();
    let target_key = action.target_key();

    // Check RecoveryRequired fences BEFORE deduplication or target admission (TASK-2344, R4).
    if let Err(err) = inv.check_submission_fence(&request_id, &dedupe_key, &target_key) {
        cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, false);
        emit_dispatch_event(
            main_worktree,
            &caller.mandate().id,
            action.verb_str(),
            &action.subject_label(),
            Outcome::Refused,
            Some(format!("{err:#}")),
        );
        return Err(err);
    }

    // Check existing active requests for idempotency or target collision.
    match submission_collision(&inv, &dedupe_key, &target_key) {
        Some(SubmissionCollision::Duplicate { request, result }) => {
            cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, false);
            return Ok(SubmittedDispatch {
                request,
                result,
                deduplicated: true,
            });
        }
        Some(SubmissionCollision::TargetBusy(msg)) => {
            cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, false);
            emit_dispatch_event(
                main_worktree,
                &caller.mandate().id,
                action.verb_str(),
                &action.subject_label(),
                Outcome::Refused,
                Some(msg.clone()),
            );
            bail!("{msg}");
        }
        None => {}
    }

    let req = caller.admitted_request(
        request_id.clone(),
        now,
        principal,
        dedupe_key.clone(),
        target_key.clone(),
        action.clone(),
    );

    // Verify dual-end validator agrees with what we are about to write.
    if let Err(err) = revalidate_request(
        main_worktree,
        mandate_dir,
        &req,
        caller.mandate().include_supervised,
    ) {
        cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, false);
        bail!("internal validation check failed: {err:#}");
    }

    let summary = if action.requires_serve() {
        format!(
            "queued {} {} (waiting for serve)",
            action.verb_str(),
            action.subject_label()
        )
    } else {
        format!("queued {} {}", action.verb_str(), action.subject_label())
    };
    let res = caller.pending_result(
        &req,
        summary.clone(),
        format!("{LOGS_SUBDIR}/{request_id}.log"),
    );

    let res_bytes = serde_json::to_vec_pretty(&res)?;
    let req_bytes = serde_json::to_vec_pretty(&req)?;

    #[cfg(test)]
    if TEST_FAIL_SUBMIT_STAGE.with(|c| c.get()) == 1 {
        cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, false);
        bail!("simulated submit failure before result write");
    }

    // 1. Write `results/<id>.json` (`Pending`) FIRST so no published
    //    `requests/<id>.json` can ever exist without its companion result file.
    if let Err(err) = write_private_atomic(&result_path(main_worktree, &request_id), &res_bytes) {
        cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, false);
        return Err(err);
    }

    #[cfg(test)]
    if TEST_SIMULATE_CRASH_AFTER_RESULT_WRITE.with(|c| c.get()) {
        // Simulate an abrupt process crash after writing `results/<id>.json`
        // before `requests/<id>.json` is written (skipping same-invocation
        // cleanup so the next invocation sees an orphan Pending result).
        bail!("simulated hard crash after result write before request write");
    }

    #[cfg(test)]
    if TEST_FAIL_SUBMIT_STAGE.with(|c| c.get()) == 2 {
        cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, true);
        bail!("simulated submit failure after result write before request write");
    }

    // 2. Publish `requests/<id>.json`. If this write fails within the same
    //    invocation under `_lock`, roll back the just-written `results/<id>.json`
    //    and `briefs/<id>.md`.
    if let Err(err) = write_private_atomic(&request_path(main_worktree, &request_id), &req_bytes) {
        cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, true);
        return Err(err);
    }

    emit_dispatch_event(
        main_worktree,
        &caller.mandate().id,
        action.verb_str(),
        &action.subject_label(),
        Outcome::Routed,
        Some(summary),
    );

    Ok(SubmittedDispatch {
        request: req,
        result: res,
        deduplicated: false,
    })
}

/// Read one `(DispatchRequest, DispatchResult)` pair by exact ID.
// trace:STORY-1917 | ai:agy
pub(crate) fn load_pair(
    main_worktree: &Path,
    id: &str,
) -> Result<(DispatchRequest, DispatchResult)> {
    if Uuid::parse_str(id).is_err() {
        bail!("invalid dispatch request id `{id}`");
    }
    let req_bytes =
        read_bounded_regular_file(&request_path(main_worktree, id), MAX_BRIEF_BYTES, true)?;
    let req: DispatchRequest = serde_json::from_slice(&req_bytes)
        .with_context(|| format!("malformed request JSON for {id}"))?;
    if req.id() != id {
        bail!("request file {id}.json has mismatched id `{}`", req.id());
    }
    let res_bytes =
        read_bounded_regular_file(&result_path(main_worktree, id), MAX_BRIEF_BYTES, true)?;
    let res: DispatchResult = serde_json::from_slice(&res_bytes)
        .with_context(|| format!("malformed result JSON for {id}"))?;
    if res.id() != id {
        bail!("result file {id}.json has mismatched id `{}`", res.id());
    }
    Ok((req, res))
}

/// Resolve an exact UUID or unique prefix to `(DispatchRequest, DispatchResult)`.
// trace:STORY-1917 | ai:agy
pub(crate) fn resolve_pair(
    main_worktree: &Path,
    id_or_prefix: &str,
) -> Result<(DispatchRequest, DispatchResult)> {
    let needle = id_or_prefix.trim();
    if Uuid::parse_str(needle).is_ok() {
        return load_pair(main_worktree, needle);
    }
    let mut matches: Vec<(DispatchRequest, DispatchResult)> = load_all_pairs(main_worktree)?
        .into_iter()
        .filter(|(req, _)| req.id().starts_with(needle))
        .collect();
    match matches.len() {
        0 => bail!("no dispatch request matching `{needle}`"),
        1 => Ok(matches.remove(0)),
        _ => bail!("ambiguous dispatch request prefix `{needle}`"),
    }
}

/// Load all valid `(DispatchRequest, DispatchResult)` pairs sorted by
/// `requested_at` ascending (then `id`) without mutating disk or spamming
/// stderr warnings (TASK-2344, R4).
// trace:STORY-1917 trace:TASK-2344 | ai:agy
pub(crate) fn load_all_pairs(
    main_worktree: &Path,
) -> Result<Vec<(DispatchRequest, DispatchResult)>> {
    Ok(inspect_dispatch_inventory_readonly(main_worktree)?.pairs)
}

/// Cancel a `Pending` dispatch request.
// trace:STORY-1917 trace:TASK-2344 | ai:agy
pub(crate) fn cancel_request(
    main_worktree: &Path,
    id_or_prefix: &str,
    now: DateTime<Utc>,
) -> Result<DispatchResult> {
    let _lock = acquire_dispatch_lock(main_worktree)?;
    let inv = reconcile_inventory_locked(main_worktree, now)?;
    if let Some(fence) = inv.find_fence(id_or_prefix) {
        bail!(
            "cannot cancel dispatch request {}: record is in `needs-recovery` state ({})",
            fence.id,
            fence.reason
        );
    }
    let (req, mut res) = resolve_pair(main_worktree, id_or_prefix)?;
    res.cancel(&req, now)?;
    write_private_atomic(
        &result_path(main_worktree, &req.id()),
        &serde_json::to_vec_pretty(&res)?,
    )?;
    emit_dispatch_event(
        main_worktree,
        &req.mandate_id(),
        req.action().verb_str(),
        &req.action().subject_label(),
        Outcome::Skipped,
        Some("cancelled".to_string()),
    );
    Ok(res)
}

/// Human-facing status label for `aida dispatch list`: per ADVISOR SIGNOFF,
/// `Pending` requests for `review`, `flow`, and `train` display as
/// `"pending (waiting for serve)"`.
// trace:STORY-1917 | ai:agy
pub(crate) fn display_status(req: &DispatchRequest, res: &DispatchResult) -> String {
    if res.status() == DispatchStatus::Pending && req.action().requires_serve() {
        "pending (waiting for serve)".to_string()
    } else {
        res.status().as_str().to_string()
    }
}

pub(crate) fn format_list_text(pairs: &[(DispatchRequest, DispatchResult)]) -> String {
    format_inventory_list_text(pairs, &[])
}

pub(crate) fn format_inventory_list_text(
    pairs: &[(DispatchRequest, DispatchResult)],
    fences: &[RecoveryFenceEntry],
) -> String {
    if pairs.is_empty() && fences.is_empty() {
        return "No dispatch requests.\n".to_string();
    }
    let mut out = String::new();
    for (req, res) in pairs {
        let short_id: String = req.id().chars().take(8).collect();
        let status = display_status(req, res);
        let ts = req.requested_at().format("%Y-%m-%d %H:%M:%S");
        out.push_str(&format!(
            "{short_id}  {:<28}  {:<6}  {:<18}  {}  {}\n",
            status,
            req.action().verb_str(),
            req.target_key(),
            ts,
            res.summary()
        ));
    }
    for fence in fences {
        let short_id: String = fence.id.chars().take(8).collect();
        let verb = fence.known_verb.as_deref().unwrap_or("unknown");
        let target = fence.known_target_key.as_deref().unwrap_or("unknown");
        let ts = fence
            .known_requested_at
            .unwrap_or(fence.detected_at)
            .format("%Y-%m-%d %H:%M:%S");
        out.push_str(&format!(
            "{short_id}  {:<28}  {:<6}  {:<18}  {}  {}\n",
            "needs-recovery", verb, target, ts, fence.reason
        ));
    }
    out
}

pub(crate) fn list_json(pairs: &[(DispatchRequest, DispatchResult)]) -> serde_json::Value {
    inventory_list_json(pairs, &[])
}

pub(crate) fn inventory_list_json(
    pairs: &[(DispatchRequest, DispatchResult)],
    fences: &[RecoveryFenceEntry],
) -> serde_json::Value {
    let mut rows: Vec<serde_json::Value> = pairs
        .iter()
        .map(|(req, res)| {
            serde_json::json!({
                "id": req.id(),
                "verb": req.action().verb_str(),
                "target": req.target_key(),
                "dedupe_key": req.dedupe_key(),
                "status": res.status().as_str(),
                "display_status": display_status(req, res),
                "waiting_for_serve": res.status() == DispatchStatus::Pending && req.action().requires_serve(),
                "requested_at": req.requested_at().to_rfc3339(),
                "started_at": res.started_at().map(|t| t.to_rfc3339()),
                "finished_at": res.finished_at().map(|t| t.to_rfc3339()),
                "mandate_id": req.mandate_id(),
                "requester_grant_prefix": res.requester_grant_prefix(),
                "summary": res.summary(),
                "refusal_reason": res.refusal_reason,
                "request": req,
                "result": res,
            })
        })
        .collect();
    for fence in fences {
        rows.push(serde_json::json!({
            "id": fence.id,
            "verb": fence.known_verb.as_deref().unwrap_or("unknown"),
            "target": fence.known_target_key.as_deref().unwrap_or("unknown"),
            "dedupe_key": fence.known_dedupe_key,
            "status": "needs-recovery",
            "display_status": "needs-recovery",
            "waiting_for_serve": false,
            "requested_at": fence.known_requested_at.unwrap_or(fence.detected_at).to_rfc3339(),
            "started_at": serde_json::Value::Null,
            "finished_at": serde_json::Value::Null,
            "mandate_id": fence.known_mandate_id,
            "requester_grant_prefix": serde_json::Value::Null,
            "summary": fence.reason,
            "refusal_reason": fence.reason,
            "recovery": fence,
        }));
    }
    serde_json::Value::Array(rows)
}

// ── CLI Entrypoint ──────────────────────────────────────────────────────────

pub(crate) fn handle_dispatch(action: &DispatchAction) -> Result<()> {
    let main_worktree = crate::find_main_worktree_root()
        .or_else(|_| std::env::current_dir())
        .context("cannot locate main worktree root")?;
    let mandate_dir = crate::gate::mandate::mandate_dir()?;
    let now = Utc::now();

    match action {
        DispatchAction::Request { verb, json } => {
            let principal = crate::current_user_id(None);
            let submitted = submit_request(&main_worktree, &mandate_dir, &principal, verb, now)?;
            if *json {
                let payload = serde_json::json!({
                    "deduplicated": submitted.deduplicated,
                    "waiting_for_serve": submitted.result.status() == DispatchStatus::Pending
                        && submitted.request.action().requires_serve(),
                    "display_status": display_status(&submitted.request, &submitted.result),
                    "request": submitted.request,
                    "result": submitted.result,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(());
            }
            if submitted.deduplicated {
                println!(
                    "already active: {} ({})",
                    submitted.request.id(),
                    display_status(&submitted.request, &submitted.result)
                );
            } else {
                println!(
                    "queued: {} ({} {} — {})",
                    submitted.request.id(),
                    submitted.request.action().verb_str(),
                    submitted.request.action().subject_label(),
                    display_status(&submitted.request, &submitted.result)
                );
            }
            Ok(())
        }
        DispatchAction::List { status, json } => {
            let inv = inspect_dispatch_inventory_readonly(&main_worktree)?;
            let mut pairs = inv.pairs;
            let fences = if status.is_none() {
                inv.fences
            } else {
                Vec::new()
            };
            if let Some(filter) = status {
                pairs.retain(|(_, res)| res.status().matches_filter(*filter));
            }
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&inventory_list_json(&pairs, &fences))?
                );
            } else {
                print!("{}", format_inventory_list_text(&pairs, &fences));
            }
            Ok(())
        }
        DispatchAction::Show { id, json } => {
            let inv = inspect_dispatch_inventory_readonly(&main_worktree)?;
            if let Some(fence) = inv.find_fence(id) {
                if *json {
                    let payload = serde_json::json!({
                        "display_status": "needs-recovery",
                        "waiting_for_serve": false,
                        "recovery": fence,
                    });
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                } else {
                    println!("id:             {}", fence.id);
                    println!(
                        "verb:           {}",
                        fence.known_verb.as_deref().unwrap_or("unknown")
                    );
                    println!(
                        "target:         {}",
                        fence.known_target_key.as_deref().unwrap_or("unknown")
                    );
                    println!("status:         needs-recovery");
                    println!("detected_at:    {}", fence.detected_at.to_rfc3339());
                    println!("summary:        {}", fence.reason);
                }
                return Ok(());
            }
            let (req, res) = resolve_pair(&main_worktree, id)?;
            if *json {
                let payload = serde_json::json!({
                    "display_status": display_status(&req, &res),
                    "waiting_for_serve": res.status() == DispatchStatus::Pending && req.action().requires_serve(),
                    "request": req,
                    "result": res,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("id:             {}", req.id());
                println!("verb:           {}", req.action().verb_str());
                println!("target:         {}", req.target_key());
                println!("dedupe_key:     {}", req.dedupe_key());
                println!("status:         {}", display_status(&req, &res));
                println!("requested_at:   {}", req.requested_at().to_rfc3339());
                if let Some(started) = res.started_at() {
                    println!("started_at:     {}", started.to_rfc3339());
                }
                if let Some(finished) = res.finished_at() {
                    println!("finished_at:    {}", finished.to_rfc3339());
                }
                println!("mandate_id:     {}", req.mandate_id());
                println!("grant_prefix:   {}", res.requester_grant_prefix());
                println!("summary:        {}", res.summary());
                if let Some(reason) = &res.refusal_reason {
                    println!("refusal_reason: {reason}");
                }
                println!("log_path:       {}", res.log_path);
            }
            Ok(())
        }
        DispatchAction::Cancel { id } => {
            let res = cancel_request(&main_worktree, id, now)?;
            println!("cancelled: {} ({})", res.id(), res.summary());
            Ok(())
        }
        DispatchAction::Tick => {
            let summary = run_dispatch_tick(&main_worktree, &mandate_dir, now)?;
            if summary.spawned > 0 || summary.reaped > 0 {
                println!(
                    "dispatch tick: spawned={}, reaped={}, waiting_for_serve={}",
                    summary.spawned, summary.reaped, summary.waiting_for_serve
                );
            }
            Ok(())
        }
        DispatchAction::Serve { once, poll_ms } => {
            run_dispatch_serve(&main_worktree, &mandate_dir, *once, *poll_ms)
        }
        DispatchAction::ActivateLock => {
            // trace:STORY-1917 trace:TASK-2344 | ai:claude
            let lock = activate_lock_as_human(&main_worktree)?;
            drop(lock);
            println!(
                "dispatch lock active: {} is the v2 lock directory",
                dispatch_root(&main_worktree).join(TICK_LOCK_FILE).display()
            );
            Ok(())
        }
        DispatchAction::Run { id, via_serve } => {
            let res = execute_one_request(&main_worktree, &mandate_dir, id, *via_serve, now)?;
            println!(
                "{}: {} ({})",
                res.status().as_str(),
                res.id(),
                res.summary()
            );
            Ok(())
        }
    }
}

// ── Slice 2: Executor Core, Serve, and Review / Take / Impl Runners ─────────

pub(crate) const MAX_DISPATCH_LOG_BYTES: usize = 1024 * 1024;
pub(crate) const STALE_GIT_CONFIG_LOCK_AGE_SECS: u64 = 120;

/// Summary of one `dispatch tick` or `dispatch serve` claim/reap pass.
// trace:STORY-1917 | ai:agy
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DispatchPassSummary {
    pub reaped: usize,
    pub spawned: usize,
    pub claimed_ids: Vec<String>,
    pub waiting_for_serve: usize,
}

/// Safe stale `.git/config.lock` cleanup: unlinks `<main_worktree>/.git/config.lock`
/// ONLY when `symlink_metadata` confirms a regular file (never a symlink),
/// `len() == 0`, mode `0o444`, and `mtime >= 120s` in the past relative to `now`.
/// Returns `Ok(true)` when a stale lock was removed, `Ok(false)` otherwise.
// trace:STORY-1917 | ai:agy
pub(crate) fn clean_stale_git_config_lock(
    main_worktree: &Path,
    now: std::time::SystemTime,
) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let lock_path = main_worktree.join(".git").join("config.lock");
    let meta = match std::fs::symlink_metadata(&lock_path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("cannot stat {}", lock_path.display())),
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Ok(false);
    }
    if meta.len() != 0 {
        return Ok(false);
    }
    if (meta.mode() & 0o777) != 0o444 {
        return Ok(false);
    }
    let Ok(mtime) = meta.modified() else {
        return Ok(false);
    };
    let Ok(age) = now.duration_since(mtime) else {
        return Ok(false);
    };
    if age < std::time::Duration::from_secs(STALE_GIT_CONFIG_LOCK_AGE_SECS) {
        return Ok(false);
    }
    match std::fs::remove_file(&lock_path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("cannot unlink {}", lock_path.display())),
    }
}

/// Send one best-effort mailbox notification to `advisor` when a request
/// reaches a terminal state (`Done`, `Refused`, or `Failed`), and return the
/// notification timestamp if delivery succeeded.
// trace:STORY-1917 | ai:agy
pub(crate) fn notify_advisor_outcome(
    main_worktree: &Path,
    req: &DispatchRequest,
    res: &DispatchResult,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    use aida_core::mailbox::{Intent, Message, Recipient};
    let subject = format!(
        "dispatch {} {}: {} ({})",
        req.action().verb_str(),
        req.action().subject_label(),
        res.status().as_str(),
        &req.id()[..8.min(req.id().len())]
    );
    let body = match &res.refusal_reason {
        Some(reason) => format!(
            "Dispatch request {} ({} {}) {}: {}\nreason: {}\nlog: {}",
            req.id(),
            req.action().verb_str(),
            req.action().subject_label(),
            res.status().as_str(),
            res.summary(),
            reason,
            res.log_path
        ),
        None => format!(
            "Dispatch request {} ({} {}) {}: {}\nlog: {}",
            req.id(),
            req.action().verb_str(),
            req.action().subject_label(),
            res.status().as_str(),
            res.summary(),
            res.log_path
        ),
    };
    let msg_id = Uuid::new_v4().to_string();
    let msg = Message {
        id: msg_id.clone(),
        thread_id: msg_id,
        from: "dispatch".to_string(),
        to: Recipient::Agent("advisor".to_string()),
        timestamp: now.timestamp_millis(),
        in_reply_to: None,
        body,
        subject: Some(subject),
        urgent: matches!(
            res.status(),
            DispatchStatus::Refused | DispatchStatus::Failed
        ),
        intent: Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: aida_core::mailbox::SenderSource::Explicit,
        from_role: Some("dispatch".to_string()),
        relayed_from: None,
    };
    if crate::mailbox_store::write_message(main_worktree, &msg).is_ok() {
        crate::events::emit(
            main_worktree,
            &crate::events::Event::new(
                None,
                "",
                crate::events::EventKind::MailReceived {
                    to: "advisor".to_string(),
                },
            ),
        );
        Some(now)
    } else {
        None
    }
}

pub(crate) const DISPATCH_CLAIM_TOKEN_ENV: &str = "AIDA_DISPATCH_CLAIM_TOKEN";

/// Append bounded log text to `.aida/dispatch/logs/<id>.log` (mode 0600, capped
/// at [`MAX_DISPATCH_LOG_BYTES`]) through a pinned, no-follow regular-file
/// descriptor so symlinks and hardlinks can neither disclose outside files nor
/// redirect writes (F1).
// trace:STORY-1917 | ai:agy
pub(crate) fn append_dispatch_log(main_worktree: &Path, id: &str, text: &str) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut file = open_pinned_log_file(main_worktree, id, false)?;
    file.seek(SeekFrom::Start(0))?;
    let mut existing = Vec::new();
    (&file)
        .take((MAX_DISPATCH_LOG_BYTES as u64) + 1)
        .read_to_end(&mut existing)
        .with_context(|| format!("cannot read log for {id}"))?;
    if existing.len() > MAX_DISPATCH_LOG_BYTES {
        bail!("log for {id} exceeds maximum size ({MAX_DISPATCH_LOG_BYTES} bytes)");
    }
    existing.extend_from_slice(text.as_bytes());
    if existing.len() > MAX_DISPATCH_LOG_BYTES {
        let drop_len = existing.len() - MAX_DISPATCH_LOG_BYTES;
        existing.drain(..drop_len);
    }
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&existing)?;
    file.sync_all()?;
    Ok(())
}

/// Fail every `Running` claim the gate's [`dead_claims`] releases (positive
/// `Probe::Dead` evidence), notify the advisor and log the event. Caller must
/// hold `acquire_dispatch_lock`.
// trace:STORY-1917 trace:TASK-2344 | ai:agy
// trace:STORY-1889 | ai:claude
pub(crate) fn reap_dead_workers_locked(main_worktree: &Path, now: DateTime<Utc>) -> Result<usize> {
    let inv = reconcile_inventory_locked(main_worktree, now)?;
    let dead = dead_claims(&inv);
    let mut reaped = 0usize;
    for (req, mut res) in inv.pairs {
        let Some((_, reason)) = dead.iter().find(|(id, _)| id == req.id()) else {
            continue;
        };
        res.fail_running(
            format!(
                "failed {} {}: {reason}",
                req.action().verb_str(),
                req.action().subject_label()
            ),
            reason.clone(),
            false,
            now,
        )?;
        res.notified_at = notify_advisor_outcome(main_worktree, &req, &res, now);
        write_private_atomic(
            &result_path(main_worktree, req.id()),
            &serde_json::to_vec_pretty(&res)?,
        )?;
        emit_dispatch_event(
            main_worktree,
            req.mandate_id(),
            req.action().verb_str(),
            &req.action().subject_label(),
            Outcome::Skipped,
            Some(reason.clone()),
        );
        reaped += 1;
    }
    Ok(reaped)
}

/// Spawn a detached `aida dispatch run <id> [--via-serve]` worker process,
/// redirecting stdout/stderr to the pinned `.aida/dispatch/logs/<id>.log`
/// (mode 0600, `O_NOFOLLOW`) and passing `claim_token` via
/// [`DISPATCH_CLAIM_TOKEN_ENV`].
// trace:STORY-1917 | ai:agy
pub(crate) fn spawn_detached_worker(
    main_worktree: &Path,
    id: &str,
    via_serve: bool,
    claim_token: &str,
) -> Result<(std::process::Child, u32, Option<String>)> {
    let aida_bin = resolve_aida_bin()?;
    let log_out = open_pinned_log_file(main_worktree, id, true)?;
    let log_err = log_out.try_clone()?;

    let mut cmd = std::process::Command::new(aida_bin);
    cmd.current_dir(main_worktree)
        .args(["dispatch", "run", id])
        .env(DISPATCH_CLAIM_TOKEN_ENV, claim_token);
    if via_serve {
        cmd.arg("--via-serve");
    } else {
        // Cron/schedule tick workers never inherit ambient session grants/roles.
        for key in crate::mandate_drive::SESSION_ENV {
            cmd.env_remove(key);
        }
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log_out))
        .stderr(std::process::Stdio::from(log_err));

    let child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn `aida dispatch run {id}`"))?;
    let pid = child.id();
    let start = crate::process_probe::process_start_identity(pid);
    Ok((child, pid, start))
}

fn publish_spawned_worker_claim(
    main_worktree: &Path,
    id: &str,
    res: &DispatchResult,
) -> Result<()> {
    #[cfg(test)]
    if TEST_FAIL_POST_SPAWN_PUBLICATION.with(|c| c.get()) {
        bail!("simulated post-spawn worker PID publication failure");
    }
    write_private_atomic(
        &result_path(main_worktree, id),
        &serde_json::to_vec_pretty(res)?,
    )
}

/// Claim eligible `Pending` requests under `tick.lock` (moving `Pending ->
/// Running` and persisting `claim_token` BEFORE spawning so no unrecorded child
/// can race ahead of `Running` publication), enforcing concurrency caps
/// (2 reviews, 4 flows, 1 train).
// trace:STORY-1917 | ai:agy
pub(crate) fn claim_and_dispatch_pass(
    main_worktree: &Path,
    via_serve: bool,
    spawn_detached: bool,
    now: DateTime<Utc>,
) -> Result<DispatchPassSummary> {
    let _ = clean_stale_git_config_lock(main_worktree, std::time::SystemTime::from(now));
    let _lock = acquire_dispatch_lock(main_worktree)?;
    let reaped = reap_dead_workers_locked(main_worktree, now)?;
    let inv = reconcile_inventory_locked(main_worktree, now)?;
    let mut caps = ClaimCaps::from_inventory(&inv, None)?;
    let pairs = inv.pairs.clone();

    let mut summary = DispatchPassSummary {
        reaped,
        ..Default::default()
    };

    let self_pid = std::process::id();
    let self_start = crate::process_probe::process_start_identity(self_pid);

    for (req, mut res) in pairs {
        if res.status() != DispatchStatus::Pending {
            continue;
        }
        let admission = match pass_admission(&inv, &req, via_serve, &mut caps) {
            PassAdmission::WaitingForServe => {
                summary.waiting_for_serve += 1;
                continue;
            }
            PassAdmission::Skip => continue,
            PassAdmission::Claim(admission) => admission,
        };

        // Persist the Running claim and fresh claim_token BEFORE spawning (F2).
        // When `spawn_detached` is true, `worker_pid` starts as `None` until the
        // spawned child's `(pid, start)` is durably published under `tick.lock`;
        // if post-spawn publication fails, the child is immediately killed and
        // reaped under `tick.lock` and the claim is rolled back to `Failed`.
        let claim_token = Uuid::new_v4().to_string();
        res.begin_running(
            admission,
            Some(&req),
            (!spawn_detached).then(|| (self_pid, self_start.clone())),
            claim_token.clone(),
            now,
        )?;
        write_private_atomic(
            &result_path(main_worktree, &req.id()),
            &serde_json::to_vec_pretty(&res)?,
        )?;

        if spawn_detached {
            match spawn_detached_worker(main_worktree, &req.id(), via_serve, &claim_token) {
                Ok((mut child, pid, start)) => {
                    res.publish_worker(&req, pid, start);
                    if let Err(pub_err) =
                        publish_spawned_worker_claim(main_worktree, &req.id(), &res)
                    {
                        // Parent publication failed: kill and reap the spawned
                        // child while still holding `tick.lock` so no unowned
                        // child can ever execute work, then fail the claim.
                        let _ = child.kill();
                        let _ = child.wait();
                        caps.release(&req.action());
                        let reason = format!(
                            "failed to publish dispatch worker claim for pid {pid}: {pub_err:#}"
                        );
                        res.fail_running(reason.clone(), reason, true, now)?;
                        res.notified_at = notify_advisor_outcome(main_worktree, &req, &res, now);
                        // A lost Failed write would leave a Running claim with
                        // no worker that the reaper cannot judge, holding a cap
                        // slot forever: stop the pass and name the record.
                        // trace:STORY-1917 trace:TASK-2344 | ai:claude
                        write_private_atomic(
                            &result_path(main_worktree, &req.id()),
                            &serde_json::to_vec_pretty(&res)?,
                        )
                        .with_context(|| {
                            format!(
                                "dispatch request {} was left Running with no worker after its worker was killed ({reason}); the Failed result could not be written. {RECOVERY_GUIDANCE}",
                                req.id(),
                                reason = res.summary()
                            )
                        })?;
                        continue;
                    }
                }
                Err(err) => {
                    caps.release(&req.action());
                    let reason = format!("failed to spawn dispatch worker: {err:#}");
                    res.fail_running(reason.clone(), reason, true, now)?;
                    res.notified_at = notify_advisor_outcome(main_worktree, &req, &res, now);
                    write_private_atomic(
                        &result_path(main_worktree, &req.id()),
                        &serde_json::to_vec_pretty(&res)?,
                    )?;
                    continue;
                }
            }
        }

        summary.spawned += 1;
        summary.claimed_ids.push(req.id().to_string());
    }

    Ok(summary)
}

/// Run one substrate `aida dispatch tick` pass: executes only `take` and `impl`
/// requests (leaves `review`, `flow`, and `train` `Pending` as `waiting for serve`).
// trace:STORY-1917 | ai:agy
pub(crate) fn run_dispatch_tick(
    main_worktree: &Path,
    _mandate_dir: &Path,
    now: DateTime<Utc>,
) -> Result<DispatchPassSummary> {
    claim_and_dispatch_pass(main_worktree, false, true, now)
}

/// Synchronous test/in-process tick helper: claims eligible `take`/`impl`
/// requests (`via_serve = false`) and runs them in-process, leaving `review`,
/// `flow`, and `train` `Pending`.
// trace:STORY-1917 | ai:agy
pub(crate) fn run_dispatch_tick_sync(
    main_worktree: &Path,
    mandate_dir: &Path,
    now: DateTime<Utc>,
) -> Result<DispatchPassSummary> {
    let summary = claim_and_dispatch_pass(main_worktree, false, false, now)?;
    for id in &summary.claimed_ids {
        let token = load_pair(main_worktree, id)
            .ok()
            .and_then(|(_, res)| res.claim_token().map(str::to_string));
        let _ = execute_one_request_with_token(
            main_worktree,
            mandate_dir,
            id,
            false,
            token.as_deref(),
            now,
        );
    }
    Ok(summary)
}

/// Synchronous `serve --once` helper: verifies the caller's bound `advisor`
/// grant first (refusing to start without one), then claims and executes
/// eligible requests.
// trace:STORY-1917 | ai:agy
pub(crate) fn run_dispatch_serve_once_sync(
    main_worktree: &Path,
    mandate_dir: &Path,
    now: DateTime<Utc>,
) -> Result<DispatchPassSummary> {
    let _grant = check_serve_grant(main_worktree, now)?;
    let summary = claim_and_dispatch_pass(main_worktree, true, false, now)?;
    let self_pid = std::process::id();
    let self_start = crate::process_probe::process_start_identity(self_pid);
    for id in &summary.claimed_ids {
        let token = load_pair(main_worktree, id)
            .ok()
            .and_then(|(_, res)| res.claim_token().map(str::to_string));
        // Re-check the advisor grant before each request so mid-pass expiry
        // stops immediately and reverts unclaimed requests back to Pending.
        if check_serve_grant(main_worktree, Utc::now()).is_err() {
            if let Ok(_lock) = acquire_dispatch_lock(main_worktree) {
                if let Ok((_req, mut res)) = load_pair(main_worktree, id) {
                    if res.revert_unstarted_claim(self_pid, self_start.as_deref(), token.as_deref())
                    {
                        let _ = write_private_atomic(
                            &result_path(main_worktree, id),
                            &serde_json::to_vec_pretty(&res)?,
                        );
                    }
                }
            }
            continue;
        }
        let _ = execute_one_request_with_token(
            main_worktree,
            mandate_dir,
            id,
            true,
            token.as_deref(),
            now,
        );
    }
    Ok(summary)
}

/// Long-running `aida dispatch serve` loop started from a human advisor
/// terminal after `eval "$(aida role enter advisor ...)"`.
// trace:STORY-1917 | ai:agy
pub(crate) fn run_dispatch_serve(
    main_worktree: &Path,
    _mandate_dir: &Path,
    once: bool,
    poll_ms: u64,
) -> Result<()> {
    let now = Utc::now();
    let grant = check_serve_grant(main_worktree, now)?;
    println!(
        "dispatch serve: bound advisor grant {} active until {}",
        &grant.id[..8.min(grant.id.len())],
        grant.expires_at.to_rfc3339()
    );
    loop {
        let tick_now = Utc::now();
        if let Err(err) = check_serve_grant(main_worktree, tick_now) {
            eprintln!("dispatch serve stopping (pending requests left pending): {err:#}");
            return Ok(());
        }
        let summary = claim_and_dispatch_pass(main_worktree, true, true, tick_now)?;
        if summary.spawned > 0 || summary.reaped > 0 {
            println!(
                "dispatch serve: spawned={}, reaped={}",
                summary.spawned, summary.reaped
            );
        }
        if once {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(poll_ms.clamp(100, 60_000)));
    }
}

/// Execute a single dispatch request (`aida dispatch run <id> [--via-serve]`).
///
/// Enforces:
/// 1. Atomic claim/ownership check under `tick.lock` (F2): a `Running` request
///    requires this process's `(worker_pid, worker_start, claim_token)` to
///    match the persisted claim; a `Pending` request checks concurrency caps
///    and persists a fresh claim before running.
/// 2. Dual-end re-validation of `requests/<id>.json` and any frozen brief file
///    (`O_NOFOLLOW | O_NONBLOCK`, mode 0600, current uid, SHA-256 match).
/// 3. Hard Boundary B rule (ADVISOR SIGNOFF): if `req.action.requires_serve()`
///    (`review`, `flow`, `train`) and `!via_serve`, refuses immediately without
///    touching any grant. When `via_serve` is true, verifies `check_serve_grant`
///    (inherited binding; never self-binds the grant).
/// 4. Exact-ID Mandate re-check (`mandate::recheck(mandate_dir, &req.mandate_id, ...)` — F3),
///    `HumanGates` (`GateOp::Dispatch` / `GateOp::Ship`), and requester grant
///    re-check on disk (`seat_authority::grant_authorized_at`).
// trace:STORY-1917 | ai:agy
pub(crate) fn execute_one_request(
    main_worktree: &Path,
    mandate_dir: &Path,
    id: &str,
    via_serve: bool,
    now: DateTime<Utc>,
) -> Result<DispatchResult> {
    let env_token = std::env::var(DISPATCH_CLAIM_TOKEN_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty());
    execute_one_request_with_token(
        main_worktree,
        mandate_dir,
        id,
        via_serve,
        env_token.as_deref(),
        now,
    )
}

pub(crate) fn execute_one_request_with_token(
    main_worktree: &Path,
    mandate_dir: &Path,
    id: &str,
    via_serve: bool,
    presented_token: Option<&str>,
    now: DateTime<Utc>,
) -> Result<DispatchResult> {
    let _ = clean_stale_git_config_lock(main_worktree, std::time::SystemTime::from(now));

    if Uuid::parse_str(id).is_err() {
        bail!("invalid dispatch request id `{id}`");
    }

    let self_pid = std::process::id();
    let self_start = crate::process_probe::process_start_identity(self_pid);

    // Acquire `tick.lock` to verify or establish exclusive worker ownership (F2)
    // BEFORE reading/validating the request or recording any refusal.
    let (mut res, claim, raw_req_res) = {
        let _lock = acquire_dispatch_lock(main_worktree)?;
        let inv = reconcile_inventory_locked(main_worktree, now)?;
        if let Some(fence) = inv.find_fence(id) {
            bail!(
                "dispatch request {id} is in `needs-recovery` state and cannot be executed: {}. {RECOVERY_GUIDANCE}",
                fence.reason
            );
        }

        let res_bytes =
            read_bounded_regular_file(&result_path(main_worktree, id), MAX_BRIEF_BYTES, true)?;
        let mut res: DispatchResult = serde_json::from_slice(&res_bytes)
            .with_context(|| format!("malformed result JSON for {id}"))?;
        if res.id() != id {
            bail!("result file {id}.json has mismatched id `{}`", res.id());
        }
        let entry = res.execution_entry(id)?;

        let raw_req_res =
            read_bounded_regular_file(&request_path(main_worktree, id), MAX_BRIEF_BYTES, true)
                .and_then(|bytes| {
                    serde_json::from_slice::<DispatchRequest>(&bytes)
                        .with_context(|| format!("malformed request JSON for {id}"))
                });

        let claim = match entry {
            ExecutionEntry::Running => {
                match verify_running_claim(id, &res, presented_token, self_pid, &self_start)? {
                    RunningClaim::Owned(claim) => claim,
                    RunningClaim::Unpublished { reason, refusal } => {
                        res.fail_running(reason.clone(), reason, true, now)?;
                        let _ = write_private_atomic(
                            &result_path(main_worktree, id),
                            &serde_json::to_vec_pretty(&res)?,
                        );
                        bail!("{refusal}");
                    }
                    RunningClaim::Foreign { dead, refusal } => {
                        if dead {
                            let _ = reap_dead_workers_locked(main_worktree, now);
                        }
                        bail!("{refusal}");
                    }
                }
            }
            ExecutionEntry::Pending => {
                let _ = reap_dead_workers_locked(main_worktree, now);
                let inv_after_reap = reconcile_inventory_locked(main_worktree, now)?;
                let admission = run_admission(&inv_after_reap, id, &raw_req_res)?;
                let claim_token = Uuid::new_v4().to_string();
                res.begin_running(
                    admission,
                    None,
                    Some((self_pid, self_start.clone())),
                    claim_token.clone(),
                    now,
                )?;
                write_private_atomic(
                    &result_path(main_worktree, id),
                    &serde_json::to_vec_pretty(&res)?,
                )?;
                WorkerClaim {
                    worker_pid: self_pid,
                    worker_start: self_start.clone(),
                    claim_token,
                }
            }
        };

        (res, claim, raw_req_res)
    };

    let req: DispatchRequest = match raw_req_res {
        Ok(req) if req.id() == id => req,
        Ok(req) => {
            let reason = format!(
                "tampered request JSON for {id}: inner id `{}` does not match `{id}`",
                req.id()
            );
            return record_tampered_refusal(main_worktree, id, &claim, &mut res, &reason, now);
        }
        Err(err) => {
            let reason = format!("request validation failed on disk: {err:#}");
            return record_tampered_refusal(main_worktree, id, &claim, &mut res, &reason, now);
        }
    };

    // 1-5. Execution-time admission (Boundary B, serve grant, exact mandate,
    // human gates, requester grant, dual-end re-validation). Every runner
    // takes the admission it returns.
    let admission = match admit_execution(main_worktree, mandate_dir, &req, via_serve, now) {
        Ok(admission) => admission,
        Err(refusal) => {
            return finalize_request(
                main_worktree,
                &req,
                &claim,
                res,
                DispatchStatus::Refused,
                refusal.outcome,
                None,
                None,
                refusal.reason.clone(),
                Some(refusal.reason),
                now,
            );
        }
    };

    // 6. Execute the verb's runner.
    let runner_outcome = match &req.action() {
        DispatchVerb::Review { pr, vendor } => {
            run_review_verb(main_worktree, &req, &admission, *pr, *vendor)
        }
        DispatchVerb::Take {
            pr,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            vendor,
        } => run_take_verb(
            main_worktree,
            &req,
            &admission,
            *pr,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            *vendor,
            now,
        ),
        DispatchVerb::Impl {
            name,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            vendor,
        } => run_impl_verb(
            main_worktree,
            &req,
            &admission,
            name,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            *vendor,
            now,
        ),
        DispatchVerb::Flow {
            pr,
            no_ship,
            vendor,
        } => dispatch_flow::run_flow_verb(
            main_worktree,
            mandate_dir,
            &req,
            &admission,
            &claim,
            *pr,
            *no_ship,
            *vendor,
            now,
        ),
        DispatchVerb::Train { prs } => dispatch_flow::run_train_verb(
            main_worktree,
            mandate_dir,
            &req,
            &admission,
            &claim,
            prs,
            now,
        ),
    };

    match runner_outcome {
        RunnerOutcome::Done {
            summary,
            spawned_session,
            exit_code,
        } => finalize_request(
            main_worktree,
            &req,
            &claim,
            res,
            DispatchStatus::Done,
            Outcome::Done,
            spawned_session,
            exit_code,
            summary,
            None,
            now,
        ),
        RunnerOutcome::Refused { reason } => finalize_request(
            main_worktree,
            &req,
            &claim,
            res,
            DispatchStatus::Refused,
            Outcome::Refused,
            None,
            None,
            reason.clone(),
            Some(reason),
            now,
        ),
        RunnerOutcome::Failed { summary, exit_code } => finalize_request(
            main_worktree,
            &req,
            &claim,
            res,
            DispatchStatus::Failed,
            Outcome::Skipped,
            None,
            exit_code,
            summary.clone(),
            Some(summary),
            now,
        ),
    }
}

#[derive(Debug)]
pub(crate) enum RunnerOutcome {
    Done {
        summary: String,
        spawned_session: Option<String>,
        exit_code: Option<i32>,
    },
    Refused {
        reason: String,
    },
    Failed {
        summary: String,
        exit_code: Option<i32>,
    },
}

pub(crate) fn record_tampered_refusal(
    main_worktree: &Path,
    id: &str,
    claim: &WorkerClaim,
    res: &mut DispatchResult,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<DispatchResult> {
    let _lock = acquire_dispatch_lock(main_worktree)?;
    verify_claim_locked(main_worktree, id, claim)?;
    // Log only once the gate has accepted the transition.
    // trace:STORY-1889 | ai:claude
    res.finish(
        DispatchStatus::Refused,
        reason.to_string(),
        Some(reason.to_string()),
        None,
        None,
        now,
    )?;
    let _ = append_dispatch_log(main_worktree, id, &format!("refused: {reason}\n"));
    write_private_atomic(
        &result_path(main_worktree, id),
        &serde_json::to_vec_pretty(res)?,
    )?;
    emit_dispatch_event(
        main_worktree,
        &res.mandate_id(),
        &res.verb,
        &res.target,
        Outcome::Refused,
        Some(reason.to_string()),
    );
    Ok(res.clone())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn finalize_request(
    main_worktree: &Path,
    req: &DispatchRequest,
    claim: &WorkerClaim,
    mut res: DispatchResult,
    status: DispatchStatus,
    event_outcome: Outcome,
    spawned_session: Option<String>,
    exit_code: Option<i32>,
    summary: String,
    refusal_reason: Option<String>,
    now: DateTime<Utc>,
) -> Result<DispatchResult> {
    let _lock = acquire_dispatch_lock(main_worktree)?;
    verify_claim_locked(main_worktree, &req.id(), claim)?;
    // Log only once the gate has accepted the transition.
    // trace:STORY-1889 | ai:claude
    res.finish(
        status,
        summary.clone(),
        refusal_reason.clone(),
        spawned_session,
        exit_code,
        now,
    )?;
    let _ = append_dispatch_log(
        main_worktree,
        &req.id(),
        &format!("{}: {}\n", status.as_str(), summary),
    );
    res.notified_at = notify_advisor_outcome(main_worktree, req, &res, now);
    write_private_atomic(
        &result_path(main_worktree, &req.id()),
        &serde_json::to_vec_pretty(&res)?,
    )?;
    emit_dispatch_event(
        main_worktree,
        &req.mandate_id(),
        req.action().verb_str(),
        &req.action().subject_label(),
        event_outcome,
        refusal_reason.or(Some(summary)),
    );
    Ok(res)
}

/// Resolve the sibling `<project>-worktrees` root directory next to `main_worktree`.
// trace:STORY-1917 | ai:agy
pub(crate) fn worktrees_root(main_worktree: &Path) -> PathBuf {
    let parent = main_worktree.parent().unwrap_or(main_worktree);
    let repo_name = main_worktree
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("aida");
    parent.join(format!("{repo_name}-worktrees"))
}

fn worktree_current_branch(wt: &Path) -> Option<String> {
    if !wt.exists() {
        return None;
    }
    let out = aida_core::git_ops::host_git_command(wt)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!b.is_empty()).then_some(b)
}

/// Scan `/proc/<pid>/cwd` for any live non-zombie process (excluding the calling
/// process) whose current working directory is equal to or inside `wt_canon`.
fn find_live_proc_with_cwd_in(wt_canon: &Path) -> Option<u32> {
    let self_pid = std::process::id();
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !name_str.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = name_str.parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let Ok(raw_cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) else {
            continue;
        };
        let raw_str = raw_cwd.to_string_lossy();
        let cleaned = Path::new(raw_str.strip_suffix(" (deleted)").unwrap_or(&raw_str));
        let proc_canon = cleaned
            .canonicalize()
            .unwrap_or_else(|_| cleaned.to_path_buf());
        if proc_canon == wt_canon || proc_canon.starts_with(wt_canon) {
            if crate::process_probe::pid_is_alive(pid) {
                return Some(pid);
            }
        }
    }
    None
}

/// Check if any live or unknown occupant owns `worktree`, `branch`, or `scope`
/// across session leases (`.aida/sessions/`), the agent process registry
/// (`.aida/agents/`), or live OS processes running inside the target worktree
/// (F5). Returns a descriptive refusal reason when occupied or when registry/lease
/// state is unreadable/unknown.
// trace:STORY-1917 | ai:agy
pub(crate) fn check_live_worktree_or_branch_occupant(
    main_worktree: &Path,
    worktree: &Path,
    branch: Option<&str>,
    scope: Option<&str>,
) -> Option<String> {
    let live_sessions = crate::process_probe::probe_live_claude_sessions();
    let now = Utc::now();
    let main_canon = main_worktree
        .canonicalize()
        .unwrap_or_else(|_| main_worktree.to_path_buf());
    let target_canon = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf());

    // 1. Check `.aida/sessions/` leases (treating unreadable/corrupt lease files
    //    as unknown occupants so cleanup/branch reset fails closed).
    let leases_dir = crate::leases::leases_dir(main_worktree);
    match std::fs::read_dir(&leases_dir) {
        Ok(entries) => {
            for entry in entries {
                let Ok(entry) = entry else {
                    return Some(format!(
                        "unknown occupant: unreadable entry in {}",
                        leases_dir.display()
                    ));
                };
                let p = entry.path();
                if p.extension().and_then(|s| s.to_str()) != Some("toml") {
                    continue;
                }
                let Ok(body) = std::fs::read_to_string(&p) else {
                    return Some(format!(
                        "unknown occupant: cannot read session lease {}",
                        p.display()
                    ));
                };
                if toml::from_str::<crate::SessionLease>(&body).is_err() {
                    return Some(format!(
                        "unknown occupant: malformed session lease {}",
                        p.display()
                    ));
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Some(format!(
                "unknown occupant: cannot read {}: {e}",
                leases_dir.display()
            ));
        }
    }

    for lease in crate::leases::list_leases(main_worktree) {
        let state = crate::leases::lease_state_for(&lease, &live_sessions, now);
        let active_pid_live = lease.active_pid.is_some_and(|pid| {
            crate::process_probe::process_identity_is_alive(
                pid,
                lease.active_pid_start_time.as_deref(),
            )
        });
        let is_live = matches!(state, crate::LeaseState::Live)
            || active_pid_live
            || crate::agent_registry::live_agents_covering_cwd(main_worktree, &lease.worktree_path)
                > 0;
        if !is_live {
            continue;
        }
        let lease_canon = lease
            .worktree_path
            .canonicalize()
            .unwrap_or_else(|_| lease.worktree_path.clone());
        if !lease.worktree_path.as_os_str().is_empty() && lease_canon == target_canon {
            return Some(format!(
                "worktree {} is currently occupied by live session lease {} (scope `{}`, role `{}`)",
                worktree.display(),
                lease.id,
                lease.scope,
                lease.role.as_deref().unwrap_or("unspecified")
            ));
        }
        if let Some(b) = branch {
            if !lease.branch.is_empty() && lease.branch == b {
                return Some(format!(
                    "branch `{b}` is currently occupied by live session lease {} in {}",
                    lease.id,
                    lease.worktree_path.display()
                ));
            }
        }
        if let Some(s) = scope {
            if lease.scope.eq_ignore_ascii_case(s) {
                return Some(format!(
                    "scope `{s}` is currently occupied by live session lease {} in {}",
                    lease.id,
                    lease.worktree_path.display()
                ));
            }
        }
    }

    // 2. Check `.aida/agents/*.toml` independently of leases (F5): any live or
    //    unobservable/unknown agent (or unreadable/corrupt registry file)
    //    retains its worktree, branch, and scope.
    let observer = crate::agent_registry::PidObserver::current();
    let mut agent_dirs = vec![crate::agent_registry::agents_dir(main_worktree)];
    let wt_agents_dir = worktree.join(".aida").join("agents");
    if wt_agents_dir.exists() {
        let main_agents_canon = agent_dirs[0]
            .canonicalize()
            .unwrap_or_else(|_| agent_dirs[0].clone());
        let wt_agents_canon = wt_agents_dir
            .canonicalize()
            .unwrap_or_else(|_| wt_agents_dir.clone());
        if wt_agents_canon != main_agents_canon {
            agent_dirs.push(wt_agents_dir);
        }
    }
    let wt_dir_name = worktree.file_name().and_then(|n| n.to_str());
    for ag_dir in agent_dirs {
        let entries = match std::fs::read_dir(&ag_dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Some(format!(
                    "unknown occupant: cannot read agent registry {}: {e}",
                    ag_dir.display()
                ));
            }
        };
        for entry in entries {
            let Ok(entry) = entry else {
                return Some(format!(
                    "unknown occupant: unreadable entry in {}",
                    ag_dir.display()
                ));
            };
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&path) else {
                return Some(format!(
                    "unknown occupant: cannot read agent registry record {}",
                    path.display()
                ));
            };
            let Ok(record) = toml::from_str::<crate::agent_registry::AgentRegistryEntry>(&body)
            else {
                return Some(format!(
                    "unknown occupant: malformed agent registry record {}",
                    path.display()
                ));
            };
            if record.ended_at.is_some() {
                continue;
            }
            let observable = observer.can_observe(record.pid_namespace);
            let live_or_unknown = if !observable {
                true
            } else {
                crate::process_probe::process_identity_is_alive(
                    record.pid,
                    record.pid_start.as_deref(),
                )
            };
            if !live_or_unknown {
                continue;
            }
            let state_word = if observable {
                "live"
            } else {
                "unknown-liveness"
            };
            let label = record.name.as_deref().unwrap_or(&record.id);
            let rec_wt = record
                .worktree_path
                .canonicalize()
                .unwrap_or_else(|_| record.worktree_path.clone());
            let wt_matches = !record.worktree_path.as_os_str().is_empty()
                && (rec_wt == target_canon
                    || (rec_wt != main_canon
                        && target_canon != main_canon
                        && (target_canon.starts_with(&rec_wt)
                            || rec_wt.starts_with(&target_canon))));
            if wt_matches {
                return Some(format!(
                    "worktree {} is currently occupied by {state_word} registered agent `{label}` (pid {})",
                    worktree.display(),
                    record.pid
                ));
            }
            if let Some(wt_name) = wt_dir_name {
                if record
                    .name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(wt_name))
                {
                    return Some(format!(
                        "lane `{wt_name}` is currently occupied by {state_word} registered agent `{label}` (pid {})",
                        record.pid
                    ));
                }
            }
            if let Some(s) = scope {
                if record
                    .current_spec
                    .as_deref()
                    .is_some_and(|cs| cs.eq_ignore_ascii_case(s))
                    || record
                        .name
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(s))
                {
                    return Some(format!(
                        "scope `{s}` is currently occupied by {state_word} registered agent `{label}` (pid {}) in {}",
                        record.pid,
                        record.worktree_path.display()
                    ));
                }
            }
            if let Some(b) = branch {
                if !record.worktree_path.as_os_str().is_empty() && rec_wt != main_canon {
                    if worktree_current_branch(&record.worktree_path).as_deref() == Some(b) {
                        return Some(format!(
                            "branch `{b}` is currently occupied by {state_word} registered agent `{label}` (pid {}) in {}",
                            record.pid,
                            record.worktree_path.display()
                        ));
                    }
                }
            }
        }
    }

    // 3. Direct process / cwd liveness probe for `worktree` and any linked
    //    worktree currently holding `branch` (independent of both leases and
    //    registry entries — catches raw vendor processes launched by take/impl).
    let mut probe_wts = Vec::new();
    if target_canon != main_canon && worktree.exists() {
        probe_wts.push((worktree.to_path_buf(), target_canon.clone()));
    }
    if let Some(b) = branch {
        if let Some(b_wt) = find_worktree_for_branch(main_worktree, b) {
            let b_canon = b_wt.canonicalize().unwrap_or_else(|_| b_wt.clone());
            if b_canon != main_canon && !probe_wts.iter().any(|(_, c)| *c == b_canon) {
                probe_wts.push((b_wt, b_canon));
            }
        }
    }
    for (wt_display, wt_canon) in &probe_wts {
        for sess in &live_sessions {
            let sess_canon = sess.cwd.canonicalize().unwrap_or_else(|_| sess.cwd.clone());
            if sess_canon == *wt_canon || sess_canon.starts_with(wt_canon) {
                return Some(format!(
                    "worktree {} is currently occupied by live claude process (pid {})",
                    wt_display.display(),
                    sess.pid
                ));
            }
        }
        if let Some(pid) = find_live_proc_with_cwd_in(wt_canon) {
            return Some(format!(
                "worktree {} is currently occupied by live process (pid {pid})",
                wt_display.display()
            ));
        }
    }

    None
}

/// Prepare a dead reviewer worktree (`aida-pr-<pr>`) before launching a fresh
/// review:
/// 1. Refuses if a live or unknown occupant (lease, registered agent, or live
///    process) currently occupies `rev_wt`, `pr-<pr>`, or `PR-<pr>` (F5).
/// 2. Atomically preserves any `<rev_wt>/.aida/merge-holds/PR-<pr>` marker into
///    `<main_worktree>/.aida/merge-holds/PR-<pr>` with verified no-clobber
///    transfer, aborting cleanup on any preservation failure (F4).
/// 3. Removes `rev_wt` via `git worktree remove --force`.
///
/// Steps 2 and 3 are the gate's [`remove_reviewer_worktree_preserving_hold`].
// trace:STORY-1917 | ai:agy
// trace:STORY-1889 | ai:claude
pub(crate) fn cleanup_dead_reviewer_worktree(main_worktree: &Path, pr: u64) -> Result<()> {
    let pr_scope = format!("PR-{pr}");
    let pr_branch = format!("pr-{pr}");
    let candidates = [
        worktrees_root(main_worktree).join(format!("aida-pr-{pr}")),
        main_worktree
            .parent()
            .unwrap_or(main_worktree)
            .join(format!("aida-pr-{pr}")),
    ];
    for rev_wt in candidates {
        if let Some(busy) = check_live_worktree_or_branch_occupant(
            main_worktree,
            &rev_wt,
            Some(&pr_branch),
            Some(&pr_scope),
        ) {
            bail!("refusing to clean up reviewer worktree: {busy}");
        }
        if !rev_wt.exists() && std::fs::symlink_metadata(&rev_wt).is_err() {
            continue;
        }
        remove_reviewer_worktree_preserving_hold(main_worktree, &rev_wt, &pr_scope)?;
    }
    // Best-effort cleanup of a leftover `pr-<pr>` branch after removing any dead
    // reviewer worktree, matching `review-one.sh`.
    let _ = aida_core::git_ops::host_git_command(main_worktree)
        .args(["branch", "-D", &pr_branch])
        .output();
    Ok(())
}

#[cfg(test)]
std::thread_local! {
    pub(crate) static TEST_AIDA_BIN: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    pub(crate) static TEST_AGENT_BIN: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    pub(crate) static TEST_FAIL_POST_SPAWN_PUBLICATION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(crate) static TEST_FAIL_SUBMIT_STAGE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
    pub(crate) static TEST_SIMULATE_CRASH_AFTER_RESULT_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn resolve_aida_bin() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(p) = TEST_AIDA_BIN.with(|c| c.borrow().clone()) {
        return Ok(p);
    }
    crate::aida_bin::process()
        .map(|r| r.path)
        .or_else(|_| Ok(crate::aida_exe_path().to_path_buf()))
}

fn resolve_vendor_bin(vendor: DispatchVendor) -> PathBuf {
    #[cfg(test)]
    if let Some(p) = TEST_AGENT_BIN.with(|c| c.borrow().clone()) {
        return p;
    }
    let name = match vendor {
        DispatchVendor::Claude => "claude",
        DispatchVendor::Codex => "codex",
        DispatchVendor::Agy => "agy",
    };
    crate::find_executable_on_path(name).unwrap_or_else(|| PathBuf::from(name))
}

/// Run the `review` verb (`aida queue work PR-<pr> --role reviewer --no-human [--vendor <v>]`).
/// Runs only under `aida dispatch serve` so it inherits the human advisor's
/// bound seat grant directly.
// trace:STORY-1917 | ai:agy
pub(crate) fn run_review_verb(
    main_worktree: &Path,
    req: &DispatchRequest,
    _admission: &ExecutionAdmission,
    pr: u64,
    vendor: Option<DispatchVendor>,
) -> RunnerOutcome {
    if let Err(err) = cleanup_dead_reviewer_worktree(main_worktree, pr) {
        return RunnerOutcome::Refused {
            reason: format!("{err:#}"),
        };
    }

    let aida_bin = match resolve_aida_bin() {
        Ok(b) => b,
        Err(err) => {
            return RunnerOutcome::Failed {
                summary: format!("cannot locate aida binary: {err:#}"),
                exit_code: None,
            }
        }
    };

    let pr_arg = format!("PR-{pr}");
    let mut cmd = std::process::Command::new(aida_bin);
    cmd.current_dir(main_worktree).args([
        "queue",
        "work",
        &pr_arg,
        "--role",
        "reviewer",
        "--no-human",
    ]);
    if let Some(v) = vendor {
        cmd.args(["--vendor", v.as_str()]);
    }
    cmd.env_remove("AIDA_SESSION_PURPOSE")
        .env_remove(DISPATCH_CLAIM_TOKEN_ENV);

    match cmd.output() {
        Ok(out) => {
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let _ = append_dispatch_log(main_worktree, &req.id(), &combined);
            let code = out.status.code();
            if out.status.success() {
                RunnerOutcome::Done {
                    summary: format!("completed review of PR #{pr}"),
                    spawned_session: None,
                    exit_code: code,
                }
            } else {
                RunnerOutcome::Failed {
                    summary: format!("review of PR #{pr} exited with {:?}", code),
                    exit_code: code,
                }
            }
        }
        Err(err) => RunnerOutcome::Failed {
            summary: format!("failed to spawn `aida queue work PR-{pr}`: {err:#}"),
            exit_code: None,
        },
    }
}

/// If `refs/heads/<branch>` is already checked out in a linked worktree, return
/// that worktree's path so `take` can reuse it rather than failing `git worktree add`.
// trace:STORY-1917 | ai:agy
fn find_worktree_for_branch(main_worktree: &Path, branch: &str) -> Option<PathBuf> {
    let out = aida_core::git_ops::host_git_command(main_worktree)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let want_branch = format!("branch refs/heads/{branch}");
    let main_canon = main_worktree
        .canonicalize()
        .unwrap_or_else(|_| main_worktree.to_path_buf());
    let mut current_wt: Option<PathBuf> = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            current_wt = Some(PathBuf::from(rest.trim()));
        } else if line.trim() == want_branch {
            if let Some(wt) = current_wt.take() {
                let wt_canon = wt.canonicalize().unwrap_or_else(|_| wt.clone());
                if wt_canon != main_canon {
                    return Some(wt);
                }
            }
        } else if line.trim().is_empty() {
            current_wt = None;
        }
    }
    None
}

fn git_ref_exists(repo: &Path, git_ref: &str) -> bool {
    aida_core::git_ops::host_git_command(repo)
        .args(["rev-parse", "--verify", "--quiet", git_ref])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Prepare or reuse a worktree at `wt_path` on `branch` tracking `start_ref`
/// (e.g. `origin/<branch>` for `take`, or `origin/main` for `impl`) without
/// ever force-pushing. Refuses if `wt_path` or `branch` is currently occupied
/// by a live or unknown process/agent/lease (F5).
// trace:STORY-1917 | ai:agy
pub(crate) fn prepare_lane_worktree(
    main_worktree: &Path,
    wt_path: &Path,
    branch: &str,
    fetch_ref: &str,
    start_ref: &str,
) -> Result<()> {
    if let Some(busy) =
        check_live_worktree_or_branch_occupant(main_worktree, wt_path, Some(branch), None)
    {
        bail!("refusing to modify occupied worktree or branch: {busy}");
    }
    if let Some(parent) = wt_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Best-effort fetch of the base/branch from origin if `origin` remote is configured.
    let has_origin = aida_core::git_ops::host_git_command(main_worktree)
        .args(["remote", "get-url", "origin"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if has_origin {
        let fetch_out = aida_core::git_ops::host_remote_git_command(main_worktree, "origin")
            .args(["fetch", "origin", fetch_ref])
            .output()
            .with_context(|| format!("failed to run git fetch origin {fetch_ref}"))?;
        if !fetch_out.status.success() {
            bail!(
                "git fetch origin {fetch_ref} failed: {}",
                String::from_utf8_lossy(&fetch_out.stderr).trim()
            );
        }
    }

    let resolved_start = if git_ref_exists(main_worktree, start_ref) {
        start_ref.to_string()
    } else if git_ref_exists(main_worktree, &format!("refs/heads/{fetch_ref}")) {
        fetch_ref.to_string()
    } else if git_ref_exists(main_worktree, &format!("refs/heads/{branch}")) {
        branch.to_string()
    } else {
        "HEAD".to_string()
    };

    if wt_path.exists() {
        let out = aida_core::git_ops::host_git_command(wt_path)
            .args(["checkout", "-B", branch, &resolved_start])
            .output()
            .with_context(|| format!("failed to checkout {branch} in {}", wt_path.display()))?;
        if !out.status.success() {
            bail!(
                "git checkout -B {branch} {resolved_start} in {} failed: {}",
                wt_path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    } else {
        let out = aida_core::git_ops::host_git_command(main_worktree)
            .args(["worktree", "add", "-B", branch])
            .arg(wt_path)
            .arg(&resolved_start)
            .output()
            .with_context(|| format!("failed to add worktree {}", wt_path.display()))?;
        if !out.status.success() {
            bail!(
                "git worktree add -B {branch} {} {resolved_start} failed: {}",
                wt_path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    }

    // Ensure `.aida` inside the worktree links back to `main_worktree/.aida`
    // so the implementer sees the project config, sessions, and mailbox.
    let wt_aida = wt_path.join(".aida");
    if !wt_aida.exists() && std::fs::symlink_metadata(&wt_aida).is_err() {
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(main_worktree.join(".aida"), &wt_aida);
    }
    Ok(())
}

/// Build the scrubbed `Command` that launches an implementer worker in `wt_path`
/// with the session grant and purpose environment variables cleared and
/// `AIDA_SESSION_ROLE="implementer"`.
// trace:STORY-1917 | ai:agy
pub(crate) fn build_implementer_launch_command(
    agent_bin: &Path,
    main_worktree: &Path,
    wt_path: &Path,
    spec: &str,
    prompt_text: &str,
    name: &str,
    vendor: DispatchVendor,
) -> std::process::Command {
    let mut cmd = std::process::Command::new(agent_bin);
    cmd.current_dir(wt_path);
    match vendor {
        DispatchVendor::Claude => {
            cmd.args(["--bg", "-n", name, prompt_text]);
        }
        DispatchVendor::Codex => {
            cmd.args(["exec", "--full-auto", "-C"])
                .arg(wt_path)
                .arg(prompt_text);
        }
        DispatchVendor::Agy => {
            cmd.args(["--prompt", prompt_text]);
        }
    }
    // Critical Boundary B requirement: implementer lanes never inherit or hold
    // any session grant, purpose, or dispatch worker claim token.
    cmd.env(crate::gate::seat_authority::GRANT_ENV, "")
        .env(crate::gate::seat_authority::GRANT_FILE_ENV, "")
        .env("AIDA_SESSION_PURPOSE", "")
        .env_remove(DISPATCH_CLAIM_TOKEN_ENV)
        .env("AIDA_SESSION_ROLE", "implementer")
        .env("AIDA_SESSION_SCOPE", spec)
        .env("AIDA_PROJECT_ROOT", main_worktree)
        .env("AIDA_AGENT_NAME", name)
        .env("AIDA_USER", name);
    cmd
}

#[allow(clippy::too_many_arguments)]
fn run_take_verb(
    main_worktree: &Path,
    req: &DispatchRequest,
    _admission: &ExecutionAdmission,
    pr: u64,
    branch: &str,
    spec: &str,
    brief_relpath: &str,
    brief_sha256: &str,
    vendor: Option<DispatchVendor>,
    now: DateTime<Utc>,
) -> RunnerOutcome {
    let brief_body =
        match verify_frozen_brief(main_worktree, &req.id(), brief_relpath, brief_sha256) {
            Ok(b) => b,
            Err(err) => {
                return RunnerOutcome::Refused {
                    reason: format!("frozen brief verification failed: {err:#}"),
                }
            }
        };

    let wt_name = format!("take-{pr}");
    let default_wt = worktrees_root(main_worktree).join(&wt_name);
    // Reuse an existing worktree that already has `branch` checked out when
    // `take-<pr>` does not exist yet (matching `take.sh`).
    let wt_path = if !default_wt.exists() {
        find_worktree_for_branch(main_worktree, branch).unwrap_or(default_wt)
    } else {
        default_wt
    };
    if let Some(busy) =
        check_live_worktree_or_branch_occupant(main_worktree, &wt_path, Some(branch), Some(spec))
    {
        return RunnerOutcome::Refused {
            reason: format!("cannot launch takeover on PR #{pr}: {busy}"),
        };
    }

    let start_ref = format!("origin/{branch}");
    if let Err(err) = prepare_lane_worktree(main_worktree, &wt_path, branch, branch, &start_ref) {
        return RunnerOutcome::Failed {
            summary: format!("takeover worktree setup failed for PR #{pr}: {err:#}"),
            exit_code: None,
        };
    }

    let prompt_text =
        match render_lane_prompt(main_worktree, &wt_path, &req.action(), &brief_body, now) {
            Ok(p) => p,
            Err(err) => {
                return RunnerOutcome::Failed {
                    summary: format!("failed to render takeover prompt: {err:#}"),
                    exit_code: None,
                }
            }
        };
    let rendered_prompt_path = briefs_dir(main_worktree).join(format!("{}.prompt.md", req.id()));
    if let Err(err) = write_private_atomic(&rendered_prompt_path, prompt_text.as_bytes()) {
        return RunnerOutcome::Failed {
            summary: format!("failed to write rendered prompt: {err:#}"),
            exit_code: None,
        };
    }

    launch_implementer_lane(
        main_worktree,
        req,
        &wt_path,
        spec,
        &prompt_text,
        &wt_name,
        vendor.unwrap_or(DispatchVendor::Claude),
    )
}

#[allow(clippy::too_many_arguments)]
fn run_impl_verb(
    main_worktree: &Path,
    req: &DispatchRequest,
    _admission: &ExecutionAdmission,
    name: &str,
    branch: &str,
    spec: &str,
    brief_relpath: &str,
    brief_sha256: &str,
    vendor: Option<DispatchVendor>,
    now: DateTime<Utc>,
) -> RunnerOutcome {
    let brief_body =
        match verify_frozen_brief(main_worktree, &req.id(), brief_relpath, brief_sha256) {
            Ok(b) => b,
            Err(err) => {
                return RunnerOutcome::Refused {
                    reason: format!("frozen brief verification failed: {err:#}"),
                }
            }
        };

    let wt_path = worktrees_root(main_worktree).join(name);
    if let Some(busy) =
        check_live_worktree_or_branch_occupant(main_worktree, &wt_path, Some(branch), Some(spec))
    {
        return RunnerOutcome::Refused {
            reason: format!("cannot launch impl lane `{name}` ({spec}): {busy}"),
        };
    }

    let default_branch = match resolve_verified_local_default_branch(main_worktree) {
        Ok(b) => b,
        Err(err) => {
            return RunnerOutcome::Refused {
                reason: format!("default branch verification failed for `{name}`: {err:#}"),
            };
        }
    };
    let default_remote_ref = format!("origin/{default_branch}");
    if let Err(err) = prepare_lane_worktree(
        main_worktree,
        &wt_path,
        branch,
        &default_branch,
        &default_remote_ref,
    ) {
        return RunnerOutcome::Failed {
            summary: format!("impl worktree setup failed for `{name}`: {err:#}"),
            exit_code: None,
        };
    }

    let prompt_text =
        match render_lane_prompt(main_worktree, &wt_path, &req.action(), &brief_body, now) {
            Ok(p) => p,
            Err(err) => {
                return RunnerOutcome::Failed {
                    summary: format!("failed to render impl prompt: {err:#}"),
                    exit_code: None,
                }
            }
        };
    let rendered_prompt_path = briefs_dir(main_worktree).join(format!("{}.prompt.md", req.id()));
    if let Err(err) = write_private_atomic(&rendered_prompt_path, prompt_text.as_bytes()) {
        return RunnerOutcome::Failed {
            summary: format!("failed to write rendered prompt: {err:#}"),
            exit_code: None,
        };
    }

    launch_implementer_lane(
        main_worktree,
        req,
        &wt_path,
        spec,
        &prompt_text,
        name,
        vendor.unwrap_or(DispatchVendor::Claude),
    )
}

fn parse_spawned_bg_session_id(stdout: &str) -> Option<String> {
    if let Some(id) = crate::agent_launch::parse_bg_session_id(stdout) {
        return Some(id);
    }
    for line in stdout.lines() {
        if let Some((_, rest)) = line.split_once("claude logs ") {
            let token = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
            if !token.is_empty() && token.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                return Some(token.to_string());
            }
        }
    }
    None
}

fn launch_implementer_lane(
    main_worktree: &Path,
    req: &DispatchRequest,
    wt_path: &Path,
    spec: &str,
    prompt_text: &str,
    lane_name: &str,
    vendor: DispatchVendor,
) -> RunnerOutcome {
    let agent_bin = resolve_vendor_bin(vendor);

    let mut cmd = build_implementer_launch_command(
        &agent_bin,
        main_worktree,
        wt_path,
        spec,
        prompt_text,
        lane_name,
        vendor,
    );

    match cmd.output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{stdout}{stderr}");
            let _ = append_dispatch_log(main_worktree, &req.id(), &combined);
            let code = out.status.code();
            if out.status.success() {
                let bg_id = parse_spawned_bg_session_id(&stdout);
                RunnerOutcome::Done {
                    summary: format!(
                        "launched {} {} in {}",
                        req.action().verb_str(),
                        req.action().subject_label(),
                        wt_path.display()
                    ),
                    spawned_session: bg_id,
                    exit_code: code,
                }
            } else {
                RunnerOutcome::Failed {
                    summary: format!(
                        "{} {} launcher exited with {:?}: {}",
                        req.action().verb_str(),
                        req.action().subject_label(),
                        code,
                        stderr.lines().next().unwrap_or("launch failed").trim()
                    ),
                    exit_code: code,
                }
            }
        }
        Err(err) => RunnerOutcome::Failed {
            summary: format!("failed to spawn implementer launcher: {err:#}"),
            exit_code: None,
        },
    }
}

/// Queue and inspect typed dispatch requests (`flow`, `review`, `take`,
/// `impl`, `train`) from a sandboxed advisor seat for unsandboxed execution.
// trace:STORY-1917 | ai:agy
#[derive(clap::Parser, Debug)]
#[clap(name = "dispatch")]
pub(crate) struct DispatchArgs {
    #[clap(subcommand)]
    pub action: crate::cli::DispatchAction,
}

fn run_dispatch(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <DispatchArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    handle_dispatch(&args.action)
}

// trace:STORY-1917 | ai:agy
pub(crate) static DISPATCH: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "dispatch",
        help_after: "fleet",
        command: <DispatchArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_dispatch),
    };
