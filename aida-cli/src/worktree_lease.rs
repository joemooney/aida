//! TASK-634 (SPIKE-41 slice): pure derivation of an AIDA lease record from a
//! Claude Code WorktreeCreate / WorktreeRemove hook payload.
//!
//! Substrate-CAPTURE only: given the hook payload, compute the deterministic
//! lease fields to write (on create) or the path to clear (on remove). The
//! non-deterministic fields (id, started_at, hostname, owner) are filled at
//! write time by the existing lease-creation machinery — they need a
//! clock/uuid/git-config and so are not part of this pure core.
//!
//! Motivation: a Claude Code Workflow's parallel worktree-isolated agents
//! (`isolation: "worktree"`) are invisible to the AIDA substrate today — they
//! provision worktrees the harness owns, ship code, and populate zero lease
//! state (the exact gap BUG-431 exposed, and the source of the orphan-worktree
//! pile-up `aida doctor` later has to reap). Wiring a WorktreeCreate hook to
//! register a lease (and WorktreeRemove to release it) closes that gap.
//!
//! The live hook wiring + `aida init` scaffold are DEFERRED until SPIKE-41
//! confirms the Claude Code event timing/payload (operator decision
//! 2026-06-06: "build the pure-testable core now … don't wire against
//! unverified harness behavior"). Until then these functions have no caller, so
//! the module is `allow(dead_code)`. trace:TASK-634 | ai:claude
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// The generic lease scope for a harness worktree whose branch carries no
/// recognizable SPEC-ID. trace:TASK-634
pub(crate) const HARNESS_WORKTREE_SCOPE: &str = "harness-worktree";

/// The subset of a Claude Code worktree hook payload we capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreePayload {
    /// The worktree path the harness provisioned (WorktreeCreate) or is
    /// tearing down (WorktreeRemove).
    pub path: PathBuf,
    /// The branch the worktree is on, when the event carries it.
    pub branch: Option<String>,
}

/// The deterministic lease fields derived from a WorktreeCreate payload — the
/// subset a hook can compute without a clock/uuid/git-config. trace:TASK-634
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeLeaseSpec {
    /// The lease scope: a SPEC-ID derived from the branch when recognizable,
    /// else the generic [`HARNESS_WORKTREE_SCOPE`].
    pub scope: String,
    /// The branch the worktree is on (empty when the payload omits it).
    pub branch: String,
    /// The worktree path to record on the lease.
    pub worktree_path: PathBuf,
}

/// The known AIDA spec-type branch prefixes (`task-688-…` → `TASK-688`).
const SPEC_TYPES: &[&str] = &[
    "fr", "func", "nfr", "sys", "user", "bug", "epic", "story", "task", "spike", "sprint", "adr",
    "meta", "doc",
];

/// Extract a SPEC-ID (e.g. `TASK-688`) from a branch name like
/// `task-688-aida-release-after-pr`. Recognizes the standard
/// `<type>-<number>-<slug>` convention; returns `None` for harness-generated
/// names like `worktree-agent-<hex>` or anything without a `<type>-<number>`
/// head. trace:TASK-634
pub(crate) fn spec_id_from_branch(branch: &str) -> Option<String> {
    let mut parts = branch.split('-');
    let kind = parts.next()?.to_ascii_lowercase();
    let num = parts.next()?;
    if !SPEC_TYPES.contains(&kind.as_str()) {
        return None;
    }
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!("{}-{}", kind.to_uppercase(), num))
}

/// Derive the lease record to WRITE for a WorktreeCreate payload. The scope is
/// the branch's SPEC-ID when derivable, else the generic harness scope so the
/// worktree is still tracked. trace:TASK-634
pub(crate) fn lease_spec_for_create(payload: &WorktreePayload) -> WorktreeLeaseSpec {
    let scope = payload
        .branch
        .as_deref()
        .and_then(spec_id_from_branch)
        .unwrap_or_else(|| HARNESS_WORKTREE_SCOPE.to_string());
    WorktreeLeaseSpec {
        scope,
        branch: payload.branch.clone().unwrap_or_default(),
        worktree_path: payload.path.clone(),
    }
}

/// The worktree path whose lease should be CLEARED for a WorktreeRemove
/// payload — the match key for the existing lease-by-worktree lookup.
/// trace:TASK-634
pub(crate) fn lease_path_for_remove(payload: &WorktreePayload) -> &Path {
    &payload.path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_id_from_named_branch() {
        assert_eq!(
            spec_id_from_branch("task-688-aida-release"),
            Some("TASK-688".into())
        );
        assert_eq!(
            spec_id_from_branch("bug-466-windows"),
            Some("BUG-466".into())
        );
        assert_eq!(
            spec_id_from_branch("story-510-gitlab-ci"),
            Some("STORY-510".into())
        );
        // Already-uppercase head normalizes too.
        assert_eq!(spec_id_from_branch("BUG-471-x"), Some("BUG-471".into()));
    }

    #[test]
    fn spec_id_none_for_harness_and_unrecognized_branches() {
        // Harness auto-generated worktree branch — no SPEC-ID.
        assert_eq!(
            spec_id_from_branch("worktree-agent-a0f3696de475d07c3"),
            None
        );
        // Default branches + slugs without a <type>-<number> head.
        assert_eq!(spec_id_from_branch("main"), None);
        assert_eq!(spec_id_from_branch("random-branch"), None);
        // Known type but non-numeric id.
        assert_eq!(spec_id_from_branch("task-foo-bar"), None);
        assert_eq!(spec_id_from_branch(""), None);
    }

    #[test]
    fn create_derives_spec_scope_from_branch() {
        let p = WorktreePayload {
            path: PathBuf::from("/repo/.worktrees/task-688"),
            branch: Some("task-688-aida-release".into()),
        };
        let spec = lease_spec_for_create(&p);
        assert_eq!(spec.scope, "TASK-688");
        assert_eq!(spec.branch, "task-688-aida-release");
        assert_eq!(
            spec.worktree_path,
            PathBuf::from("/repo/.worktrees/task-688")
        );
    }

    #[test]
    fn create_falls_back_to_harness_scope() {
        // Harness branch → generic scope, still tracked.
        let p = WorktreePayload {
            path: PathBuf::from("/repo/.claude/worktrees/agent-abc"),
            branch: Some("worktree-agent-abc".into()),
        };
        assert_eq!(lease_spec_for_create(&p).scope, HARNESS_WORKTREE_SCOPE);

        // No branch in the payload at all → generic scope, branch empty.
        let p2 = WorktreePayload {
            path: PathBuf::from("/repo/.claude/worktrees/agent-def"),
            branch: None,
        };
        let spec = lease_spec_for_create(&p2);
        assert_eq!(spec.scope, HARNESS_WORKTREE_SCOPE);
        assert_eq!(spec.branch, "");
    }

    #[test]
    fn remove_returns_the_payload_path() {
        let p = WorktreePayload {
            path: PathBuf::from("/repo/.claude/worktrees/agent-abc"),
            branch: None,
        };
        assert_eq!(
            lease_path_for_remove(&p),
            Path::new("/repo/.claude/worktrees/agent-abc")
        );
    }
}
