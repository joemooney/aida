//! Retracting a PR the publication guards refused (ADR-51: retract-after,
//! not prevent-before).
//!
//! `/aida-pr` opens the PR inside the implementer session, before the
//! orchestrator regains control, so the pre-flight guards can only run AFTER
//! publication. When they refuse, the orchestrator retracts the PR. This
//! module holds the pieces of that retraction that are pure or filesystem
//! only, so they are testable without a forge:
//!
//! - which PR may be retracted at all (only one the agent opened during this
//!   phase — never one that was already open when the phase started);
//! - what "retracted" means (the forge reports the change CLOSED afterwards —
//!   a close call whose outcome is never read is a capability, not a guard);
//! - the durable marker under `.aida/preflight-retractions/` that records the
//!   retraction, or its failure, so a seat that later finds the PR learns why
//!   from the substrate rather than from scrollback.
// trace:TASK-1529 | ai:claude

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Which PR, if any, the refused-preflight retraction may act on.
// trace:TASK-1529 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RetractionTarget {
    /// The agent opened this PR during the phase: retract it.
    Retract(u64),
    /// This PR was already open on the branch before the implementer ran, so
    /// the agent did not open it. Never touched.
    NotOurs(u64),
    /// No open PR exists on the branch; nothing to retract.
    Nothing,
}

/// Decide whether `found` (the open PR on the branch after the phase) may be
/// retracted, given `preexisting` (the open PR on the same branch before the
/// implementer was launched). Pure.
// trace:TASK-1529 | ai:claude
pub(crate) fn decide_target(found: Option<u64>, preexisting: Option<u64>) -> RetractionTarget {
    match found {
        None => RetractionTarget::Nothing,
        Some(pr) if preexisting == Some(pr) => RetractionTarget::NotOurs(pr),
        Some(pr) => RetractionTarget::Retract(pr),
    }
}

/// How a PR was taken out of review. Draft is preferred (it keeps the review
/// history and the branch link); closing is the fallback for a forge that
/// cannot mark a change as draft, or whose draft conversion did not take.
// trace:TASK-1529 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetractionMode {
    Draft,
    Closed,
}

impl RetractionMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            RetractionMode::Draft => "draft",
            RetractionMode::Closed => "closed",
        }
    }

    /// The past-tense verb for user-facing lines.
    pub(crate) fn verb(self) -> &'static str {
        match self {
            RetractionMode::Draft => "converted to draft",
            RetractionMode::Closed => "closed",
        }
    }
}

/// What the retraction did. The phase-failure message and the marker are
/// both derived from this so they cannot disagree.
// trace:TASK-1529 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RetractionOutcome {
    /// The forge reports the change as a draft (or CLOSED) after the call.
    Retracted { pr: u64, mode: RetractionMode },
    /// The change was already a draft, closed or merged before this
    /// retraction ran (an earlier attempt, or a human) — nothing to do, and
    /// nothing claimed.
    AlreadyRetracted { pr: u64 },
    /// Neither the draft conversion nor the close left the forge reporting
    /// the change retracted. The PR is published with failing guards.
    RetractionFailed { pr: u64, error: String },
    /// The PR was open before the phase started; the agent did not open it.
    NotOurs { pr: u64 },
    /// The branch's change already merged — the drive's AlreadyMerged
    /// handling owns it.
    AlreadyMerged { pr: u64 },
    /// No open PR on the branch.
    NoOpenPr,
}

impl RetractionOutcome {
    /// True when a PR with failing guards is still published.
    #[cfg(test)]
    pub(crate) fn leaves_pr_open(&self) -> bool {
        matches!(self, RetractionOutcome::RetractionFailed { .. })
    }
}

/// The durable record of one retraction attempt for one PR.
// trace:TASK-1529 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RetractionRecord {
    pub schema_version: u32,
    pub pr: u64,
    pub spec: String,
    pub branch: String,
    /// `retracted` or `retraction-failed`.
    pub state: String,
    /// `draft` or `closed` when retracted; absent when the retraction failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// The guard output that refused publication.
    pub detail: String,
    /// The close error or the post-close state, when the retraction failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub recorded_at: String,
    /// How many retraction attempts this PR has seen (idempotent re-runs
    /// increment it rather than rewriting history).
    #[serde(default)]
    pub attempts: u32,
}

pub(crate) const STATE_RETRACTED: &str = "retracted";
pub(crate) const STATE_FAILED: &str = "retraction-failed";

/// The state one marker records: a verified retraction (with how it was
/// done) or a failed one.
// trace:TASK-1529 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordState {
    Retracted(RetractionMode),
    Failed,
}

impl RecordState {
    fn as_str(self) -> &'static str {
        match self {
            RecordState::Retracted(_) => STATE_RETRACTED,
            RecordState::Failed => STATE_FAILED,
        }
    }

    fn mode(self) -> Option<RetractionMode> {
        match self {
            RecordState::Retracted(mode) => Some(mode),
            RecordState::Failed => None,
        }
    }
}

fn records_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("preflight-retractions")
}

/// The marker path for one PR. Public so callers can log it.
// trace:TASK-1529 | ai:claude
pub(crate) fn marker_path(project_root: &Path, pr: u64) -> PathBuf {
    records_dir(project_root).join(format!("PR-{pr}.json"))
}

/// Read the marker for `pr`, if any.
// trace:TASK-1529 | ai:claude
pub(crate) fn read_record(project_root: &Path, pr: u64) -> Option<RetractionRecord> {
    let body = std::fs::read_to_string(marker_path(project_root, pr)).ok()?;
    serde_json::from_str(&body).ok()
}

/// Write (or refresh) the marker for `pr`. A re-run bumps `attempts` and
/// keeps the first `recorded_at` only when the state is unchanged, so a
/// later failure after an earlier success is visible as a new record.
// trace:TASK-1529 | ai:claude
pub(crate) fn write_record(
    project_root: &Path,
    pr: u64,
    spec: &str,
    branch: &str,
    state: RecordState,
    detail: &str,
    error: Option<&str>,
) -> std::io::Result<PathBuf> {
    let (state, mode) = (state.as_str(), state.mode());
    let dir = records_dir(project_root);
    std::fs::create_dir_all(&dir)?;
    let previous = read_record(project_root, pr);
    let attempts = previous.as_ref().map(|r| r.attempts).unwrap_or(0) + 1;
    let recorded_at = match &previous {
        Some(p) if p.state == state => p.recorded_at.clone(),
        _ => chrono::Utc::now().to_rfc3339(),
    };
    let record = RetractionRecord {
        schema_version: 1,
        pr,
        spec: spec.to_string(),
        branch: branch.to_string(),
        state: state.to_string(),
        mode: mode.map(|m| m.as_str().to_string()),
        detail: detail.to_string(),
        error: error.map(str::to_string),
        recorded_at,
        attempts,
    };
    let body = serde_json::to_vec_pretty(&record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let path = marker_path(project_root, pr);
    aida_core::fs_atomic::write_atomic(&path, &body)?;
    Ok(path)
}

/// Interpret the forge's post-close state. Only an observed `Closed` counts
/// as retracted; an unreadable status is a failure, because "we could not
/// tell" is not "we saw it closed" (PRIN-5). Pure.
// trace:TASK-1529 | ai:claude
pub(crate) fn verify_closed(
    observed: Result<crate::forge::ChangeState, String>,
) -> Result<(), String> {
    match observed {
        Ok(crate::forge::ChangeState::Closed) => Ok(()),
        Ok(crate::forge::ChangeState::Merged) => Err(
            "the change reports MERGED after the close call — it shipped before the guards \
             could retract it"
                .to_string(),
        ),
        Ok(crate::forge::ChangeState::Open) => {
            Err("the change still reports OPEN after the close call".to_string())
        }
        Err(e) => Err(format!(
            "could not read the change state after the close call: {e}"
        )),
    }
}

/// Interpret the forge's post-conversion draft flag. Only an observed draft
/// counts; an unreadable flag is a failure for the same reason as
/// [`verify_closed`]. Pure.
// trace:TASK-1529 | ai:claude
pub(crate) fn verify_drafted(observed: Result<bool, String>) -> Result<(), String> {
    match observed {
        Ok(true) => Ok(()),
        Ok(false) => Err("the change still reports ready for review after the draft \
                          conversion"
            .to_string()),
        Err(e) => Err(format!(
            "could not read the change's draft state after the conversion: {e}"
        )),
    }
}

/// The comment posted on a PR that was converted to draft because the
/// publication guards refused it. Mirrors
/// `implementer_preflight::retraction_notice` (the close reason) for the
/// gentler retraction.
// trace:TASK-1529 | ai:claude
pub(crate) fn draft_notice(detail: &str) -> String {
    format!(
        "Converted to draft automatically: the publication guards refused this change \
         before it was reviewed.\n\n{detail}\n\nThe branch is untouched — fix the guard \
         failure and mark the PR ready for review, or let the drain retry."
    )
}

/// The one-line detail a merge-hold placed on a PR whose retraction failed
/// carries, so `aida human` / `aida status --awaiting` explain the hold
/// without this module.
// trace:TASK-1529 | ai:claude
pub(crate) fn failed_retraction_hold_detail(spec: &str, error: &str) -> String {
    format!(
        "publication guards refused {spec} and the automatic retraction failed ({error}); \
         close the PR or fix the guards before merging — see .aida/preflight-retractions/"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::ChangeState;

    #[test]
    fn task_1529_target_is_retract_only_for_a_pr_the_phase_opened() {
        assert_eq!(decide_target(None, None), RetractionTarget::Nothing);
        assert_eq!(decide_target(None, Some(7)), RetractionTarget::Nothing);
        assert_eq!(decide_target(Some(42), None), RetractionTarget::Retract(42));
        assert_eq!(
            decide_target(Some(42), Some(42)),
            RetractionTarget::NotOurs(42),
            "the PR that was open before the implementer ran is not ours"
        );
        assert_eq!(
            decide_target(Some(43), Some(42)),
            RetractionTarget::Retract(43),
            "a NEW PR on the branch is ours even when an older one pre-existed"
        );
    }

    #[test]
    fn task_1529_only_an_observed_close_counts_as_retracted() {
        assert!(verify_closed(Ok(ChangeState::Closed)).is_ok());
        assert!(verify_closed(Ok(ChangeState::Open))
            .unwrap_err()
            .contains("still reports OPEN"));
        assert!(verify_closed(Ok(ChangeState::Merged))
            .unwrap_err()
            .contains("MERGED"));
        assert!(verify_closed(Err("gh: timeout".into()))
            .unwrap_err()
            .contains("could not read"));
    }

    #[test]
    fn task_1529_only_an_observed_draft_counts_as_retracted() {
        assert!(verify_drafted(Ok(true)).is_ok());
        assert!(verify_drafted(Ok(false))
            .unwrap_err()
            .contains("still reports ready for review"));
        assert!(verify_drafted(Err("gh: timeout".into()))
            .unwrap_err()
            .contains("could not read"));
    }

    #[test]
    fn task_1529_draft_notice_carries_the_guard_failure_and_the_next_step() {
        let detail = "guard `Check formatting` failed:\nsrc/x.rs needs rustfmt";
        let note = draft_notice(detail);
        assert!(note.contains(detail), "{note}");
        assert!(note.contains("Converted to draft automatically"), "{note}");
        assert!(note.contains("branch is untouched"), "{note}");
    }

    #[test]
    fn task_1529_marker_round_trips_and_counts_attempts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(read_record(root, 42).is_none());
        let path = write_record(
            root,
            42,
            "TASK-1",
            "claude/task-1",
            RecordState::Failed,
            "guard `fmt` failed",
            Some("gh exploded"),
        )
        .unwrap();
        assert_eq!(path, marker_path(root, 42));
        let first = read_record(root, 42).unwrap();
        assert_eq!(first.state, STATE_FAILED);
        assert_eq!(first.attempts, 1);
        assert_eq!(first.error.as_deref(), Some("gh exploded"));
        assert_eq!(first.spec, "TASK-1");

        // A second failed attempt keeps the first timestamp and bumps attempts.
        write_record(
            root,
            42,
            "TASK-1",
            "claude/task-1",
            RecordState::Failed,
            "guard `fmt` failed",
            Some("still open"),
        )
        .unwrap();
        let second = read_record(root, 42).unwrap();
        assert_eq!(second.attempts, 2);
        assert_eq!(second.recorded_at, first.recorded_at);
        assert_eq!(second.error.as_deref(), Some("still open"));

        // A later success is a new state with its own timestamp semantics.
        write_record(
            root,
            42,
            "TASK-1",
            "claude/task-1",
            RecordState::Retracted(RetractionMode::Draft),
            "guard `fmt` failed",
            None,
        )
        .unwrap();
        let third = read_record(root, 42).unwrap();
        assert_eq!(third.state, STATE_RETRACTED);
        assert_eq!(third.mode.as_deref(), Some("draft"));
        assert_eq!(third.attempts, 3);
        assert!(third.error.is_none());
        assert!(first.mode.is_none(), "a failed retraction records no mode");
    }
}
