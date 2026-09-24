//! The into-Completed seam (STORY-1418).
//!
//! Every CLI path that moves a spec into `Completed` goes through this module:
//!
//! - [`transition_to_completed`] — the single-spec path: stamp Completed, let
//!   the caller persist it by whatever write path it already uses, then emit
//!   the `SpecCompleted` ship record. `aida done`, the queue's close action and
//!   `aida promote --auto-complete` use it.
//! - [`mark_completed`] — the in-memory stamp alone, for paths whose
//!   persistence and emission are separated by work this module cannot own:
//!   the batch auto-bump / reconcile flips (stamped inside an
//!   `update_atomically` closure, emitted only for flips the reloaded store
//!   confirms) and `aida edit`, which persists a targeted YAML write several
//!   steps later. Those callers emit through [`emit_spec_completed`].
//!
//! The write paths genuinely differ (BUG-1286's finding), so the seam owns the
//! status stamp and the emission, not the storage call. The guard test
//! `completion_seam_guard_tests` fails when a new direct `Completed`
//! assignment appears outside this module, or when a file stamps Completed
//! through [`mark_completed`] without also emitting.
// trace:STORY-1418 | ai:claude

use aida_core::{Requirement, RequirementStatus};
use anyhow::Result;

/// Stamp `req` as Completed in memory and return its prior status. Does not
/// persist and does not emit — the caller must do both (see module docs).
///
/// Also stamps `implementation_info.completed_at` when it's not already
/// set. Before this, only the merge-driven auto-bump paths (`apply_auto_bump_
/// flip` and friends in `lib.rs`) stamped it — via their own `info.completed_
/// at.get_or_insert(now)` call, made *after* this function returns — so a
/// manual completion (`aida edit --status completed`, `aida done`, queue
/// close, `aida promote`) fell back to `modified_at`, which a later edit
/// (tag/comment) reorders under `aida list --sort completed`. Stamping here,
/// first, means every into-Completed path gets a `completed_at` once; the
/// auto-bump call sites' own `get_or_insert` becomes a no-op on that path and
/// never overwrites this stamp.
// trace:TASK-1477 | ai:claude
// trace:STORY-1418 | ai:claude
pub(crate) fn mark_completed(req: &mut Requirement) -> RequirementStatus {
    let prior = req.status.clone();
    req.set_status_from_str("Completed");
    let info = req
        .implementation_info
        .get_or_insert_with(aida_core::ImplementationInfo::default);
    info.completed_at.get_or_insert_with(chrono::Utc::now);
    prior
}

/// Move `req` into Completed, persist it with `persist`, and emit the durable
/// ship record when this was a real transition (prior status not Completed).
///
/// `persist` receives the already-stamped requirement so the caller can add
/// its history entry / timestamps and write it by its own storage path. The
/// event is emitted only after `persist` succeeds. `project_root` of `None`
/// (a store path with no parent) skips emission, as the call sites did before.
/// Returns whether an into-Completed transition happened.
// trace:STORY-1418 | ai:claude
pub(crate) fn transition_to_completed(
    req: &mut Requirement,
    project_root: Option<&std::path::Path>,
    spec_id: &str,
    sha: &str,
    closed_by: &str,
    persist: impl FnOnce(&mut Requirement, &RequirementStatus) -> Result<()>,
) -> Result<bool> {
    let prior = mark_completed(req);
    persist(req, &prior)?;
    let into = crate::is_into_completed_transition(&prior, "Completed");
    if into {
        if let Some(project_root) = project_root {
            emit_spec_completed(project_root, spec_id, sha, None, closed_by);
        }
    }
    Ok(into)
}

/// Clear a stale `implementation_info.completed_at` when a status edit takes
/// `req` OUT of Completed (a reopen: `aida edit --status <non-Completed>`,
/// `aida queue rework`/`queue_rework` on a Completed spec). Call this AFTER
/// the status has actually left Completed, using the status captured BEFORE
/// the mutation.
///
/// `mark_completed`'s stamp is absent-only (`get_or_insert_with`), so
/// without this a reopen followed by a re-complete would keep the FIRST
/// completion's date forever, misordering `aida list --sort completed`.
/// `completion_sha` is left untouched — BUG-410's reopen guard depends on it
/// surviving a reopen to detect "this exact commit already completed it
/// once".
// trace:TASK-1477 | ai:claude
pub(crate) fn clear_completed_at_on_reopen(req: &mut Requirement, prior: &RequirementStatus) {
    if matches!(prior, RequirementStatus::Completed) {
        if let Some(info) = req.implementation_info.as_mut() {
            info.completed_at = None;
        }
    }
}

/// Emit the durable ship record after the store confirms a transition to
/// `Completed`. Best-effort like every event-stream write.
// trace:BUG-1286 | ai:codex
// trace:STORY-1418 | ai:claude
pub(crate) fn emit_spec_completed(
    project_root: &std::path::Path,
    spec_id: &str,
    sha: &str,
    pr: Option<u64>,
    closed_by: &str,
) {
    let pr = pr.or_else(|| {
        if sha.is_empty() {
            return None;
        }
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["show", "-s", "--format=%s", sha])
            .output()
            .ok()?;
        output.status.success().then_some(())?;
        crate::extract_pr_number_from_commit_subject(String::from_utf8_lossy(&output.stdout).trim())
    });
    let (_, run_uuid) = crate::drain_state::current_context(project_root);
    crate::events::emit(
        project_root,
        &crate::events::Event::new(
            Some(spec_id.to_string()),
            run_uuid,
            crate::events::EventKind::SpecCompleted {
                commit: sha.to_string(),
                pr,
                closed_by: closed_by.to_string(),
            },
        ),
    );
}
